#![allow(dead_code)]
pub mod consts;
mod error;
mod models;
pub mod prelude;

pub use error::*;
pub use models::*;
use prelude::VideoService;

use futures_util::StreamExt;
pub struct Service<'a> {
    api_host: &'a str,
    protocol: Protocol,
    client: reqwest::Client,
}

impl<'a> Service<'a> {
    pub fn new() -> Self {
        Self {
            api_host: consts::HOST,
            protocol: Protocol::HTTPS,
            client: reqwest::Client::new(),
        }
    }
}

pub trait DebugPrint {
    fn debug_print(self) -> Self;
}
impl<T> DebugPrint for T
where
    T: std::fmt::Debug,
{
    fn debug_print(self) -> Self {
        println!("{:?}", self);
        self
    }
}

async fn do_download(
    client: &reqwest::Client,
    url: String,
    bound: (u64, u64),
    tx: tokio::sync::mpsc::Sender<(u64, reqwest::Response)>,
) -> Result<()> {
    let resp = client
        .get(url)
        .header("Referer", "https://www.bilibili.com")
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36 Edg/122.0.0.0")
        .send()
        .await?;
    tx.send((bound.0, resp))
        .await
        .map_err(|e| Error::ChannelError(e.to_string()))
}

impl<'a> VideoService for &Service<'a> {
    // GET /x/player/pagelist
    async fn get_basic_info(self, id: &VideoId) -> Result<VideoMetadata> {
        let url = format!(
            "{}{}/x/player/pagelist",
            self.protocol.get_prefix(),
            self.api_host
        );
        let query = match id {
            VideoId::AID(aid) => [("aid", aid.to_string())],
            VideoId::BVID(bvid) => [("bvid", bvid.clone())],
        };
        let res = self
            .client
            .get(url)
            .query(&query)
            .send()
            .await?
            .json::<PackInfo<Vec<VideoMetadata>>>()
            .await?
            .as_result()?;
        res.into_iter().nth(0).ok_or(Error::UnexpectedResp)
    }

    // GET /x/player/playurl
    async fn get_download_info(self, param: &GetDownloadInfoParam) -> Result<DownloadInfo> {
        let url = format!(
            "{}{}/x/player/playurl",
            self.protocol.get_prefix(),
            self.api_host
        );
        self.client
            .get(url)
            .query(&param.get_query())
            .send()
            .await?
            .json::<PackInfo<DownloadInfo>>()
            .await?
            .as_result()
    }

    async fn download<'b, W>(self, param: &DownloadParam, writer: &mut W) -> Result<()>
    where
        W: tokio::io::AsyncWriteExt + tokio::io::AsyncSeekExt + Send + Sync + Unpin,
    {
        let durl_info = param.info().durl().first().ok_or(Error::UnexpectedResp)?;
        let size = *durl_info.size();
        
        // --- prepare chan
        let pool_size = param.conn_pool().unwrap_or(1);
        let (tx, mut rx) =
            tokio::sync::mpsc::channel::<(u64, reqwest::Response)>(pool_size as usize);
        let chunk_size = param.chunk_size().unwrap_or(*durl_info.size());
        let mut start = 0;
        let mut fg = tokio::task::JoinSet::new();

        // --- start chunk download
        while start < size {
            let end = (start + chunk_size).min(size-1);
            let url = durl_info.url().clone();
            let txc = tx.clone();
            let client = self.client.clone();
            fg.spawn(async move { do_download(&client, url, (start, end), txc).await });
            start += chunk_size;
        }

        // --- stop
        let stop = tokio::spawn(async move {
            while let Some(f) = fg.join_next().await {
                f.map_err(|e| Error::FutureErr(e.to_string()))??;
            }
            drop(tx);
            Ok::<(), Error>(())
        });

        // --- block recv
        while let Some((start, resp)) = rx.recv().await {
            let _seek = writer.seek(std::io::SeekFrom::Start(start)).await?;
            let mut stream = resp.bytes_stream();
            while let Some(bt) = stream.next().await {
                tokio::io::copy(&mut bt?.as_ref(), writer).await?;
            }
        }
        writer.flush().await?;

        stop.await.map_err(|e| Error::FutureErr(e.to_string()))??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_download() -> anyhow::Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(anyhow_downolad())?;
        Ok(())
    }

    async fn anyhow_downolad() -> anyhow::Result<()> {
        let s = Service::new();
        let basic_info = s
            .get_basic_info(&VideoId::BVID("BV1qJ4m1Y71G".to_owned()))
            .await?;
        tokio::fs::create_dir_all("tests_download").await?;
        let mut file =
            tokio::fs::File::create(format!("tests_download/{}.mp4", basic_info.title())).await?;
        let download_info = s
            .get_download_info(&GetDownloadInfoParam {
                id: VideoId::BVID("BV1qJ4m1Y71G".to_owned()),
                cid: *basic_info.cid(),
                clarity: Clarity::Low,
            })
            .await?;
        s.download(
            &DownloadParam {
                info: download_info,
                chunk_size: Some(4 * 1024 * 1024),
                conn_pool: Some(8),
            },
            &mut file,
        )
        .await?;
        Ok(())
    }
}
