//! Chunked, parallel and resumable HTTP downloader.
//!
//! * the file is split into byte ranges that are fetched by several
//!   concurrent connections (optionally capped globally across downloads);
//! * every chunk is retried with exponential backoff, rotating through
//!   mirror URLs, and guarded by an idle timeout;
//! * progress is persisted in a small JSON sidecar (`<dest>.state`) after
//!   every finished chunk, so an interrupted download resumes where it
//!   stopped — even with freshly signed URLs, as long as the resume key and
//!   the remote size match;
//! * servers without range support fall back to a single streamed request.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, ACCEPT_ENCODING, CONTENT_RANGE, RANGE};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncSeekExt, AsyncWriteExt};
use tokio::sync::{mpsc, Semaphore};
use tokio::task::JoinSet;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("server answered {status} for {url}")]
    Status { status: StatusCode, url: String },
    #[error("connection stalled for more than {0:?}")]
    Timeout(Duration),
    #[error("received {got} bytes, expected {expected}")]
    Incomplete { expected: u64, got: u64 },
    #[error("invalid response: {0}")]
    InvalidResponse(String),
    #[error("no download url given")]
    NoUrl,
    #[error("download task failed: {0}")]
    Task(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Receives progress notifications. All methods have no-op defaults.
pub trait ProgressSink: Send + Sync {
    /// Called once the size is known. `already` bytes were restored from a
    /// previous, interrupted run.
    fn start(&self, _total: Option<u64>, _already: u64) {}
    /// New bytes arrived.
    fn advance(&self, _bytes: u64) {}
    /// Bytes reported via [`Self::advance`] were discarded (chunk retried).
    fn rewind(&self, _bytes: u64) {}
    /// A request failed and will be retried.
    fn retry(&self, _attempt: u32, _error: &Error) {}
}

/// A [`ProgressSink`] that ignores everything.
pub struct NoProgress;
impl ProgressSink for NoProgress {}

#[derive(Debug, Clone)]
pub struct DownloadOptions {
    /// Concurrent range requests per file.
    pub connections: usize,
    /// Chunk size in bytes; `None` picks one from the file size.
    pub chunk_size: Option<u64>,
    /// Retries per chunk (in addition to the first attempt).
    pub retries: u32,
    /// Abort a request when no data arrives for this long.
    pub idle_timeout: Duration,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            connections: 4,
            chunk_size: None,
            retries: 5,
            idle_timeout: Duration::from_secs(20),
        }
    }
}

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    /// Primary URL followed by mirrors.
    pub urls: Vec<String>,
    /// Where the data is written.
    pub dest: PathBuf,
    /// Identifies the content; a saved partial download is only resumed when
    /// the key (and the remote size) match. URLs are not part of the key
    /// because CDN URLs are re-signed on every request.
    pub resume_key: String,
    /// Extra headers (referer, user agent ...).
    pub headers: HeaderMap,
}

#[derive(Debug, Clone, Copy)]
pub struct DownloadReport {
    pub total: u64,
    /// Bytes restored from a previous run.
    pub resumed: u64,
}

const STATE_VERSION: u32 = 1;
const MIN_AUTO_CHUNK: u64 = 512 * 1024;
const MAX_AUTO_CHUNK: u64 = 8 * 1024 * 1024;

#[derive(Debug, Serialize, Deserialize)]
struct State {
    version: u32,
    key: String,
    total: u64,
    chunk_size: u64,
    done: Vec<bool>,
}

impl State {
    fn chunk_range(&self, idx: usize) -> (u64, u64) {
        let start = idx as u64 * self.chunk_size;
        let end = (start + self.chunk_size).min(self.total) - 1;
        (start, end)
    }

    fn done_bytes(&self) -> u64 {
        (0..self.done.len())
            .filter(|i| self.done[*i])
            .map(|i| {
                let (s, e) = self.chunk_range(i);
                e - s + 1
            })
            .sum()
    }
}

/// Path of the resume sidecar belonging to `dest`.
pub fn state_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(".state");
    PathBuf::from(s)
}

