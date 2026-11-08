use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

/// Tiny HTTP/1.1 server serving `data` on every path except `/missing`.
struct TestServer {
    base: String,
    served: Arc<AtomicU64>,
    requests: Arc<AtomicUsize>,
}

#[derive(Clone, Copy)]
struct Behaviour {
    ranges: bool,
    /// The first request for every n-th distinct range is cut off half way
    /// (0 = never).
    flaky_every: usize,
}

async fn serve(data: Arc<Vec<u8>>, behaviour: Behaviour) -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let served = Arc::new(AtomicU64::new(0));
    let requests = Arc::new(AtomicUsize::new(0));
    let (s2, r2) = (served.clone(), requests.clone());
    let cut_ranges = Arc::new(Mutex::new(std::collections::HashSet::<u64>::new()));
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let data = data.clone();
            let served = s2.clone();
            let requests = r2.clone();
            let cut_ranges = cut_ranges.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                let req = String::from_utf8_lossy(&buf).to_string();
                let path = req.split_whitespace().nth(1).unwrap_or("/").to_string();
                requests.fetch_add(1, Ordering::SeqCst);
                if path == "/missing" {
                    let _ = sock
                        .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
                        .await;
                    return;
                }
                let range = req
                    .lines()
                    .find_map(|l| {
                        l.to_ascii_lowercase()
                            .strip_prefix("range: bytes=")
                            .map(str::to_owned)
                    })
                    .and_then(|r| {
                        let (s, e) = r.trim().split_once('-')?;
                        Some((s.parse::<u64>().ok()?, e.parse::<u64>().ok()?))
                    });
                let (head, body) = match range {
                    Some((s, e)) if behaviour.ranges => {
                        let e = e.min(data.len() as u64 - 1);
                        let body = &data[s as usize..=e as usize];
                        (
                            format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {s}-{e}/{}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                data.len(),
                                body.len()
                            ),
                            body,
                        )
                    }
                    _ => (
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            data.len()
                        ),
                        &data[..],
                    ),
                };
                let _ = sock.write_all(head.as_bytes()).await;
                let cut = match range {
                    Some((s, e)) if behaviour.flaky_every > 0 && s != e => {
                        let idx = s / (e - s + 1);
                        idx % behaviour.flaky_every as u64 == 0
                            && cut_ranges.lock().unwrap().insert(s)
                    }
                    _ => false,
                };
                let body = if cut && body.len() > 1 {
                    &body[..body.len() / 2]
                } else {
                    body
                };
                served.fetch_add(body.len() as u64, Ordering::SeqCst);
                let _ = sock.write_all(body).await;
                let _ = sock.shutdown().await;
            });
        }
    });
    TestServer {
        base,
        served,
        requests,
    }
}

#[derive(Default)]
struct Counter {
    total: AtomicU64,
    already: AtomicU64,
    net: AtomicU64,
    retries: AtomicUsize,
}

impl ProgressSink for Counter {
    fn start(&self, total: Option<u64>, already: u64) {
        self.total.store(total.unwrap_or(0), Ordering::SeqCst);
        self.already.store(already, Ordering::SeqCst);
    }
    fn advance(&self, n: u64) {
        self.net.fetch_add(n, Ordering::SeqCst);
    }
    fn rewind(&self, n: u64) {
        self.net.fetch_sub(n, Ordering::SeqCst);
    }
    fn retry(&self, _: u32, _: &Error) {
        self.retries.fetch_add(1, Ordering::SeqCst);
    }
}

fn test_data(n: usize) -> Arc<Vec<u8>> {
    Arc::new((0..n).map(|i| (i * 7 % 251) as u8).collect())
}

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nt-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d.join("file.part")
}

fn downloader(chunk: u64) -> Downloader {
    Downloader::new(
        reqwest::Client::new(),
        DownloadOptions {
            connections: 3,
            chunk_size: Some(chunk),
            retries: 4,
            idle_timeout: Duration::from_secs(5),
        },
    )
    .with_global_limit(4)
}

#[tokio::test]
async fn parallel_download_with_failures_and_mirror() {
    let data = test_data(10_500);
    let srv = serve(
        data.clone(),
        Behaviour {
            ranges: true,
            flaky_every: 3,
        },
    )
    .await;
    let dest = tmp("parallel");
    let req = DownloadRequest {
        urls: vec![
            format!("{}/missing", srv.base),
            format!("{}/file", srv.base),
        ],
        dest: dest.clone(),
        resume_key: "k".into(),
        headers: HeaderMap::new(),
    };
    let counter = Arc::new(Counter::default());
    let report = downloader(1000)
        .download(&req, counter.clone())
        .await
        .unwrap();
    assert_eq!(report.total, 10_500);
    assert_eq!(report.resumed, 0);
    assert_eq!(std::fs::read(&dest).unwrap(), *data);
    assert!(!state_path(&dest).exists(), "state removed after success");
    assert_eq!(counter.total.load(Ordering::SeqCst), 10_500);
    assert_eq!(counter.net.load(Ordering::SeqCst), 10_500);
    // the 404 mirror once + the first attempt of chunks 0, 3, 6, 9
    assert_eq!(counter.retries.load(Ordering::SeqCst), 5);
}

