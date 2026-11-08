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
    /// Category name, e.g. `翻唱` (often empty nowadays, see [`Self::category`]).
    #[serde(default)]
    pub tname: String,
    #[serde(default)]
    pub tname_v2: String,
    /// Category id.
    #[serde(default)]
    pub tid: u32,
    #[serde(default, deserialize_with = "null_default")]
    pub pages: Vec<Page>,
    #[serde(default)]
    pub ugc_season: Option<UgcSeason>,
}

impl VideoView {
    /// Category name used as genre: `tname`, `tname_v2`, or derived from the
    /// (music) category id.
    pub fn category(&self) -> String {
        let name = [self.tname.trim(), self.tname_v2.trim()]
            .into_iter()
            .find(|s| !s.is_empty());
        if let Some(name) = name {
            return name.to_owned();
        }
        match self.tid {
            28 => "原创音乐",
            29 => "音乐现场",
            30 => "VOCALOID·UTAU",
            31 => "翻唱",
            59 => "演奏",
            130 => "音乐综合",
            193 => "MV",
            194 => "电音",
            243 => "乐评盘点",
            244 => "音乐教学",
            _ => "",
        }
        .to_owned()
    }
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

impl BgmInfo {
    /// The song name. The API decorates it (`发现《一样的月光》`), so the
    /// text inside the first `《》` is preferred.
    pub fn song_title(&self) -> String {
        let t = self.music_title.trim();
        let name = t
            .split_once('《')
            .and_then(|(_, rest)| rest.split_once('》'))
            .map(|(inner, _)| inner.trim())
            .filter(|inner| !inner.is_empty())
            .unwrap_or_else(|| t.strip_prefix("发现").unwrap_or(t).trim());
        strip_backing_track_marker(name)
    }
}

/// Covers are often matched against the karaoke backing track, e.g.
/// `Breeze (伴奏)`; the song itself is what the video contains.
fn strip_backing_track_marker(name: &str) -> String {
    const MARKERS: [&str; 6] = [
        "伴奏",
        "inst",
        "instrumental",
        "off vocal",
        "karaoke",
        "纯伴",
    ];
    let mut s = name.trim().to_owned();
    while let Some(close) = s
        .chars()
        .last()
        .filter(|c| matches!(c, ')' | '）' | ']' | '】'))
    {
        let open = match close {
            ')' => ['(', '（'],
            '）' => ['（', '('],
            ']' => ['[', '['],
            _ => ['【', '【'],
        };
        let Some(start) = s.rfind(open) else { break };
        let inner = s[start..s.len() - close.len_utf8()].to_lowercase();
        if start == 0 || !MARKERS.iter().any(|m| inner.contains(m)) {
            break;
        }
        s.truncate(start);
        s = s.trim_end().to_owned();
    }
    for suffix in [" - 伴奏", "-伴奏", "伴奏版", " 伴奏"] {
        if let Some(rest) = s.strip_suffix(suffix) {
            if !rest.trim().is_empty() {
                s = rest.trim_end().to_owned();
            }
        }
    }
    s
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
    fn bgm_song_title() {
        let bgm = |t: &str| BgmInfo {
            music_id: String::new(),
            music_title: t.into(),
            jump_url: String::new(),
        };
        assert_eq!(bgm("发现《一样的月光》").song_title(), "一样的月光");
        assert_eq!(
            bgm(" 发现《The Truth That You Leave》 ").song_title(),
            "The Truth That You Leave"
        );
        assert_eq!(bgm("一样的月光").song_title(), "一样的月光");
        assert_eq!(bgm("发现 Song").song_title(), "Song");
        assert_eq!(bgm("发现《Breeze (伴奏)》").song_title(), "Breeze");
        assert_eq!(bgm("晴天（伴奏）").song_title(), "晴天");
        assert_eq!(bgm("Lemon (Instrumental)").song_title(), "Lemon");
        assert_eq!(bgm("夜曲 伴奏版").song_title(), "夜曲");
        assert_eq!(bgm("Song (Live)").song_title(), "Song (Live)");
        assert_eq!(bgm("(伴奏)").song_title(), "(伴奏)");
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