/// Chunk size used when [`DownloadOptions::chunk_size`] is `None`: about
/// four chunks per connection, clamped to 512 KiB ..= 8 MiB.
pub fn auto_chunk_size(total: u64, connections: usize) -> u64 {
    (total / (connections.max(1) as u64 * 4)).clamp(MIN_AUTO_CHUNK, MAX_AUTO_CHUNK)
}

#[derive(Clone)]
pub struct Downloader {
    client: reqwest::Client,
    opts: DownloadOptions,
    limiter: Option<Arc<Semaphore>>,
}

struct Probe {
    total: Option<u64>,
    ranges: bool,
}

impl Downloader {
    pub fn new(client: reqwest::Client, opts: DownloadOptions) -> Self {
        Self {
            client,
            opts,
            limiter: None,
        }
    }

    /// Caps the number of simultaneous requests across every download made
    /// through this downloader (and its clones).
    pub fn with_global_limit(mut self, max_connections: usize) -> Self {
        self.limiter = Some(Arc::new(Semaphore::new(max_connections.max(1))));
        self
    }

    pub fn options(&self) -> &DownloadOptions {
        &self.opts
    }

    /// Downloads `req` to `req.dest`, resuming a previous partial download
    /// when possible.
    pub async fn download(
        &self,
        req: &DownloadRequest,
        progress: Arc<dyn ProgressSink>,
    ) -> Result<DownloadReport> {
        if req.urls.is_empty() {
            return Err(Error::NoUrl);
        }
        let urls = Arc::new(UrlPool::new(req.urls.clone()));
        let probe = self.probe(req, &urls, progress.as_ref()).await?;
        match probe {
            Probe {
                ranges: true,
                total: Some(total),
            } if total > 0 => self.download_ranged(req, urls, total, progress).await,
            Probe { total, .. } => self.download_single(req, urls, total, progress).await,
        }
    }

    fn base_request(&self, url: &str, headers: &HeaderMap) -> reqwest::RequestBuilder {
        self.client
            .get(url)
            .headers(headers.clone())
            .header(ACCEPT_ENCODING, HeaderValue::from_static("identity"))
    }

    async fn permit(&self) -> Option<tokio::sync::OwnedSemaphorePermit> {
        match &self.limiter {
            Some(s) => s.clone().acquire_owned().await.ok(),
            None => None,
        }
    }

    fn backoff(attempt: u32) -> Duration {
        Duration::from_millis(300u64.saturating_mul(1 << attempt.min(5)))
            .min(Duration::from_secs(8))
    }

    /// Asks for the first byte to learn the size and range support.
    async fn probe(
        &self,
        req: &DownloadRequest,
        urls: &UrlPool,
        progress: &dyn ProgressSink,
    ) -> Result<Probe> {
        let mut attempt = 0;
        let mut last = None;
        loop {
            let idx = urls.pick(last);
            last = Some(idx);
            let url = &urls.urls[idx];
            let res = async {
                let _permit = self.permit().await;
                let resp = tokio::time::timeout(
                    self.opts.idle_timeout,
                    self.base_request(url, &req.headers)
                        .header(RANGE, "bytes=0-0")
                        .send(),
                )
                .await
                .map_err(|_| Error::Timeout(self.opts.idle_timeout))??;
                match resp.status() {
                    StatusCode::PARTIAL_CONTENT => {
                        let total = resp
                            .headers()
                            .get(CONTENT_RANGE)
                            .and_then(|v| v.to_str().ok())
                            .and_then(parse_content_range)
                            .and_then(|(_, total)| total);
                        let fallback = resp.content_length();
                        // Drain the 1 byte body so the connection is reused.
                        let _ = tokio::time::timeout(self.opts.idle_timeout, resp.bytes()).await;
                        Ok(Probe {
                            ranges: total.is_some(),
                            total: total.or(fallback),
                        })
                    }
                    StatusCode::OK => Ok(Probe {
                        ranges: false,
                        total: resp.content_length(),
                    }),
                    status => Err(Error::Status {
                        status,
                        url: url.clone(),
                    }),
                }
            }
            .await;
            match res {
                Ok(p) => {
                    urls.reward(idx);
                    return Ok(p);
                }
                Err(e) if attempt < self.opts.retries + urls.len() as u32 - 1 => {
                    urls.penalize(idx, &e);
                    progress.retry(attempt + 1, &e);
                    // Try every mirror once before backing off.
                    if (attempt as usize + 1).is_multiple_of(urls.len()) {
                        tokio::time::sleep(Self::backoff(attempt)).await;
                    }
                    attempt += 1;
                }
                Err(e) => return Err(e),
            }
        }
    }

