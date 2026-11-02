//! Response models. Only the fields used by the downloader are declared and
//! nearly everything is optional/defaulted so API drift does not break
//! parsing.

use serde::{Deserialize, Deserializer};

/// `null` → `T::default()`.
fn null_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: Default + Deserialize<'de>,
{
    Ok(Option::<T>::deserialize(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Owner {
    #[serde(default)]
    pub mid: u64,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub face: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Page {
    pub cid: u64,
    #[serde(default)]
    pub page: u32,
    #[serde(default)]
    pub part: String,
    #[serde(default)]
    pub duration: u64,
}

/// `GET /x/web-interface/view`
#[derive(Debug, Clone, Deserialize)]
pub struct VideoView {
    pub aid: u64,
    pub bvid: String,
    #[serde(default)]
    pub cid: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pic: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub owner: Owner,
    #[serde(default)]
    pub pubdate: i64,
    #[serde(default)]
    pub duration: u64,
    /// Category name, e.g. `翻唱`.
    #[serde(default)]
    pub tname: String,
    #[serde(default, deserialize_with = "null_default")]
    pub pages: Vec<Page>,
    #[serde(default)]
    pub ugc_season: Option<UgcSeason>,
}

/// A "合集" (UGC season) as embedded in the view response.
#[derive(Debug, Clone, Deserialize)]
pub struct UgcSeason {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub mid: u64,
    #[serde(default)]
    pub intro: String,
    #[serde(default, deserialize_with = "null_default")]
    pub sections: Vec<SeasonSection>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeasonSection {
    #[serde(default)]
    pub title: String,
    #[serde(default, deserialize_with = "null_default")]
    pub episodes: Vec<Episode>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Episode {
    pub aid: u64,
    pub bvid: String,
    pub cid: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub arc: Option<EpisodeArc>,
    #[serde(default)]
    pub page: Option<Page>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct EpisodeArc {
    #[serde(default)]
    pub pic: String,
    #[serde(default)]
    pub pubdate: i64,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub duration: u64,
}

impl UgcSeason {
    /// All episodes of every section, in order.
    pub fn episodes(&self) -> impl Iterator<Item = &Episode> {
        self.sections.iter().flat_map(|s| s.episodes.iter())
    }
}

// ------------------------------------------------------------------ playurl

/// `GET /x/player/wbi/playurl`
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PlayUrl {
    #[serde(default)]
    pub quality: u32,
    #[serde(default)]
    pub format: String,
    #[serde(default)]
    pub dash: Option<Dash>,
    #[serde(default)]
    pub durl: Option<Vec<Durl>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Dash {
    #[serde(default)]
    pub duration: u64,
    #[serde(default)]
    pub audio: Option<Vec<DashStream>>,
    #[serde(default)]
    pub dolby: Option<DashDolby>,
    #[serde(default)]
    pub flac: Option<DashFlac>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DashDolby {
    #[serde(default, rename = "type")]
    pub kind: u32,
    #[serde(default)]
    pub audio: Option<Vec<DashStream>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct DashFlac {
    #[serde(default)]
    pub display: bool,
    #[serde(default)]
    pub audio: Option<DashStream>,
}

/// One DASH representation. The API returns most fields twice (camelCase
/// and snake_case), so both spellings are captured separately.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct DashStream {
    pub id: u32,
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default, rename = "baseUrl")]
    base_url_camel: Option<String>,
    #[serde(default)]
    backup_url: Option<Vec<String>>,
    #[serde(default, rename = "backupUrl")]
    backup_url_camel: Option<Vec<String>>,
    #[serde(default)]
    pub bandwidth: u64,
    #[serde(default)]
    mime_type: Option<String>,
    #[serde(default, rename = "mimeType")]
    mime_type_camel: Option<String>,
    #[serde(default)]
    pub codecs: String,
    #[serde(default)]
    pub size: Option<u64>,
}

impl DashStream {
    /// Primary URL followed by the backups, without duplicates.
    pub fn urls(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let candidates = self
            .base_url
            .iter()
            .chain(self.base_url_camel.iter())
            .chain(self.backup_url.iter().flatten())
            .chain(self.backup_url_camel.iter().flatten());
        for u in candidates {
            if !u.is_empty() && !out.contains(u) {
                out.push(u.clone());
            }
        }
        out
    }

    pub fn mime_type(&self) -> &str {
        self.mime_type
            .as_deref()
            .or(self.mime_type_camel.as_deref())
            .unwrap_or("")
    }
}

/// Progressive (muxed audio+video) stream, used when no DASH is offered.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Durl {
    #[serde(default)]
    pub order: u32,
    #[serde(default)]
    pub size: u64,
    #[serde(default)]
    pub length: u64,
    pub url: String,
    #[serde(default, deserialize_with = "null_default")]
    pub backup_url: Vec<String>,
}

// ------------------------------------------------------------------ misc

/// `bgm_info` of `GET /x/player/wbi/v2`.
#[derive(Debug, Clone, Deserialize)]
pub struct BgmInfo {
    #[serde(default)]
    pub music_id: String,
    #[serde(default)]
    pub music_title: String,
    #[serde(default)]
    pub jump_url: String,
}

/// `GET /x/web-interface/nav` (also returned when not logged in).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct NavInfo {
    #[serde(default, rename = "isLogin")]
    pub is_login: bool,
    #[serde(default)]
    pub mid: u64,
    #[serde(default)]
    pub uname: String,
    /// 1 when the big VIP (大会员) is active.
    #[serde(default, rename = "vipStatus")]
    pub vip_status: u32,
    #[serde(default, rename = "vipType")]
    pub vip_type: u32,
    #[serde(default)]
    pub wbi_img: Option<WbiImg>,
}

impl NavInfo {
    pub fn is_vip(&self) -> bool {
        self.vip_status == 1
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct WbiImg {
    #[serde(default)]
    pub img_url: String,
    #[serde(default)]
    pub sub_url: String,
}

/// A video entry of a list (favorites, collection).
#[derive(Debug, Clone)]
pub struct ListItem {
    pub aid: u64,
    pub bvid: String,
    pub title: String,
}

/// Result of listing a favorites folder or a collection.
#[derive(Debug, Clone)]
pub struct VideoList {
    pub title: String,
    pub owner: String,
    pub items: Vec<ListItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_playurl_dash() {
        let json = r#"{
            "quality": 80, "format": "flv",
            "dash": {
                "duration": 213,
                "video": [{"id": 80}],
                "audio": [
                    {"id": 30216, "baseUrl": "https://a/64", "base_url": "https://a/64",
                     "backupUrl": ["https://b/64"], "backup_url": ["https://b/64"],
                     "bandwidth": 67000, "mimeType": "audio/mp4", "mime_type": "audio/mp4", "codecs": "mp4a.40.2"},
                    {"id": 30280, "baseUrl": "https://a/192", "base_url": "https://a/192",
                     "backupUrl": null, "backup_url": null,
                     "bandwidth": 320000, "mimeType": "audio/mp4", "mime_type": "audio/mp4", "codecs": "mp4a.40.2"}
                ],
                "dolby": {"type": 1, "audio": [{"id": 30250, "base_url": "https://a/dolby", "bandwidth": 448000, "codecs": "ec-3"}]},
                "flac": {"display": true, "audio": {"id": 30251, "baseUrl": "https://a/flac", "bandwidth": 1500000, "codecs": "fLaC"}}
            }
        }"#;
        let p: PlayUrl = serde_json::from_str(json).unwrap();
        let dash = p.dash.unwrap();
        let audio = dash.audio.unwrap();
        assert_eq!(audio.len(), 2);
        assert_eq!(audio[0].urls(), vec!["https://a/64", "https://b/64"]);
        assert_eq!(audio[0].mime_type(), "audio/mp4");
        assert_eq!(
            dash.dolby.unwrap().audio.unwrap()[0].urls(),
            vec!["https://a/dolby"]
        );
        assert_eq!(
            dash.flac.unwrap().audio.unwrap().urls(),
            vec!["https://a/flac"]
        );
    }

    #[test]
    fn parse_view_with_nulls() {
        let json = r#"{"aid": 1, "bvid": "BV1xx411c7mD", "cid": 2, "title": "t",
            "owner": {"mid": 3, "name": "up", "face": ""}, "pages": null,
            "ugc_season": {"id": 9, "title": "s", "sections": [
                {"title": "a", "episodes": [{"aid": 1, "bvid": "BV1xx411c7mD", "cid": 2, "title": "e1"}]},
                {"title": "b", "episodes": null}
            ]}}"#;
        let v: VideoView = serde_json::from_str(json).unwrap();
        assert!(v.pages.is_empty());
        assert_eq!(v.ugc_season.unwrap().episodes().count(), 1);
    }
}
