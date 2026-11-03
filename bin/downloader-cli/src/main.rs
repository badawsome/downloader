mod qr;
mod session;
mod ui;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context};
use bili_service::bili::{AudioPreference, AudioQuality, QrStatus};
use bili_service::{DownloadOptions, PageSelection, ResolveOptions, Service};
use clap::{Args, Parser, Subcommand};
use console::style;
use futures_util::{stream, StreamExt};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Cookie string (`SESSDATA=...; bili_jct=...`) instead of the saved login
    #[arg(long, global = true, env = "DC_COOKIE", hide_env_values = true)]
    cookie: Option<String>,
    /// Read cookies from a file (cookie header or Netscape cookies.txt)
    #[arg(long, global = true)]
    cookie_file: Option<PathBuf>,
    /// Show retries and other details
    #[arg(short, long, global = true)]
    verbose: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Download audio. Accepts BV/av ids, video urls, b23.tv short links,
    /// favorites (`fav:<id>`, `ml<id>`, favlist urls) and collection urls
    Get {
        #[arg(required = true)]
        inputs: Vec<String>,
        #[command(flatten)]
        opts: GetOpts,
    },
    /// List tracks, file names and available audio qualities without downloading
    Info {
        #[arg(required = true)]
        inputs: Vec<String>,
        #[command(flatten)]
        opts: GetOpts,
    },
    /// Log in by scanning a QR code with the bilibili app (or save `--cookie`)
    Login,
    /// Forget the saved login
    Logout,
    /// Show the login / VIP state and the best reachable audio quality
    Status,
    /// download bilibili with {av} num
    #[command(hide = true)]
    AV {
        aid: Vec<u64>,
        #[command(flatten)]
        opts: GetOpts,
    },
    /// download bilibili with {bv} num
    #[command(hide = true)]
    BV {
        bvid: Vec<String>,
        #[command(flatten)]
        opts: GetOpts,
    },
    /// download season with any {av}/{bv}
    #[command(hide = true)]
    Season {
        id: Vec<String>,
        #[command(flatten)]
        opts: GetOpts,
    },
}

#[derive(Args, Debug, Clone)]
struct GetOpts {
    /// Output directory
    #[arg(short, long, default_value = ".")]
    out_dir: PathBuf,
    /// File name template (without extension); `/` creates folders.
    /// Variables: {title} {artist} {album} {collection} {video_title} {part}
    /// {index} {total} {page} {bvid} {aid} {cid} {date} {year}
    #[arg(short = 'n', long)]
    name: Option<String>,
    /// Highest audio quality to use: best, hires, dolby, 192k, 132k, 64k
    #[arg(short, long, default_value = "best")]
    quality: String,
    /// Never pick Dolby Atmos (E-AC-3) streams
    #[arg(long)]
    no_dolby: bool,
    /// Download the whole collection (合集) each video belongs to
    #[arg(short, long)]
    season: bool,
    /// Pages of multi-part videos, e.g. `1,3-5` (default: all)
    #[arg(short, long)]
    pages: Option<PageSelection>,
    /// Only these entries of the resolved track list (e.g. `1-10,15` of a
    /// collection or favorites folder)
    #[arg(long)]
    items: Option<PageSelection>,
    /// Tracks downloaded in parallel
    #[arg(short, long, default_value_t = 3)]
    jobs: usize,
    /// Parallel connections (chunks) per file
    #[arg(short, long, default_value_t = 4)]
    connections: usize,
    /// Upper limit of connections across all files
    #[arg(long, default_value_t = 16)]
    max_connections: usize,
    /// Chunk size in KiB (default: automatic)
    #[arg(long)]
    chunk_size: Option<u64>,
    /// Retries per chunk
    #[arg(long, default_value_t = 5)]
    retries: u32,
    /// Replace existing files
    #[arg(long)]
    overwrite: bool,
    /// Do not embed the cover image
    #[arg(long)]
    no_cover: bool,
    /// Keep the video title instead of the recognised music (BGM) title
    #[arg(long)]
    no_bgm_title: bool,
}

impl GetOpts {
    fn resolve_options(&self) -> ResolveOptions {
        ResolveOptions {
            expand_season: self.season,
            pages: self.pages.clone(),
        }
    }