    async fn download_ranged(
        &self,
        req: &DownloadRequest,
        urls: Arc<UrlPool>,
        total: u64,
        progress: Arc<dyn ProgressSink>,
    ) -> Result<DownloadReport> {
        let state_file = state_path(&req.dest);
        let existing_len = tokio::fs::metadata(&req.dest).await.map(|m| m.len()).ok();
        let restored = load_state(&state_file).await.filter(|s| {
            s.version == STATE_VERSION
                && s.key == req.resume_key
                && s.total == total
                && s.chunk_size > 0
                && s.done.len() as u64 == total.div_ceil(s.chunk_size)
                && existing_len == Some(total)
        });

        let (mut state, mut file) = match restored {
            Some(state) => {
                let file = tokio::fs::OpenOptions::new()
                    .write(true)
                    .open(&req.dest)
                    .await?;
                (state, file)
            }
            None => {
                let chunk_size = self
                    .opts
                    .chunk_size
                    .filter(|c| *c > 0)
                    .unwrap_or_else(|| auto_chunk_size(total, self.opts.connections));
                let state = State {
                    version: STATE_VERSION,
                    key: req.resume_key.clone(),
                    total,
                    chunk_size,
                    done: vec![false; total.div_ceil(chunk_size) as usize],
                };
                if let Some(parent) = req.dest.parent() {
                    tokio::fs::create_dir_all(parent).await?;
                }
                let file = tokio::fs::File::create(&req.dest).await?;
                file.set_len(total).await?;
                save_state(&state_file, &state).await?;
                (state, file)
            }
        };

        let resumed = state.done_bytes();
        progress.start(Some(total), resumed);

        let pending: VecDeque<usize> = (0..state.done.len()).filter(|i| !state.done[*i]).collect();
        let workers_n = self.opts.connections.max(1).min(pending.len());
        let queue = Arc::new(Mutex::new(pending));
        let (tx, mut rx) = mpsc::channel::<(usize, Vec<u8>)>(workers_n.max(1));
        let ranges: Arc<Vec<(u64, u64)>> = Arc::new(
            (0..state.done.len())
                .map(|i| state.chunk_range(i))
                .collect(),
        );

        let mut workers = JoinSet::new();
        for _ in 0..workers_n {
            let this = self.clone();
            let queue = queue.clone();
            let tx = tx.clone();
            let urls = urls.clone();
            let headers = req.headers.clone();
            let progress = progress.clone();
            let ranges = ranges.clone();
            workers.spawn(async move {
                loop {
                    let Some(idx) = queue.lock().unwrap().pop_front() else {
                        return Ok::<(), Error>(());
                    };
                    let (start, end) = ranges[idx];
                    let buf = this
                        .fetch_chunk_with_retry(&urls, &headers, start, end, progress.as_ref())
                        .await?;
                    if tx.send((idx, buf)).await.is_err() {
                        return Ok(());
                    }
                }
            });
        }
        drop(tx);

        let mut failure: Option<Error> = None;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some((idx, buf)) => {
                        let (start, _) = state.chunk_range(idx);
                        file.seek(std::io::SeekFrom::Start(start)).await?;
                        file.write_all(&buf).await?;
                        state.done[idx] = true;
                        // Data must reach the OS before the chunk is recorded.
                        file.flush().await?;
                        save_state(&state_file, &state).await?;
                    }
                    None => break,
                },
                Some(res) = workers.join_next(), if failure.is_none() => {
                    let res = res.map_err(|e| Error::Task(e.to_string())).and_then(|r| r);
                    if let Err(e) = res {
                        // Stop the remaining connections; finished chunks are
                        // already recorded for the next attempt.
                        workers.abort_all();
                        failure = Some(e);
                    }
                }
            }
        }
        if let Some(e) = failure {
            return Err(e);
        }
        if state.done.iter().any(|d| !d) {
            return Err(Error::Task("download ended with missing chunks".into()));
        }
        file.sync_all().await?;
        drop(file);
        let _ = tokio::fs::remove_file(&state_file).await;
        Ok(DownloadReport { total, resumed })
    }

    async fn fetch_chunk_with_retry(
        &self,
        urls: &UrlPool,
        headers: &HeaderMap,
        start: u64,
        end: u64,
        progress: &dyn ProgressSink,
    ) -> Result<Vec<u8>> {
        let mut attempt = 0u32;
        let mut last = None;
        loop {
            // Healthiest mirror first; a retry avoids the URL that just failed.
            let idx = urls.pick(last);
            last = Some(idx);
            let mut received = 0u64;
            let res = self
                .fetch_chunk(
                    &urls.urls[idx],
                    headers,
                    start,
                    end,
                    progress,
                    &mut received,
                )
                .await;
            match res {
                Ok(buf) => {
                    urls.reward(idx);
                    return Ok(buf);
                }
                Err(e) => {
                    urls.penalize(idx, &e);
                    progress.rewind(received);
                    if attempt >= self.opts.retries {
                        return Err(e);
                    }
                    attempt += 1;
                    progress.retry(attempt, &e);
                    tokio::time::sleep(Self::backoff(attempt - 1)).await;
                }
            }
        }
    }

    async fn fetch_chunk(
        &self,
        url: &str,
        headers: &HeaderMap,
        start: u64,
        end: u64,
        progress: &dyn ProgressSink,
        received: &mut u64,
    ) -> Result<Vec<u8>> {
        let _permit = self.permit().await;
        let idle = self.opts.idle_timeout;
        let expected = end - start + 1;
        let mut resp = tokio::time::timeout(
            idle,
            self.base_request(url, headers)
                .header(RANGE, format!("bytes={start}-{end}"))
                .send(),
        )
        .await
        .map_err(|_| Error::Timeout(idle))??;
        if resp.status() != StatusCode::PARTIAL_CONTENT {
            return Err(Error::Status {
                status: resp.status(),
                url: url.to_owned(),
            });
        }
        let range = resp
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|v| v.to_str().ok())
            .and_then(parse_content_range);
        if let Some(((s, e), _)) = range {
            if s != start || e != end {
                return Err(Error::InvalidResponse(format!(
                    "asked for bytes {start}-{end}, got {s}-{e}"
                )));
            }
        }
        let mut buf = Vec::with_capacity(expected as usize);
        loop {
            let next = tokio::time::timeout(idle, resp.chunk())
                .await
                .map_err(|_| Error::Timeout(idle))??;
            let Some(bytes) = next else { break };
            if buf.len() as u64 + bytes.len() as u64 > expected {
                return Err(Error::InvalidResponse(
                    "server sent more data than requested".into(),
                ));
            }
            buf.extend_from_slice(&bytes);
            *received += bytes.len() as u64;
            progress.advance(bytes.len() as u64);
        }
        if buf.len() as u64 != expected {
            return Err(Error::Incomplete {
                expected,
                got: buf.len() as u64,
            });
        }
        Ok(buf)
    }

    /// Plain sequential download for servers without range support.
    async fn download_single(
        &self,
        req: &DownloadRequest,
        urls: Arc<UrlPool>,
        total: Option<u64>,
        progress: Arc<dyn ProgressSink>,
    ) -> Result<DownloadReport> {
        let _ = tokio::fs::remove_file(state_path(&req.dest)).await;
        if let Some(parent) = req.dest.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        progress.start(total, 0);
        let idle = self.opts.idle_timeout;
        let mut attempt = 0u32;
        let mut last = None;
        loop {
            let idx = urls.pick(last);
            last = Some(idx);
            let url = &urls.urls[idx];
            let mut received = 0u64;
            let res: Result<u64> = async {
                let _permit = self.permit().await;
                let mut file = tokio::fs::File::create(&req.dest).await?;
                let mut resp =
                    tokio::time::timeout(idle, self.base_request(url, &req.headers).send())
                        .await
                        .map_err(|_| Error::Timeout(idle))??;
                if !resp.status().is_success() {
                    return Err(Error::Status {
                        status: resp.status(),
                        url: url.clone(),
                    });
                }
                let expected = resp.content_length().or(total);
                while let Some(bytes) = tokio::time::timeout(idle, resp.chunk())
                    .await
                    .map_err(|_| Error::Timeout(idle))??
                {
                    file.write_all(&bytes).await?;
                    received += bytes.len() as u64;
                    progress.advance(bytes.len() as u64);
                }
                if let Some(expected) = expected {
                    if expected != received {
                        return Err(Error::Incomplete {
                            expected,
                            got: received,
                        });
                    }
                }
                file.sync_all().await?;
                Ok(received)
            }
            .await;
            match res {
                Ok(n) => {
                    return Ok(DownloadReport {
                        total: n,
                        resumed: 0,
                    })
                }
                Err(e) => {
                    urls.penalize(idx, &e);
                    progress.rewind(received);
                    if attempt >= self.opts.retries {
                        return Err(e);
                    }
                    attempt += 1;
                    progress.retry(attempt, &e);
                    tokio::time::sleep(Self::backoff(attempt - 1)).await;
                }
            }
        }
    }
}

