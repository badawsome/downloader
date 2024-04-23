use super::*;

impl<'a> facade::VideoService for &Service<'a> {
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

  }

#[cfg(test)]
mod tests {
    use crate::facade::*;
    use super::*;

    #[test]
    fn test_get_basic_info() -> anyhow::Result<()> {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(anyhow_get_basic_info())?;
        Ok(())
    }

    async fn anyhow_get_basic_info() -> anyhow::Result<()> {
        let s = Service::new();
        let basic_info = s
            .get_basic_info(&VideoId::BVID("BV1qJ4m1Y71G".to_owned()))
            .await?;
        // tokio::fs::create_dir_all("tests_download").await?;
        // let mut file =
        //     tokio::fs::File::create(format!("tests_download/{}.mp4", basic_info.title())).await?;
        // let download_info = s
        //     .get_download_info(&GetDownloadInfoParam {
        //         id: VideoId::BVID("BV1qJ4m1Y71G".to_owned()),
        //         cid: *basic_info.cid(),
        //         clarity: Clarity::Low,
        //     })
        //     .await?;
        // s.download(
        //     &DownloadParam {
        //         info: download_info,
        //         chunk_size: Some(4 * 1024 * 1024),
        //         conn_pool: Some(8),
        //     },
        //     &mut file,
        // )
        // .await?;
        Ok(())
    }
}
