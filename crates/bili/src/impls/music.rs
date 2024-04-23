use derive_getters::Getters;
use serde::{Deserialize, Serialize};

use self::facade::MusicService;

use super::*;

#[derive(Debug, Serialize, Getters)]
pub struct BasicMusicInfo {
    music_id: String,
    title: String,
}

impl<'a> MusicService for &Service<'a> {
    type Id = (VideoId, u64);

    type BasicMusicInfo = BasicMusicInfo;

    // GET /x/player/wbi/v2
    async fn get_music_info(self, id: &Self::Id) -> Result<Self::BasicMusicInfo> {
        let url = format!(
            "{}{}/x/player/wbi/v2",
            self.protocol.get_prefix(),
            self.api_host
        );
        let query = [
            match &id.0 {
                VideoId::AID(aid) => ("aid", aid.to_string()),
                VideoId::BVID(bvid) => ("bvid", bvid.clone()),
            },
            ("cid", id.1.to_string()),
        ];

        #[derive(Debug, Deserialize)]
        struct MusicInfoInner {
            bgm_info: BgmInfoInner,
        }

        #[derive(Debug, Deserialize)]
        struct BgmInfoInner {
            music_id: String,
            music_title: String,
        }

        let res = self
            .client
            .get(url)
            .query(&query)
            .send()
            .await?
            .json::<PackInfo<MusicInfoInner>>()
            .await?
            .as_result()?;
        Ok(BasicMusicInfo {
            music_id: res.bgm_info.music_id,
            title: res.bgm_info.music_title,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facade::*;

    #[tokio::test]
    async fn test_music_info() -> anyhow::Result<()> {
        anyhow_music_info().await
    }

    async fn anyhow_music_info() -> anyhow::Result<()> {
        let s = Service::new();
        let id = VideoId::BVID("BV1Vh4y1v7qn".to_owned());
        let basic_info = s.get_basic_info(&id).await?;
        let music_info = s.get_music_info(&(id, *basic_info.cid())).await?;
        assert!(music_info.title == "一样的月光".to_owned());
        Ok(())
    }
}
