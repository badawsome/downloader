//! Parsing of user supplied inputs (ids, urls, short links, lists).

use source_api::protocol::{SHORT_LINK_HOSTS, SPACE_HOST};
use source_api::VideoId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Input {
    /// A video; `page` is set when the url points at one part (`?p=3`).
    Video { id: VideoId, page: Option<u32> },
    /// A short link, resolved over the network.
    ShortLink(String),
    /// Favorites folder (收藏夹).
    Favorites { media_id: u64 },
    /// Collection (合集) of a user.
    Collection { mid: u64, season_id: u64 },
}

fn query_param<'a>(url: &'a str, key: &str) -> Option<&'a str> {
    let query = url.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|kv| {
        let (k, v) = kv.split_once('=')?;
        (k == key).then_some(v)
    })
}

fn number_after(text: &str, marker: &str) -> Option<u64> {
    let pos = text.find(marker)? + marker.len();
    let digits: String = text[pos..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

pub fn parse_input(raw: &str) -> Result<Input, String> {
    let s = raw.trim();
    let lower = s.to_ascii_lowercase();

    if SHORT_LINK_HOSTS
        .iter()
        .any(|host| lower.contains(&format!("{host}/")))
    {
        let url = if lower.starts_with("http") {
            s.to_owned()
        } else {
            format!("https://{s}")
        };
        return Ok(Input::ShortLink(url));
    }

    // favorites: fav:<id>, ml<id>, .../favlist?fid=<id>, /medialist/detail/ml<id>, /list/ml<id>
    if let Some(id) = lower
        .strip_prefix("fav:")
        .and_then(|v| v.trim().parse().ok())
    {
        return Ok(Input::Favorites { media_id: id });
    }
    if lower.contains("favlist") {
        if let Some(fid) = query_param(s, "fid").and_then(|v| v.parse().ok()) {
            return Ok(Input::Favorites { media_id: fid });
        }
        return Err(format!(
            "favorites url without fid (open a specific folder first): {s}"
        ));
    }
    if let Some(id) = lower
        .strip_prefix("ml")
        .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
        .and_then(|v| v.parse().ok())
        .or_else(|| number_after(&lower, "/ml"))
    {
        return Ok(Input::Favorites { media_id: id });
    }

    // Collections use /<mid>/channel/collectiondetail?sid=<sid> or /<mid>/lists/<sid>?type=season.
    let space_prefix = format!("{SPACE_HOST}/");
    if lower.contains(&space_prefix) {
        let mid = number_after(&lower, &space_prefix);
        let sid = query_param(s, "sid")
            .and_then(|v| v.parse().ok())
            .or_else(|| number_after(&lower, "/lists/"));
        if lower.contains("type=series") || lower.contains("seriesdetail") {
            return Err(format!(
                "video series (列表) are not supported, use a collection (合集): {s}"
            ));
        }
        return match (mid, sid) {
            (Some(mid), Some(season_id)) => Ok(Input::Collection { mid, season_id }),
            _ => Err(format!("unsupported space url: {s}")),
        };
    }

    let id = VideoId::parse(s)
        .or_else(|| VideoId::find_in(s))
        .ok_or_else(|| format!("not a supported video id or url: {s}"))?;
    let page = query_param(s, "p")
        .and_then(|p| p.parse().ok())
        .filter(|p| *p > 0);
    Ok(Input::Video { id, page })
}

/// Pages chosen with `--pages 1,3-5`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageSelection(Vec<(u32, u32)>);

impl PageSelection {
    pub fn contains(&self, page: u32) -> bool {
        self.0.iter().any(|(a, b)| (*a..=*b).contains(&page))
    }
}

impl std::str::FromStr for PageSelection {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        let mut out = Vec::new();
        for part in s.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let err = || format!("invalid page range '{part}'");
            let (a, b) = match part.split_once('-') {
                Some((a, b)) => (
                    a.trim().parse().map_err(|_| err())?,
                    b.trim().parse().map_err(|_| err())?,
                ),
                None => {
                    let p = part.parse().map_err(|_| err())?;
                    (p, p)
                }
            };
            if a == 0 || b < a {
                return Err(err());
            }
            out.push((a, b));
        }
        if out.is_empty() {
            return Err("empty page selection".into());
        }
        Ok(PageSelection(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inputs() {
        let bv = |p| Input::Video {
            id: VideoId::Bvid("BV1Vh4y1v7qn".into()),
            page: p,
        };
        assert_eq!(parse_input("BV1Vh4y1v7qn").unwrap(), bv(None));
        assert_eq!(
            parse_input("https://www.example.com/video/BV1Vh4y1v7qn/?spm_id_from=333&p=3").unwrap(),
            bv(Some(3))
        );
        assert_eq!(
            parse_input("av170001").unwrap(),
            Input::Video {
                id: VideoId::Aid(170001),
                page: None
            }
        );
        assert_eq!(
            parse_input(&format!("{}/abcd", SHORT_LINK_HOSTS[0])).unwrap(),
            Input::ShortLink(format!("https://{}/abcd", SHORT_LINK_HOSTS[0]))
        );
        assert_eq!(
            parse_input(&format!(
                "https://{SPACE_HOST}/123/favlist?fid=456&ftype=create"
            ))
            .unwrap(),
            Input::Favorites { media_id: 456 }
        );
        assert_eq!(
            parse_input("ml456").unwrap(),
            Input::Favorites { media_id: 456 }
        );
        assert_eq!(
            parse_input("fav:456").unwrap(),
            Input::Favorites { media_id: 456 }
        );
        assert_eq!(
            parse_input("https://www.example.com/list/ml456?oid=1").unwrap(),
            Input::Favorites { media_id: 456 }
        );
        assert_eq!(
            parse_input(&format!(
                "https://{SPACE_HOST}/123/channel/collectiondetail?sid=789"
            ))
            .unwrap(),
            Input::Collection {
                mid: 123,
                season_id: 789
            }
        );
        assert_eq!(
            parse_input(&format!("https://{SPACE_HOST}/123/lists/789?type=season")).unwrap(),
            Input::Collection {
                mid: 123,
                season_id: 789
            }
        );
        assert!(parse_input(&format!("https://{SPACE_HOST}/123/lists/789?type=series")).is_err());
        assert!(parse_input("hello world").is_err());
    }

    #[test]
    fn pages() {
        let sel: PageSelection = "1, 3-5".parse().unwrap();
        assert!(sel.contains(1) && sel.contains(4) && !sel.contains(2) && !sel.contains(6));
        assert!("0".parse::<PageSelection>().is_err());
        assert!("5-3".parse::<PageSelection>().is_err());
        assert!("".parse::<PageSelection>().is_err());
    }
}