    fn download_options(&self) -> anyhow::Result<DownloadOptions> {
        let max_quality = match self.quality.trim().to_ascii_lowercase().as_str() {
            "best" | "max" | "" => None,
            q => Some(q.parse::<AudioQuality>().map_err(anyhow::Error::msg)?),
        };
        Ok(DownloadOptions {
            out_dir: self.out_dir.clone(),
            template: self.name.clone(),
            audio: AudioPreference {
                max_quality,
                allow_dolby: !self.no_dolby,
                allow_hires: true,
            },
            jobs: self.jobs.clamp(1, 32),
            connections: self.connections.clamp(1, 32),
            max_connections: self.max_connections.clamp(1, 128),
            chunk_size: self.chunk_size.map(|k| k.max(64) * 1024),
            retries: self.retries,
            overwrite: self.overwrite,
            embed_cover: !self.no_cover,
            use_bgm_title: !self.no_bgm_title,
        })
    }
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    match run(cli).await {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("error: {e:#}");
            std::process::exit(1);
        }
    }
}

async fn run(cli: Cli) -> anyhow::Result<i32> {
    let cookies = session::load(cli.cookie.as_deref(), cli.cookie_file.as_ref())?;
    let service = Service::new(cookies)?;
    match cli.command {
        Commands::Get { inputs, opts } => download(&service, inputs, opts, cli.verbose).await,
        Commands::AV { aid, opts } => {
            let inputs = aid.iter().map(|a| format!("av{a}")).collect();
            download(&service, inputs, opts, cli.verbose).await
        }
        Commands::BV { bvid, opts } => download(&service, bvid, opts, cli.verbose).await,
        Commands::Season { id, mut opts } => {
            opts.season = true;
            download(&service, id, opts, cli.verbose).await
        }
        Commands::Info { inputs, opts } => info(&service, inputs, opts).await,
        Commands::Login => login(&service, cli.cookie.is_some()).await,
        Commands::Logout => {
            if session::remove()? {
                println!("saved login removed");
            } else {
                println!("not logged in");
            }
            Ok(0)
        }
        Commands::Status => status(&service).await,
    }
}

async fn account_line(service: &Service) -> String {
    match service.account().await {
        Ok(nav) if nav.is_login => format!(
            "logged in as {}{} — up to {}",
            nav.uname,
            if nav.is_vip() { " (VIP)" } else { "" },
            if nav.is_vip() {
                "Hi-Res / Dolby Atmos"
            } else {
                "192K AAC (Hi-Res / Dolby are usually 大会员 only)"
            }
        ),
        Ok(_) if service.client().has_session() => {
            "the saved session has expired — run `dc login` again; using anonymous access (up to 192K AAC)".into()
        }
        Ok(_) => "not logged in — up to 192K AAC; `dc login` unlocks Hi-Res / Dolby Atmos for VIP accounts".into(),
        Err(e) => format!("could not check login state: {e}"),
    }
}

async fn status(service: &Service) -> anyhow::Result<i32> {
    println!("{}", account_line(service).await);
    println!("session file: {}", session::cookie_file()?.display());
    Ok(0)
}

async fn login(service: &Service, from_cookie: bool) -> anyhow::Result<i32> {
    if from_cookie {
        let nav = service.account().await?;
        if !nav.is_login {
            bail!("the given cookie is not logged in (SESSDATA missing or expired)");
        }
        let path = session::save(&service.client().cookies())?;
        println!(
            "logged in as {}; session saved to {}",
            nav.uname,
            path.display()
        );
        return Ok(0);
    }

    let code = service.client().qr_generate().await?;
    println!("Scan with the bilibili app (我的 → 扫一扫):\n");
    match qr::render(&code.url) {
        Some(art) => println!("{art}"),
        None => println!("{}", code.url),
    }
    println!("or open: {}\n", code.url);
    let mut scanned = false;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(180);
    while tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_secs(2)).await;
        match service.client().qr_poll(&code.qrcode_key).await? {
            QrStatus::Waiting => {}
            QrStatus::Scanned => {
                if !scanned {
                    println!("scanned, please confirm the login in the app…");
                    scanned = true;
                }
            }
            QrStatus::Expired => bail!("QR code expired, run `dc login` again"),
            QrStatus::Success(jar) => {
                let path = session::save(&jar)?;
                println!("{}", account_line(service).await);
                println!("session saved to {}", path.display());
                return Ok(0);
            }
        }
    }
    bail!("login timed out")
}

