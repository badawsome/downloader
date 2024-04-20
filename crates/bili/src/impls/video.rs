use super::*;

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
        let (tx, mut rx) = tokio::sync::mpsc::channel(pool_size as usize);
        let chunk_size = param.chunk_size().unwrap_or(*durl_info.size());
        let mut start = 0;
        let mut fg = tokio::task::JoinSet::new();

        // --- start chunk download
        while start < size {
            let end = (start + chunk_size).min(size - 1);
            let url = durl_info.url().clone();
            let txc = tx.clone();
            let client = self.client.clone();
            async fn do_download<Range: std::ops::RangeBounds<u64>>(
                client: &reqwest::Client,
                url: String,
                range: Range,
                tx: tokio::sync::mpsc::Sender<(Range, reqwest::Response)>,
            ) -> Result<()> {
                let resp = client
                    .get(url)
                    .header("Referer", "https://www.bilibili.com")
                    .header("Range", {
                        let mut s = String::from("bytes=");
                        let start = match range.start_bound() {
                            std::ops::Bound::Included(&x) => x,
                            std::ops::Bound::Excluded(&x) => x + 1,
                            std::ops::Bound::Unbounded => 0,
                        };
                        s.push_str(&start.to_string());
                        s.push('-');
                        match range.end_bound() {
                            std::ops::Bound::Included(&x) => s.push_str(&x.to_string()),
                            std::ops::Bound::Excluded(&x) => s.push_str(&(x - 1).to_string()),
                            std::ops::Bound::Unbounded => {},
                        };
                        s
                    })
                    .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/122.0.0.0 Safari/537.36 Edg/122.0.0.0")
                    .send()
                    .await?;
                tx.send((range, resp))
                    .await
                    .map_err(|e| Error::ChannelError(e.to_string()))
            }
            fg.spawn(async move { do_download(&client, url, start..=end, txc).await });
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
        while let Some((range, resp)) = rx.recv().await {
            let _seek = writer
                .seek(std::io::SeekFrom::Start(*range.start()))
                .await?;
            let stream = resp.bytes_stream().map_err(std::io::Error::other);
            let mut stream = tokio_util::io::StreamReader::new(stream);
            tokio::io::copy(&mut stream, &mut writer).await?;
        }
        writer.flush().await?;

        stop.await.map_err(|e| Error::FutureErr(e.to_string()))??;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;
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
