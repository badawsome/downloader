use anyhow::anyhow;
use clap::{Parser, Subcommand};

const CHUNK_SIZE: u64 = 4 * 1024 * 1024;
const CONN_POOL_SIZE: u8 = 8;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    debug: bool,
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// download bilibili with {av} num
    AV { aid: Vec<u64> },
    /// download bilibili with {bv} num
    BV { bvid: Vec<String> },
    /// download season with any {av}/{bv}
    Season {
        /// autodetected {av} or {bv}
        id: Vec<String>,
    },
}

use bili::{prelude::*, *};
use tokio::task::JoinSet;

fn main() {
    let cli = Cli::parse();
    let rt = tokio::runtime::Runtime::new().expect("init tokio failed");
    rt.block_on(async move { anyhow_downolad(cli).await })
        .expect("dc failed!");
}

async fn anyhow_downolad(cli: Cli) -> anyhow::Result<()> {
    let s = std::sync::Arc::new(Service::new());
    match cli.command {
        Commands::AV { aid } => {
            downloads(s, aid.iter().map(|id| VideoId::AID(*id)).collect()).await
        }
        Commands::BV { bvid } => {
            downloads(s, bvid.iter().map(|id| VideoId::BVID(id.clone())).collect()).await
        }
        Commands::Season { id } => download_season(s, id).await,
    }
}

async fn download_season(
    s: std::sync::Arc<Service<'static>>,
    ids: Vec<String>,
) -> anyhow::Result<()> {
    let ids = ids
        .iter()
        .map(|id| match id.parse::<u64>() {
            Ok(id) => VideoId::AID(id),
            Err(_) => VideoId::BVID(id.to_owned()),
        })
        .collect::<Vec<_>>();
    let mut fg: JoinSet<anyhow::Result<()>> = tokio::task::JoinSet::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
    let p = indicatif::MultiProgress::new();
    for id in ids {
        let s = s.clone();
        let res = s.season_id(&id).await?.ok_or(anyhow!("this not season"));
        match res {
            Ok(season_id) => {
                let season_list = s.get_video_relation_season_list(&id, season_id).await;
                match season_list {
                    Ok(season_list) => {
                        // step1: create folder
                        let folder_path = season_list.season_name().trim();
                        tokio::fs::create_dir_all(folder_path).await.map_err(|e| {
                            anyhow::anyhow!("create folder failed: {}", e.to_string())
                        })?;

                        for section in season_list.sections().into_iter() {
                            // step2: create file
                            let file_path: std::path::PathBuf = [
                                "./",
                                folder_path,
                                format!(
                                    "{}-{}.mp4",
                                    normalization_file_name(section.title().to_owned()),
                                    section.bvid()
                                )
                                .as_str(),
                            ]
                            .iter()
                            .collect();
                            let mut f = tokio::fs::OpenOptions::new()
                                .create(true)
                                .truncate(false)
                                .write(true)
                                .open(&file_path)
                                .await
                                .map_err(|e| {
                                    anyhow::anyhow!(
                                        "create file in {} failed: {}",
                                        file_path.as_path().to_str().unwrap(),
                                        e.to_string()
                                    )
                                })?;

                            // step3: start download
                            let permit = std::sync::Arc::clone(&sem).acquire_owned().await?;
                            let s = s.clone();
                            let section = section.clone();
                            let p = p.clone();
                            fg.spawn(async move {
                                let _permit = permit;
                                download_writer(
                                    s,
                                    &mut f,
                                    VideoId::BVID(section.bvid().to_owned()),
                                    *section.cid(),
                                    p,
                                )
                                .await?;
                                Ok(())
                            });
                        }
                    }
                    Err(err) => println!("Get season list for id: {} failed: {}", id, err,),
                };
            }
            Err(err) => println!("Err: {}", err.to_string()),
        };
    }
    while let Some(f) = fg.join_next().await {
        f??;
    }
    Ok(())
}

async fn downloads(s: std::sync::Arc<Service<'static>>, ids: Vec<VideoId>) -> anyhow::Result<()> {
    let mut fg: JoinSet<anyhow::Result<()>> = tokio::task::JoinSet::new();
    let p = indicatif::MultiProgress::new();
    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(3));
    for id in ids {
        let s = s.clone();
        let p = p.clone();
        let permit = std::sync::Arc::clone(&sem).acquire_owned().await?;
        fg.spawn(async move {
            let _permit = permit;
            let basic_info = s.get_basic_info(&id).await?;
            tokio::fs::create_dir_all("tests_download").await?;
            let mut file = tokio::fs::File::create(format!(
                "{}-{}.mp4",
                normalization_file_name(basic_info.title().to_owned()),
                id
            ))
            .await?;
            download_writer(s, &mut file, id, *basic_info.cid(), p).await?;
            Ok(())
        });
    }
    while let Some(f) = fg.join_next().await {
        f??;
    }
    Ok(())
}

async fn download_writer(
    s: std::sync::Arc<Service<'static>>,
    f: &mut tokio::fs::File,
    id: VideoId,
    cid: u64,
    p: indicatif::MultiProgress,
) -> anyhow::Result<()> {
    let sty = indicatif::ProgressStyle::with_template(
        "[{elapsed_precise}] {bar:40.cyan/blue} {pos:>7}/{len:7} {msg}",
    )
    .unwrap()
    .progress_chars("##-");
    let download_info = s
        .get_download_info(&GetDownloadInfoParam {
            id: id.clone(),
            cid,
            clarity: Clarity::Low,
        })
        .await?;
    let pb = p.add(indicatif::ProgressBar::new(*download_info.size()).with_message(id.to_string()));
    pb.set_style(sty);
    pb.inc(1);
    s.download(
        &DownloadParam {
            info: download_info,
            chunk_size: Some(CHUNK_SIZE),
            conn_pool: Some(CONN_POOL_SIZE),
        },
        f,
    )
    .await?;
    pb.finish();
    Ok(())
}

fn normalization_file_name(s: String) -> String {
    let s: Vec<u8> = s
        .trim()
        .bytes()
        .filter(|x| *x != b'/' && *x != b'\\')
        .collect();
    String::from_utf8_lossy(&s).into_owned()
}
