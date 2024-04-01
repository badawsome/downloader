use tokio::io::{AsyncSeekExt, AsyncWriteExt};

use super::error::*;
use super::models::*;

pub trait AcconutService {}

pub trait VideoService {
    fn get_basic_info(
        self,
        id: &VideoId,
    ) -> impl std::future::Future<Output = Result<VideoMetadata>> + Send;

    fn get_download_info(
        self,
        param: &GetDownloadInfoParam,
    ) -> impl std::future::Future<Output = Result<DownloadInfo>> + Send;

    fn download<'a, W>(
        self,
        param: &DownloadParam,
        writer: &mut W,
    ) -> impl std::future::Future<Output = Result<()>> + Send
    where
        W: AsyncWriteExt + AsyncSeekExt + Send + Sync + Unpin;
}

pub trait SeasonService {}

pub trait SearchService {}