#[tokio::test]
async fn resumes_only_missing_chunks() {
    let data = test_data(10_000);
    let srv = serve(
        data.clone(),
        Behaviour {
            ranges: true,
            flaky_every: 0,
        },
    )
    .await;
    let dest = tmp("resume");

    // Simulate an interrupted run: chunks 0, 1 and 5 finished.
    let mut partial = vec![0u8; 10_000];
    let mut done = vec![false; 10];
    for i in [0usize, 1, 5] {
        partial[i * 1000..(i + 1) * 1000].copy_from_slice(&data[i * 1000..(i + 1) * 1000]);
        done[i] = true;
    }
    std::fs::write(&dest, &partial).unwrap();
    let state = State {
        version: STATE_VERSION,
        key: "bv:cid:30280".into(),
        total: 10_000,
        chunk_size: 1000,
        done,
    };
    std::fs::write(state_path(&dest), serde_json::to_vec(&state).unwrap()).unwrap();

    let req = DownloadRequest {
        urls: vec![format!("{}/fresh-signed-url", srv.base)],
        dest: dest.clone(),
        resume_key: "bv:cid:30280".into(),
        headers: HeaderMap::new(),
    };
    let counter = Arc::new(Counter::default());
    let report = downloader(4096)
        .download(&req, counter.clone())
        .await
        .unwrap();
    assert_eq!(report.resumed, 3000);
    assert_eq!(counter.already.load(Ordering::SeqCst), 3000);
    assert_eq!(std::fs::read(&dest).unwrap(), *data);
    // 1 probe byte + the 7 missing chunks
    assert_eq!(srv.served.load(Ordering::SeqCst), 1 + 7000);
}

#[tokio::test]
async fn different_resume_key_restarts() {
    let data = test_data(5000);
    let srv = serve(
        data.clone(),
        Behaviour {
            ranges: true,
            flaky_every: 0,
        },
    )
    .await;
    let dest = tmp("restart");
    std::fs::write(&dest, vec![9u8; 5000]).unwrap();
    let state = State {
        version: STATE_VERSION,
        key: "old-quality".into(),
        total: 5000,
        chunk_size: 1000,
        done: vec![true; 5],
    };
    std::fs::write(state_path(&dest), serde_json::to_vec(&state).unwrap()).unwrap();
    let req = DownloadRequest {
        urls: vec![format!("{}/x", srv.base)],
        dest: dest.clone(),
        resume_key: "new-quality".into(),
        headers: HeaderMap::new(),
    };
    let report = downloader(1000)
        .download(&req, Arc::new(NoProgress))
        .await
        .unwrap();
    assert_eq!(report.resumed, 0);
    assert_eq!(std::fs::read(&dest).unwrap(), *data);
}

#[tokio::test]
async fn falls_back_without_range_support() {
    let data = test_data(3333);
    let srv = serve(
        data.clone(),
        Behaviour {
            ranges: false,
            flaky_every: 0,
        },
    )
    .await;
    let dest = tmp("norange");
    let req = DownloadRequest {
        urls: vec![format!("{}/x", srv.base)],
        dest: dest.clone(),
        resume_key: "k".into(),
        headers: HeaderMap::new(),
    };
    let report = downloader(1000)
        .download(&req, Arc::new(NoProgress))
        .await
        .unwrap();
    assert_eq!(report.total, 3333);
    assert_eq!(std::fs::read(&dest).unwrap(), *data);
    assert_eq!(srv.requests.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn gives_up_after_retries() {
    let srv = serve(
        test_data(10),
        Behaviour {
            ranges: true,
            flaky_every: 0,
        },
    )
    .await;
    let dest = tmp("giveup");
    let req = DownloadRequest {
        urls: vec![format!("{}/missing", srv.base)],
        dest,
        resume_key: "k".into(),
        headers: HeaderMap::new(),
    };
    let mut d = downloader(1000);
    d.opts.retries = 1;
    let err = d.download(&req, Arc::new(NoProgress)).await.unwrap_err();
    assert!(matches!(err, Error::Status { .. }), "{err}");
}

#[test]
fn content_range_parsing() {
    assert_eq!(
        parse_content_range("bytes 0-0/1234"),
        Some(((0, 0), Some(1234)))
    );
    assert_eq!(parse_content_range("bytes 10-19/*"), Some(((10, 19), None)));
    assert_eq!(parse_content_range("items 1-2/3"), None);
    assert_eq!(auto_chunk_size(1000, 4), MIN_AUTO_CHUNK);
    assert_eq!(auto_chunk_size(10 << 30, 4), MAX_AUTO_CHUNK);
}
