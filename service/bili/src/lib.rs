//! Audio download pipeline for bilibili:
//!
//! 1. [`Service::resolve`] turns ids / urls / favorites / collections into
//!    [`Track`]s with metadata,
//! 2. [`Service::plan`] fixes the output file names ([`Job`]s),
//! 3. [`Service::run`] downloads them concurrently: best allowed audio
//!    stream → chunked, resumable download → remux (audio only, no video)
//!    → tags + cover → atomic rename.

pub mod input;
pub mod naming;
pub mod track;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use bili::{AudioPreference, AudioStream, Client, CookieJar, NavInfo};
use futures_util::{stream, StreamExt};
use network_tools::{DownloadRequest, Downloader, ProgressSink};

pub use bili;
pub use input::{parse_input, Input, PageSelection};
pub use media;
pub use network_tools;
pub use track::{ResolveOptions, Track, TrackKind};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Bili(#[from] bili::Error),
    #[error("download failed: {0}")]
    Download(#[from] network_tools::Error),
    #[error("audio extraction failed: {0}")]
    Media(#[from] media::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Input(String),
    #[error("no audio stream available for this video")]
    NoAudio,
    #[error("task failed: {0}")]
    Task(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Settings for [`Service::plan`] and [`Service::run`].
#[derive(Debug, Clone)]
pub struct DownloadOptions {
    pub out_dir: PathBuf,
    /// Output template; `None` uses the per kind default
    /// ([`TrackKind::default_template`]).
    pub template: Option<String>,
    pub audio: AudioPreference,
    /// Tracks downloaded at the same time.
    pub jobs: usize,
    /// Connections per file.
    pub connections: usize,
    /// Connections in total.
    pub max_connections: usize,
    pub chunk_size: Option<u64>,
    pub retries: u32,
    pub overwrite: bool,
    pub embed_cover: bool,
    /// Use bilibili's recognised music title (BGM) as track title.
    pub use_bgm_title: bool,
}

impl Default for DownloadOptions {
    fn default() -> Self {
        Self {
            out_dir: PathBuf::from("."),
            template: None,
            audio: AudioPreference::default(),
            jobs: 3,
            connections: 4,
            max_connections: 16,
            chunk_size: None,
            retries: 5,
            overwrite: false,
            embed_cover: true,
            use_bgm_title: true,
        }
    }
}

/// A track with its final location (without extension, which depends on
/// the codec: `.m4a` or `.flac`).
#[derive(Debug, Clone)]
pub struct Job {
    pub index: usize,
    pub total: usize,
    pub track: Track,
    pub stem: PathBuf,
}

impl Job {
    pub fn display_name(&self) -> String {
        self.stem
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.track.title.clone())
    }
}

#[derive(Debug, Clone)]
pub enum Outcome {
    Downloaded {
        path: PathBuf,
        quality: String,
        codec: String,
        bytes: u64,
        resumed: u64,
    },
    /// The output already existed.
    Skipped { path: PathBuf },
}

/// Receives events from [`Service::run`]; implemented by front-ends.
pub trait Reporter: Send + Sync {
    /// A job starts; returns the sink for its byte progress.
    fn started(&self, job: &Job) -> Arc<dyn ProgressSink>;
    /// Informational step change ("selected Hi-Res FLAC", "tagging").
    fn status(&self, _job: &Job, _message: &str) {}
    fn finished(&self, job: &Job, result: &Result<Outcome>);
}

#[derive(Debug, Default)]
pub struct Summary {
    pub downloaded: usize,
    pub skipped: usize,
    pub failed: Vec<(Job, String)>,
}

pub struct Service {
    client: Arc<Client>,
}

const OUTPUT_EXTENSIONS: [&str; 2] = ["m4a", "flac"];

fn ext_path(stem: &Path, ext: &str) -> PathBuf {
    media::with_appended_extension(stem, ext)
}

impl Service {
    pub fn new(cookies: CookieJar) -> Result<Self> {
        Ok(Self {
            client: Arc::new(Client::new(cookies)?),
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    /// Login / VIP state.
    pub async fn account(&self) -> Result<NavInfo> {
        Ok(self.client.nav().await?)
    }

    /// Resolves every input. Failing inputs are reported as warnings so one
    /// bad id does not abort a batch. Duplicate tracks are removed.
    pub async fn resolve(
        &self,
        inputs: &[String],
        opts: &ResolveOptions,
    ) -> (Vec<Track>, Vec<String>) {
        let mut tracks: Vec<Track> = Vec::new();
        let mut warnings = Vec::new();
        let mut seen = HashSet::new();
        for raw in inputs {
            let res = match input::parse_input(raw) {
                Ok(i) => track::resolve_input(&self.client, &i, opts, &mut warnings).await,
                Err(e) => Err(Error::Input(e)),
            };
            match res {
                Ok(list) => tracks.extend(
                    list.into_iter()
                        .filter(|t| seen.insert((t.bvid.clone(), t.cid))),
                ),
                Err(e) => warnings.push(format!("{raw}: {e}")),
            }
        }
        (tracks, warnings)
    }

    /// Applies BGM titles, renders file names and makes them unique.
    pub async fn plan(&self, tracks: Vec<Track>, opts: &DownloadOptions) -> Vec<Job> {
        let client = &self.client;
        let use_bgm = opts.use_bgm_title;
        let tracks: Vec<Track> = stream::iter(tracks)
            .map(|mut t| async move {
                if use_bgm && t.bgm_lookup {
                    if let Ok(Some(bgm)) = client.bgm_info(&t.video_id(), t.cid).await {
                        t.title = bgm.music_title.trim().to_owned();
                    }
                }
                t
            })
            .buffered(4)
            .collect()
            .await;

        let total = tracks.len();
        let mut used: HashSet<String> = HashSet::new();
        tracks
            .into_iter()
            .enumerate()
            .map(|(i, track)| {
                let template = opts
                    .template
                    .as_deref()
                    .unwrap_or_else(|| track.kind.default_template());
                let mut rel = naming::render_template(template, &track);
                // Two different tracks must never share an output file.
                let key = |p: &Path| p.to_string_lossy().to_lowercase();
                if !used.insert(key(&rel)) {
                    let name = rel
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    let suffix = if track.page > 1 {
                        format!("{} [{}-p{}]", name, track.bvid, track.page)
                    } else {
                        format!("{} [{}]", name, track.bvid)
                    };
                    rel.set_file_name(naming::sanitize_component(&suffix));
                    used.insert(key(&rel));
                }
                Job {
                    index: i + 1,
                    total,
                    stem: opts.out_dir.join(rel),
                    track,
                }
            })
            .collect()
    }

    /// Audio streams offered for a track, best first.
    pub async fn streams(&self, track: &Track) -> Result<Vec<AudioStream>> {
        Ok(self
            .client
            .playurl(&track.video_id(), track.cid)
            .await?
            .audio_streams())
    }

    /// Runs every job with `opts.jobs` concurrency.
    pub async fn run(
        &self,
        jobs: Vec<Job>,
        opts: &DownloadOptions,
        reporter: Arc<dyn Reporter>,
    ) -> Summary {
        let downloader = Downloader::new(
            self.client.http().clone(),
            network_tools::DownloadOptions {
                connections: opts.connections.max(1),
                chunk_size: opts.chunk_size,
                retries: opts.retries,
                idle_timeout: Duration::from_secs(20),
            },
        )
        .with_global_limit(opts.max_connections.max(opts.connections));

        let results: Vec<(Job, Result<Outcome>)> = stream::iter(jobs)
            .map(|job| {
                let downloader = &downloader;
                let reporter = reporter.clone();
                async move {
                    let progress = reporter.started(&job);
                    let res = self
                        .download_job(&job, opts, downloader, progress, reporter.as_ref())
                        .await;
                    reporter.finished(&job, &res);
                    (job, res)
                }
            })
            .buffer_unordered(opts.jobs.max(1))
            .collect()
            .await;

        let mut summary = Summary::default();
        for (job, res) in results {
            match res {
                Ok(Outcome::Downloaded { .. }) => summary.downloaded += 1,
                Ok(Outcome::Skipped { .. }) => summary.skipped += 1,
                Err(e) => summary.failed.push((job, e.to_string())),
            }
        }
        summary.failed.sort_by_key(|(j, _)| j.index);
        summary
    }

    async fn download_job(
        &self,
        job: &Job,
        opts: &DownloadOptions,
        downloader: &Downloader,
        progress: Arc<dyn ProgressSink>,
        reporter: &dyn Reporter,
    ) -> Result<Outcome> {
        let track = &job.track;
        if !opts.overwrite {
            for ext in OUTPUT_EXTENSIONS {
                let p = ext_path(&job.stem, ext);
                if p.exists() {
                    return Ok(Outcome::Skipped { path: p });
                }
            }
        }

        // 1. pick the stream
        let streams = self.streams(track).await?;
        let stream = opts.audio.select(&streams).ok_or(Error::NoAudio)?.clone();
        reporter.status(job, &stream.label());

        // 2. download (resumable) + cover, concurrently
        let part = ext_path(&job.stem, "part");
        let request = DownloadRequest {
            urls: stream.urls.clone(),
            dest: part.clone(),
            resume_key: format!(
                "{}:{}:{}:{:?}",
                track.bvid, track.cid, stream.id, stream.source
            ),
            headers: Client::browser_headers(),
        };
        let cover = async {
            if !opts.embed_cover || track.cover_url.is_empty() {
                return None;
            }
            self.client.fetch_bytes(&track.cover_url).await.ok()
        };
        let (report, cover) = tokio::join!(downloader.download(&request, progress), cover);
        let report = report?;

        // 3. remux to audio only + tags
        reporter.status(job, "writing tags");
        let meta = metadata(track, &stream, cover);
        let stem = job.stem.clone();
        let part2 = part.clone();
        let extracted = tokio::task::spawn_blocking(move || -> Result<media::ExtractedFile> {
            if let Some(dir) = stem.parent() {
                std::fs::create_dir_all(dir)?;
            }
            Ok(media::extract_audio_file(&part2, &stem, &meta)?)
        })
        .await
        .map_err(|e| Error::Task(e.to_string()))??;
        let path = extracted.path;

        // Another extension of the same track may exist when overwriting
        // with a different quality (e.g. m4a → flac).
        if opts.overwrite {
            for ext in OUTPUT_EXTENSIONS {
                let other = ext_path(&job.stem, ext);
                if other != path {
                    let _ = std::fs::remove_file(other);
                }
            }
        }
        let _ = std::fs::remove_file(&part);
        let _ = std::fs::remove_file(network_tools::state_path(&part));

        Ok(Outcome::Downloaded {
            path,
            quality: stream.label(),
            codec: extracted.codec.to_string(),
            bytes: report.total,
            resumed: report.resumed,
        })
    }
}

fn non_empty(s: &str) -> Option<String> {
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// Builds the tags written into the audio file.
pub fn metadata(track: &Track, stream: &AudioStream, cover: Option<Vec<u8>>) -> media::Metadata {
    let mut custom = vec![
        ("BVID".to_owned(), track.bvid.clone()),
        ("BILIBILI_CID".to_owned(), track.cid.to_string()),
        ("SOURCE_URL".to_owned(), track.url()),
        ("AUDIO_QUALITY".to_owned(), stream.label()),
    ];
    if track.video_title != track.title {
        custom.push(("VIDEO_TITLE".to_owned(), track.video_title.clone()));
    }
    media::Metadata {
        title: non_empty(&track.title),
        artist: non_empty(&track.artist),
        album: non_empty(&track.album),
        album_artist: non_empty(&track.album_artist),
        date: naming::format_date(track.pubdate),
        genre: non_empty(&track.genre),
        comment: Some(track.url()),
        description: non_empty(&track.description),
        track: track.index.map(|i| (i, track.total)),
        cover: cover
            .filter(|c| media::ImageFormat::detect(c).is_some())
            .map(media::Picture::new),
        custom,
        tool: Some(concat!("dc ", env!("CARGO_PKG_VERSION")).to_owned()),
    }
}
