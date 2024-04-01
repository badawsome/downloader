use clap::{Args, Parser, Subcommand, ValueEnum};

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
}

use bili::{prelude::*, *};

fn main() {
    let cli = Cli::parse();
    let rt = tokio::runtime::Runtime::new().expect("init tokio failed");
    rt.block_on(async move { anyhow_downolad(cli).await })
        .expect("dc failed!");
}

async fn anyhow_downolad(cli: Cli) -> anyhow::Result<()> {
    let s = std::sync::Arc::new(Service::new());
    let ids: Vec<_> = match cli.command {
        Commands::AV { aid } => aid.iter().map(|id| VideoId::AID(*id)).collect(),
        Commands::BV { bvid } => bvid.iter().map(|id| VideoId::BVID(id.clone())).collect(),
    };

    let mut fg = tokio::task::JoinSet::new();
    for id in ids {
        let s = s.clone();
        fg.spawn(async move {
            let basic_info = s.get_basic_info(&id).await?;
            tokio::fs::create_dir_all("tests_download").await?;
            let mut file = tokio::fs::File::create(format!(
                "tests_download/{}-{}.mp4",
                basic_info.title(),
                id
            ))
            .await?;
            let download_info = s
                .get_download_info(&GetDownloadInfoParam {
                    id: id.clone(),
                    cid: *basic_info.cid(),
                    clarity: Clarity::Low,
                })
                .await?;
            s.download(
                &DownloadParam {
                    info: download_info,
                    chunk_size: Some(CHUNK_SIZE),
                    conn_pool: Some(CONN_POOL_SIZE),
                },
                &mut file,
            )
            .await?;
            println!("download finish for: {}", id);
            Ok::<(), bili::Error>(())
        });
    }
    while let Some(f) = fg.join_next().await {
        f??;
    }
    Ok(())
}