async fn resolve(
    service: &Service,
    inputs: &[String],
    opts: &GetOpts,
) -> anyhow::Result<Vec<bili_service::Job>> {
    let download_opts = opts.download_options()?;
    let spinner = ui::phase_spinner("resolving");
    let n = inputs.len();
    let (mut tracks, warnings) = service
        .resolve_with_progress(inputs, &opts.resolve_options(), |i, raw| {
            let what = if n > 1 {
                format!(" ({}/{n})", i + 1)
            } else {
                String::new()
            };
            spinner.set_message(format!("resolving{what} {raw}"));
        })
        .await;
    spinner.finish_and_clear();
    if let Some(items) = &opts.items {
        tracks = tracks
            .into_iter()
            .enumerate()
            .filter(|(i, _)| items.contains(*i as u32 + 1))
            .map(|(_, t)| t)
            .collect();
    }
    for w in &warnings {
        eprintln!("{} {w}", style("warning:").yellow().bold().for_stderr());
    }
    if tracks.is_empty() {
        bail!("nothing to download");
    }
    let spinner = ui::phase_spinner(format!("planning {} tracks", tracks.len()));
    let jobs = service
        .plan_with_progress(tracks, &download_opts, |done, total| {
            spinner.set_message(format!("looking up song titles {done}/{total}"));
        })
        .await;
    spinner.finish_and_clear();
    Ok(jobs)
}

async fn download(
    service: &Service,
    inputs: Vec<String>,
    opts: GetOpts,
    verbose: bool,
) -> anyhow::Result<i32> {
    let download_opts = opts.download_options()?;
    eprintln!("{}", style(account_line(service).await).dim().for_stderr());
    let jobs = resolve(service, &inputs, &opts).await?;
    std::fs::create_dir_all(&download_opts.out_dir)
        .with_context(|| format!("cannot create {}", download_opts.out_dir.display()))?;

    let longest_name = jobs
        .iter()
        .map(|j| console::measure_text_width(&j.display_name()))
        .max()
        .unwrap_or(0);
    let reporter = Arc::new(ui::TerminalReporter::new(
        jobs.len(),
        longest_name,
        &download_opts.out_dir,
        verbose,
    ));
    let summary = tokio::select! {
        s = service.run(jobs, &download_opts, reporter.clone()) => s,
        _ = tokio::signal::ctrl_c() => {
            reporter.finish();
            eprintln!(
                "{} finished chunks are saved, run the same command again to resume",
                style("interrupted —").yellow().bold().for_stderr()
            );
            return Ok(130);
        }
    };
    reporter.finish();
    eprintln!("{}", reporter.summary_line(&summary));
    for (job, err) in &summary.failed {
        eprintln!(
            "  {} {} {}: {err}",
            style("✘").red().for_stderr(),
            job.display_name(),
            style(job.track.url()).dim().for_stderr()
        );
    }
    Ok(if summary.failed.is_empty() { 0 } else { 1 })
}

async fn info(service: &Service, inputs: Vec<String>, opts: GetOpts) -> anyhow::Result<i32> {
    let download_opts = opts.download_options()?;
    println!("{}", account_line(service).await);
    let logged_in = service.client().is_logged_in();
    let jobs = resolve(service, &inputs, &opts).await?;
    let results: Vec<_> = stream::iter(jobs.iter())
        .map(|job| async move {
            let t = &job.track;
            (job, service.client().playurl(&t.video_id(), t.cid).await)
        })
        .buffered(4)
        .collect()
        .await;
    for (job, streams) in results {
        let t = &job.track;
        println!(
            "\n[{}/{}] {}\n    title: {} | artist: {} | album: {}{}\n    source: {}",
            job.index,
            job.total,
            job.stem.display(),
            t.title,
            t.artist,
            t.album,
            t.index
                .map(|i| format!(" | track {i}/{}", t.total.unwrap_or(0)))
                .unwrap_or_default(),
            t.url()
        );
        match streams {
            Ok(play) => {
                let streams = play.audio_streams();
                let chosen = download_opts.audio.select(&streams).map(|s| s.id);
                for s in &streams {
                    println!(
                        "    {} {:<42} {:>5} kbps  {}",
                        if Some(s.id) == chosen { "*" } else { " " },
                        s.label(),
                        s.bandwidth / 1000,
                        s.codecs
                    );
                }
                for q in play.locked_qualities() {
                    println!(
                        "    {} {:<42} {}",
                        style("×").yellow(),
                        q.label(),
                        style(if logged_in {
                            "this video has it, but bilibili did not provide it to this account (usually 大会员 only)"
                        } else {
                            "this video has it; not provided without login — `dc login` (usually 大会员 only)"
                        })
                        .dim()
                    );
                }
            }
            Err(e) => println!("    cannot load streams: {e}"),
        }
    }
    Ok(0)
}
