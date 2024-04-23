use super::*;

impl<'a> facade::SeasonService for &Service<'a> {
    // GET /x/web-interface/view/detail
    async fn get_video_relation_season_list(
        self,
        id: &VideoId,
        _season_id: u64,
    ) -> Result<SeasonList> {
        use serde::Deserialize;
        #[derive(Debug, Clone, Deserialize)]
        #[allow(dead_code)]
        struct Detail {
            #[serde(rename = "View")]
            main_view: MainView,
            #[serde(rename = "Related")]
            related: Vec<View>,
        }

        #[derive(Debug, Clone, Deserialize)]
        #[allow(dead_code)]
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
        #[allow(dead_code)]
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
    use super::*;
    use crate::facade::*;

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