/// Mirror URLs with a shared failure count, so every connection prefers the
/// healthiest one.
struct UrlPool {
    urls: Vec<String>,
    failures: Vec<AtomicU32>,
}

impl UrlPool {
    fn new(urls: Vec<String>) -> Self {
        let failures = urls.iter().map(|_| AtomicU32::new(0)).collect();
        Self { urls, failures }
    }

    fn len(&self) -> usize {
        self.urls.len()
    }

    /// Index of the healthiest URL. On a retry the URL that just failed
    /// loses ties, so equally healthy mirrors are rotated.
    fn pick(&self, avoid: Option<usize>) -> usize {
        (0..self.urls.len())
            .min_by_key(|i| {
                (
                    self.failures[*i].load(Ordering::Relaxed),
                    Some(*i) == avoid,
                    *i,
                )
            })
            .unwrap_or(0)
    }

    /// Records a failure. Client errors (403 expired / 404 missing) mark the
    /// mirror as much worse than a transient network problem.
    fn penalize(&self, idx: usize, err: &Error) {
        let weight = match err {
            Error::Status { status, .. } if status.is_client_error() => 4,
            _ => 1,
        };
        self.failures[idx].fetch_add(weight, Ordering::Relaxed);
    }

    fn reward(&self, idx: usize) {
        let _ = self.failures[idx].try_update(Ordering::Relaxed, Ordering::Relaxed, |v| {
            Some(v.saturating_sub(1))
        });
    }
}

/// Parses `bytes <start>-<end>/<total|*>`.
fn parse_content_range(v: &str) -> Option<((u64, u64), Option<u64>)> {
    let v = v.trim().strip_prefix("bytes")?.trim_start();
    let (range, total) = v.split_once('/')?;
    let (s, e) = range.split_once('-')?;
    let total = match total.trim() {
        "*" => None,
        t => Some(t.parse().ok()?),
    };
    Some(((s.trim().parse().ok()?, e.trim().parse().ok()?), total))
}

async fn load_state(path: &Path) -> Option<State> {
    let data = tokio::fs::read(path).await.ok()?;
    serde_json::from_slice(&data).ok()
}

async fn save_state(path: &Path, state: &State) -> Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let data = serde_json::to_vec(state).map_err(std::io::Error::other)?;
    tokio::fs::write(&tmp, data).await?;
    tokio::fs::rename(&tmp, path).await?;
    Ok(())
}

#[cfg(test)]
mod tests;
