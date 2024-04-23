use self::facade::VideoService;

use super::*;

impl<'a> facade::Downloader<&'a DownloadParam> for Service<'a> {
    async fn download<'b, W>(self, param: &'a DownloadParam, mut writer: W) -> Result<()>
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

impl<'a> facade::Downloader<&'a VideoId> for Service<'a> {
    async fn download<'b, W>(self, param: &'a VideoId, mut writer: W) -> Result<()>
    where
        W: tokio::io::AsyncWriteExt + tokio::io::AsyncSeekExt + Send + Sync + Unpin,
    {
        let res = self.get_basic_info(param).await?;
        let res = self
            .get_download_info(&GetDownloadInfoParam {
                id: param.clone(),
                cid: res.cid().clone(),
                clarity: Clarity::Default,
            })
            .await?;
        self.download(
            &DownloadParam {
                info: res,
                chunk_size: Some(4 * 1024 * 1024),
                conn_pool: Some(3),
            },
            &mut writer,
        )
        .await
    }
}
