#![allow(dead_code)]
pub mod consts;
pub mod prelude;

mod error;
mod models;

pub use error::*;
pub use models::*;

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

impl<'a> prelude::VideoService for &Service<'a> {
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
    async fn get_download_info(self, param: &GetDownloadInfoParam) -> Result<DurlInfo> {
        let url = format!(
            "{}{}/x/player/playurl",
            self.protocol.get_prefix(),
            self.api_host
        );
        let res = self
            .client
            .get(url)
            .query(&param.get_query())
            .send()
            .await?
            .json::<PackInfo<DownloadInfo>>()
            .await?
            .as_result()?;
        match res.durl.len() {
            1 => Ok(res.durl[0].clone()),
            _ => Err(Error::UnexpectedResp),
        }
    }

    async fn download<'b, W>(self, param: &DownloadParam, mut writer: W) -> Result<()>
    where
        W: tokio::io::AsyncWriteExt + tokio::io::AsyncSeekExt + Send + Sync + Unpin,
    {
        use futures::TryStreamExt;

        let durl_info = param.info();
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
            let end = (start + chunk_size).min(size - 1);
            let url = durl_info.url().clone();
            let txc = tx.clone();
            let client = self.client.clone();
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
            let stream = resp.bytes_stream().map_err(std::io::Error::other);
            let mut stream = tokio_util::io::StreamReader::new(stream);
            tokio::io::copy(&mut stream, &mut writer).await?;
        }
        writer.flush().await?;

        stop.await.map_err(|e| Error::FutureErr(e.to_string()))??;
        Ok(())
    }
}

impl<'a> prelude::SeasonService for &Service<'a> {
    // GET /x/web-interface/view/detail
    async fn get_video_relation_season_list(
        self,
        id: &VideoId,
        _season_id: u64,
    ) -> Result<SeasonList> {
        use serde::Deserialize;
        #[derive(Debug, Clone, Deserialize)]
        struct Detail {
            #[serde(rename = "View")]
            main_view: MainView,
            #[serde(rename = "Related")]
            related: Vec<View>,
        }

        #[derive(Debug, Clone, Deserialize)]
        struct MainView {
            aid: u64,
            bvid: String,
            title: String,
            owner: Owner,
            #[serde(rename = "pic")]
            pic_url: String,
            season_id: u64,
            #[serde(rename = "ugc_season")]
            season_list: InternalSeasonList,
        }

        #[derive(Debug, Clone, Deserialize)]
        struct InternalSeasonList {
            #[serde(rename = "id")]
            season_id: u64,
            #[serde(rename = "title")]
            season_name: String,
            #[serde(rename = "sections")]
            season_sections: Vec<SeasonSections>, // only 1 ??? fuck
        }

        #[derive(Debug, Clone, Deserialize)]
        struct SeasonSections {
            #[serde(rename = "episodes")]
            sections: Vec<BasicView>,
        }

        let url = format!(
            "{}{}/x/web-interface/view/detail",
            self.protocol.get_prefix(),
            self.api_host
        );
        let query = match id {
            VideoId::AID(aid) => [("aid", aid.to_string())],
            VideoId::BVID(bvid) => [("bvid", bvid.clone())],
        };
        let detail = self
            .client
            .get(url)
            .query(&query)
            .send()
            .await?
            .json::<PackInfo<Detail>>()
            .await?
            .as_result()?;
        let season_name = detail.main_view.season_list.season_name;
        let season_id = detail.main_view.season_id;
        let owner = detail.main_view.owner;
        let sections = detail
            .main_view
            .season_list
            .season_sections
            .first()
            .ok_or(Error::UnexpectedResp)?
            .to_owned()
            .sections;

        Ok(SeasonListBuilder::default()
            .season_id(season_id)
            .season_name(season_name)
            .owner(owner)
            .sections(sections)
            .build()
            .expect("build struct failed"))
    }

    // GET /x/web-interface/view
    async fn season_id(self, id: &VideoId) -> Result<Option<u64>> {
        let url = format!(
            "{}{}/x/web-interface/view",
            self.protocol.get_prefix(),
            self.api_host
        );
        let query = match id {
            VideoId::AID(aid) => [("aid", aid.to_string())],
            VideoId::BVID(bvid) => [("bvid", bvid.clone())],
        };
        let view = self
            .client
            .get(url)
            .query(&query)
            .send()
            .await?
            .json::<PackInfo<View>>()
            .await?
            .as_result()?;

        if view.is_season_display().is_some_and(|x| x) {
            return Ok(view.season_id().to_owned());
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::prelude::*;
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

    #[tokio::test]
    async fn test_season_list() -> anyhow::Result<()> {
        let (a, b) = tokio::join!(
            anyhow_season_list_contains(),
            anyhow_season_list_not_contains()
        );
        a?;
        b?;
        Ok(())
    }

    async fn anyhow_season_list_contains() -> anyhow::Result<()> {
        let s = Service::new();
        let id = VideoId::BVID("BV13m421J7fM".to_owned());
        let season_id = s
            .season_id(&id)
            .await?
            .ok_or(anyhow::anyhow!("not season"))?;
        let list = s.get_video_relation_season_list(&id, season_id).await?;
        assert!(list.sections().len() > 0);
        Ok(())
    }

    async fn anyhow_season_list_not_contains() -> anyhow::Result<()> {
        let s = Service::new();
        let id = VideoId::BVID("BV1nr421t7KX".to_owned());
        assert!(s.season_id(&id).await?.is_none());
        Ok(())
    }
}
