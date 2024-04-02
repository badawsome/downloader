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
    ) -> impl std::future::Future<Output = Result<DurlInfo>> + Send;

    fn download<'a, W>(
        self,
        param: &DownloadParam,
        writer: W,
    ) -> impl std::future::Future<Output = Result<()>> + Send
    where
        W: AsyncWriteExt + AsyncSeekExt + Send + Sync + Unpin;
}

pub trait SeasonService {
    fn get_video_relation_season_list(
        self,
        id: &VideoId,
        season_id: u64,
    ) -> impl std::future::Future<Output = Result<SeasonList>> + Send;

    fn season_id(
        self,
        id: &VideoId,
    ) -> impl std::future::Future<Output = Result<Option<u64>>> + Send;
}

pub trait SearchService {
    fn search_by_keyword<T, O>(key: String, search_type: T, search_opts: Option<O>);
}
