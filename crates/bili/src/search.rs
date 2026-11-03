//! Video search result model and helpers.

use serde::{Deserialize, Deserializer};

/// Sort order of [`crate::Client::search_videos`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SearchOrder {
    /// 综合排序
    #[default]
    Relevance,
    /// 最多播放
    Plays,
    /// 最新发布
    Newest,
    /// 最多弹幕
    Danmaku,
    /// 最多收藏
    Favorites,
}

impl SearchOrder {
    pub(crate) fn param(self) -> &'static str {
        match self {
            SearchOrder::Relevance => "totalrank",
            SearchOrder::Plays => "click",
            SearchOrder::Newest => "pubdate",
            SearchOrder::Danmaku => "dm",
            SearchOrder::Favorites => "stow",
        }
    }
}

impl std::str::FromStr for SearchOrder {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "relevance" | "default" | "totalrank" => SearchOrder::Relevance,
            "plays" | "click" | "views" => SearchOrder::Plays,
            "newest" | "pubdate" | "date" => SearchOrder::Newest,
            "danmaku" | "dm" => SearchOrder::Danmaku,
            "favorites" | "stow" | "fav" => SearchOrder::Favorites,
            other => {
                return Err(format!(
                    "unknown order '{other}' (relevance, plays, newest, danmaku, favorites)"
                ))
            }
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct SearchOptions {
    /// 1-based result page (20 results per page).
    pub page: u32,
    pub order: SearchOrder,
    /// Only videos of the music category (音乐区).
    pub music_only: bool,
}

/// One page of search results.
#[derive(Debug, Clone)]
pub struct SearchPage {
    pub page: u32,
    pub pages: u32,
    pub total: u64,
    pub items: Vec<SearchItem>,
}

#[derive(Debug, Clone)]
pub struct SearchItem {
    pub aid: u64,
    pub bvid: String,
    /// Title with the keyword highlights removed.
    pub title: String,
    /// Title split into (text, is_keyword_match) parts for highlighting.
    pub title_parts: Vec<(String, bool)>,
    pub author: String,
    pub mid: u64,
    pub duration_secs: u64,
    pub plays: u64,
    pub pubdate: i64,
    pub category: String,
}

fn lenient_u64<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
    Ok(match serde_json::Value::deserialize(d)? {
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(0),
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(0),
        _ => 0,
    })
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawSearch {
    #[serde(default)]
    pub page: u32,
    #[serde(default, rename = "numPages")]
    pub num_pages: u32,
    #[serde(default, rename = "numResults")]
    pub num_results: u64,
    #[serde(default)]
    pub result: Option<Vec<RawItem>>,
    /// Present (with no results) when bilibili wants a captcha.
    #[serde(default)]
    pub v_voucher: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct RawItem {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default, deserialize_with = "lenient_u64")]
    aid: u64,
    #[serde(default)]
    bvid: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    author: String,
    #[serde(default, deserialize_with = "lenient_u64")]
    mid: u64,
    #[serde(default)]
    duration: String,
    #[serde(default, deserialize_with = "lenient_u64")]
    play: u64,
    #[serde(default)]
    pubdate: i64,
    #[serde(default)]
    typename: String,
}

impl RawSearch {
    pub(crate) fn into_page(self) -> SearchPage {
        let items = self
            .result
            .unwrap_or_default()
            .into_iter()
            .filter(|r| (r.kind.is_empty() || r.kind == "video") && !r.bvid.is_empty())
            .map(|r| {
                let title_parts = highlight_parts(&r.title);
                SearchItem {
                    aid: r.aid,
                    bvid: r.bvid,
                    title: title_parts.iter().map(|(t, _)| t.as_str()).collect(),
                    title_parts,
                    author: decode_entities(&r.author),
                    mid: r.mid,
                    duration_secs: parse_duration(&r.duration),
                    plays: r.play,
                    pubdate: r.pubdate,
                    category: r.typename,
                }
            })
            .collect();
        SearchPage {
            page: self.page.max(1),
            pages: self.num_pages,
            total: self.num_results,
            items,
        }
    }
}

/// `"4:1"` → 241, `"1:02:03"` → 3723.
pub fn parse_duration(s: &str) -> u64 {
    s.split(':')
        .map(|p| p.trim().parse::<u64>().unwrap_or(0))
        .fold(0, |acc, v| acc * 60 + v)
}

/// Decodes the HTML entities bilibili uses in titles.
pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let tail = &rest[amp..];
        let decoded = tail.find(';').filter(|&end| end <= 10).and_then(|end| {
            let name = &tail[1..end];
            let c = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            }?;
            Some((c, end + 1))
        });
        match decoded {
            Some((c, len)) => {
                out.push(c);
                rest = &tail[len..];
            }
            None => {
                out.push('&');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Splits `a<em class="keyword">b</em>c` into `[(a,false),(b,true),(c,false)]`,
/// dropping any other tags and decoding entities.
pub fn highlight_parts(title: &str) -> Vec<(String, bool)> {
    let mut parts: Vec<(String, bool)> = Vec::new();
    let mut in_em = false;
    let mut rest = title;
    loop {
        let (text, tag) = match rest.find('<') {
            Some(i) => match rest[i..].find('>') {
                Some(j) => (&rest[..i], Some(&rest[i + 1..i + j])),
                None => (rest, None),
            },
            None => (rest, None),
        };
        if !text.is_empty() {
            parts.push((decode_entities(text), in_em));
        }
        let Some(tag) = tag else { break };
        let name = tag
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");
        if name.eq_ignore_ascii_case("em") {
            in_em = !tag.starts_with('/');
        }
        rest = &rest[text.len() + tag.len() + 2..];
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_and_numbers() {
        let parts = highlight_parts(
            r#"徐佳莹《<em class="keyword">一样的月光</em>》 &amp; Hi&#39;s &lt;3"#,
        );
        assert_eq!(
            parts,
            vec![
                ("徐佳莹《".to_string(), false),
                ("一样的月光".to_string(), true),
                ("》 & Hi's <3".to_string(), false),
            ]
        );
        assert_eq!(
            decode_entities("a &unknown; &#x4E2D; & b"),
            "a &unknown; 中 & b"
        );
        assert_eq!(parse_duration("4:1"), 241);
        assert_eq!(parse_duration("44:0"), 2640);
        assert_eq!(parse_duration("1:02:03"), 3723);
        assert_eq!("plays".parse::<SearchOrder>().unwrap().param(), "click");
    }

    #[test]
    fn parse_page() {
        let raw: RawSearch = serde_json::from_value(serde_json::json!({
            "page": 1, "numPages": 50, "numResults": 1000,
            "result": [
                {"type": "video", "aid": 1, "bvid": "BV1kw4m1e7G1", "title": "<em class=\"keyword\">x</em>y",
                 "author": "a&amp;b", "mid": 2, "duration": "3:58", "play": 5369393, "pubdate": 1719658800, "typename": "电台"},
                {"type": "video", "aid": 3, "bvid": "BV1aa411c7mD", "title": "t", "play": "--", "duration": "0:5"},
                {"type": "ketang", "aid": 4, "bvid": "", "title": "course"}
            ]
        }))
        .unwrap();
        let page = raw.into_page();
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].title, "xy");
        assert_eq!(page.items[0].author, "a&b");
        assert_eq!(page.items[0].duration_secs, 238);
        assert_eq!(page.items[1].plays, 0);
        assert_eq!((page.pages, page.total), (50, 1000));
    }
}
