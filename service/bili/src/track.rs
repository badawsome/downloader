//! Turns inputs into a flat list of audio tracks with their metadata.

use bili::{Client, Episode, UgcSeason, VideoId, VideoView};
use futures_util::{stream, StreamExt};

use crate::input::{Input, PageSelection};
use crate::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    /// A single page video.
    Single,
    /// One part of a multi page video.
    Part,
    /// One episode of a collection (合集).
    Season,
    /// A video coming from a favorites folder.
    Favorite,
}

impl TrackKind {
    pub fn default_template(self) -> &'static str {
        match self {
            TrackKind::Single => "{artist} - {title}",
            TrackKind::Part | TrackKind::Season => "{album}/{index} - {title}",
            TrackKind::Favorite => "{collection}/{artist} - {title}",
        }
    }
}

/// One audio file to produce.
#[derive(Debug, Clone)]
pub struct Track {
    pub kind: TrackKind,
    pub bvid: String,
    pub aid: u64,
    pub cid: u64,
    /// 1-based page number within the video.
    pub page: u32,
    pub title: String,
    pub video_title: String,
    /// Page title.
    pub part: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub index: Option<u32>,
    pub total: Option<u32>,
    pub pubdate: i64,
    pub cover_url: String,
    pub description: String,
    /// bilibili category, e.g. `翻唱`.
    pub genre: String,
    /// Name of the season / favorites folder the track came from.
    pub collection: String,
    /// Whether the recognised background music title may replace `title`.
    pub bgm_lookup: bool,
}

impl Track {
    pub fn video_id(&self) -> VideoId {
        VideoId::Bvid(self.bvid.clone())
    }

    pub fn url(&self) -> String {
        if self.page > 1 {
            format!(
                "https://www.bilibili.com/video/{}/?p={}",
                self.bvid, self.page
            )
        } else {
            format!("https://www.bilibili.com/video/{}/", self.bvid)
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct ResolveOptions {
    /// Download the whole collection (合集) a video belongs to.
    pub expand_season: bool,
    /// Pages of multi part videos to keep.
    pub pages: Option<PageSelection>,
}

fn clean(s: &str) -> String {
    s.trim().to_owned()
}

/// Removes numbering that duplicates the part index: `001.周杰伦-爱在西元前`,
/// `P3 xxx`, `03、xxx` → the remaining title (when anything remains).
fn strip_index_prefix(part: &str, page: u32) -> String {
    let t = part.trim_start();
    let rest = t
        .strip_prefix('P')
        .or_else(|| t.strip_prefix('p'))
        .unwrap_or(t);
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() || digits.parse::<u32>().ok() != Some(page) {
        return part.to_owned();
    }
    let after = &rest[digits.len()..];
    // the number must be followed by a separator, not be part of the title
    let Some(sep) = after.chars().next() else {
        return part.to_owned();
    };
    if !matches!(
        sep,
        '.' | '、' | '-' | '_' | ' ' | ')' | '）' | ':' | '：' | '．' | '|'
    ) {
        return part.to_owned();
    }
    let stripped = after
        .trim_start_matches(['.', '、', '-', '_', ' ', ')', '）', ':', '：', '．', '|'])
        .trim();
    if stripped.is_empty() {
        part.to_owned()
    } else {
        stripped.to_owned()
    }
}

fn season_position(season: &UgcSeason, bvid: &str) -> Option<(u32, u32)> {
    let total = season.episodes().count() as u32;
    season
        .episodes()
        .position(|e| e.bvid == bvid)
        .map(|i| (i as u32 + 1, total))
}

/// Tracks of a single video: one per selected page.
pub(crate) fn tracks_from_view(
    view: &VideoView,
    only_page: Option<u32>,
    opts: &ResolveOptions,
    collection: Option<&str>,
) -> Vec<Track> {
    let mut pages = view.pages.clone();
    if pages.is_empty() {
        pages.push(bili::Page {
            cid: view.cid,
            page: 1,
            part: view.title.clone(),
            duration: view.duration,
        });
    }
    let multi = pages.len() > 1;
    let total_pages = pages.len() as u32;
    let selected: Vec<_> = pages
        .into_iter()
        .enumerate()
        .map(|(i, p)| (if p.page == 0 { i as u32 + 1 } else { p.page }, p))
        .filter(|(n, _)| match only_page {
            Some(only) => *n == only,
            None => opts.pages.as_ref().is_none_or(|s| !multi || s.contains(*n)),
        })
        .collect();

    let owner = clean(&view.owner.name);
    let season = view.ugc_season.as_ref();
    selected
        .into_iter()
        .map(|(n, p)| {
            let (kind, title, album, index, total) = if multi {
                let part = strip_index_prefix(&clean(&p.part), n);
                let title = if part.is_empty() {
                    format!("P{n}")
                } else {
                    part
                };
                (
                    TrackKind::Part,
                    title,
                    clean(&view.title),
                    Some(n),
                    Some(total_pages),
                )
            } else if let Some((i, t)) = season.and_then(|s| season_position(s, &view.bvid)) {
                let season = season.unwrap();
                (
                    TrackKind::Single,
                    clean(&view.title),
                    clean(&season.title),
                    Some(i),
                    Some(t),
                )
            } else {
                (
                    TrackKind::Single,
                    clean(&view.title),
                    clean(&view.title),
                    None,
                    None,
                )
            };
            let kind = if collection.is_some() {
                TrackKind::Favorite
            } else {
                kind
            };
            Track {
                kind,
                bvid: view.bvid.clone(),
                aid: view.aid,
                cid: p.cid,
                page: n,
                title,
                video_title: clean(&view.title),
                part: clean(&p.part),
                artist: owner.clone(),
                album,
                album_artist: owner.clone(),
                index,
                total,
                pubdate: view.pubdate,
                cover_url: view.pic.clone(),
                description: view.desc.clone(),
                genre: view.category(),
                collection: collection.map(clean).unwrap_or_default(),
                bgm_lookup: !multi,
            }
        })
        .collect()
}

/// Tracks of every episode of the collection `view` belongs to.
pub(crate) fn tracks_from_season(view: &VideoView, season: &UgcSeason) -> Vec<Track> {
    let episodes: Vec<&Episode> = season.episodes().collect();
    let total = episodes.len() as u32;
    let owner = clean(&view.owner.name);
    episodes
        .into_iter()
        .enumerate()
        .map(|(i, e)| {
            let arc = e.arc.clone().unwrap_or_default();
            let page = e.page.clone().unwrap_or_default();
            Track {
                kind: TrackKind::Season,
                bvid: e.bvid.clone(),
                aid: e.aid,
                cid: e.cid,
                page: page.page.max(1),
                title: clean(&e.title),
                video_title: clean(&e.title),
                part: clean(&page.part),
                artist: owner.clone(),
                album: clean(&season.title),
                album_artist: owner.clone(),
                index: Some(i as u32 + 1),
                total: Some(total),
                pubdate: arc.pubdate,
                cover_url: if arc.pic.is_empty() {
                    season.cover.clone()
                } else {
                    arc.pic.clone()
                },
                description: arc.desc,
                genre: view.category(),
                collection: clean(&season.title),
                bgm_lookup: true,
            }
        })
        .collect()
}

/// Resolves one input into tracks. Non fatal problems (e.g. deleted videos
/// in a favorites folder) are appended to `warnings`.
pub async fn resolve_input(
    client: &Client,
    input: &Input,
    opts: &ResolveOptions,
    warnings: &mut Vec<String>,
) -> Result<Vec<Track>> {
    match input {
        Input::ShortLink(url) => {
            let target = client.resolve_short_url(url).await?;
            let resolved = crate::input::parse_input(&target).map_err(|_| {
                Error::Input(format!("{url} points to an unsupported page: {target}"))
            })?;
            if matches!(resolved, Input::ShortLink(_)) {
                return Err(Error::Input(format!("cannot resolve short link {url}")));
            }
            Box::pin(resolve_input(client, &resolved, opts, warnings)).await
        }
        Input::Video { id, page } => {
            let view = client.view(id).await?;
            if opts.expand_season {
                if let Some(season) = view
                    .ugc_season
                    .as_ref()
                    .filter(|s| s.episodes().next().is_some())
                {
                    return Ok(tracks_from_season(&view, season));
                }
            }
            Ok(tracks_from_view(&view, *page, opts, None))
        }
        Input::Collection { mid, season_id } => {
            let list = client.collection(*mid, *season_id).await?;
            let first = list
                .items
                .first()
                .ok_or_else(|| Error::Input(format!("collection {season_id} is empty")))?;
            // The view of any member carries the full episode list.
            let view = client.view(&VideoId::Bvid(first.bvid.clone())).await?;
            match view.ugc_season.as_ref().filter(|s| s.id == *season_id) {
                Some(season) => Ok(tracks_from_season(&view, season)),
                None => Err(Error::Input(format!(
                    "could not load the episodes of collection {season_id}"
                ))),
            }
        }
        Input::Favorites { media_id } => {
            let list = client.favorites(*media_id).await?;
            let name = if list.title.is_empty() {
                format!("fav-{media_id}")
            } else {
                list.title.clone()
            };
            let views: Vec<_> =
                stream::iter(list.items.iter())
                    .map(|item| async move {
                        (item, client.view(&VideoId::Bvid(item.bvid.clone())).await)
                    })
                    .buffered(4)
                    .collect()
                    .await;
            let mut tracks = Vec::new();
            for (item, view) in views {
                match view {
                    Ok(view) => tracks.extend(tracks_from_view(&view, None, opts, Some(&name))),
                    // Removed / private videos are common in old folders.
                    Err(e) if e.is_not_found() => warnings.push(format!(
                        "skipping unavailable video {} ({}): {e}",
                        item.bvid, item.title
                    )),
                    Err(e) => return Err(e.into()),
                }
            }
            Ok(tracks)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(pages: usize, season: bool) -> VideoView {
        let pages: Vec<_> = (1..=pages)
            .map(|i| serde_json::json!({"cid": 100 + i, "page": i, "part": format!("part {i}")}))
            .collect();
        let mut v = serde_json::json!({
            "aid": 1, "bvid": "BV1xx411c7mD", "cid": 101, "title": " 视频 ", "pic": "http://i0.hdslb.com/a.jpg",
            "owner": {"mid": 9, "name": "UP"}, "pubdate": 1700000000, "tname": "翻唱", "pages": pages,
        });
        if season {
            v["ugc_season"] = serde_json::json!({"id": 5, "title": "合集", "sections": [
                {"episodes": [
                    {"aid": 2, "bvid": "BV1aa411c7mD", "cid": 201, "title": "e1", "arc": {"pic": "p1", "pubdate": 1}},
                    {"aid": 1, "bvid": "BV1xx411c7mD", "cid": 101, "title": "e2"}
                ]},
                {"episodes": [{"aid": 3, "bvid": "BV1bb411c7mD", "cid": 301, "title": "e3"}]}
            ]});
        }
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn single_and_multi_page() {
        let t = tracks_from_view(&view(1, false), None, &ResolveOptions::default(), None);
        assert_eq!(t.len(), 1);
        assert_eq!(t[0].kind, TrackKind::Single);
        assert_eq!(t[0].title, "视频");
        assert_eq!(t[0].album, "视频");
        assert!(t[0].bgm_lookup);

        let t = tracks_from_view(&view(3, false), None, &ResolveOptions::default(), None);
        assert_eq!(t.len(), 3);
        assert_eq!(
            (t[2].title.as_str(), t[2].index, t[2].total),
            ("part 3", Some(3), Some(3))
        );
        assert!(t.iter().all(|t| t.kind == TrackKind::Part && !t.bgm_lookup));

        let opts = ResolveOptions {
            pages: Some("2-3".parse().unwrap()),
            ..Default::default()
        };
        let t = tracks_from_view(&view(3, false), None, &opts, None);
        assert_eq!(t.iter().map(|t| t.cid).collect::<Vec<_>>(), vec![102, 103]);
        let t = tracks_from_view(&view(3, false), Some(1), &opts, None);
        assert_eq!(t.iter().map(|t| t.cid).collect::<Vec<_>>(), vec![101]);
    }

    #[test]
    fn part_numbering_is_stripped() {
        assert_eq!(
            strip_index_prefix("001.周杰伦-爱在西元前", 1),
            "周杰伦-爱在西元前"
        );
        assert_eq!(strip_index_prefix("P3 晴天", 3), "晴天");
        assert_eq!(strip_index_prefix("03、晴天", 3), "晴天");
        assert_eq!(strip_index_prefix("12.晴天", 3), "12.晴天");
        assert_eq!(
            strip_index_prefix("1987我不知道会遇见你", 1),
            "1987我不知道会遇见你"
        );
        assert_eq!(
            strip_index_prefix("2002年的第一场雪", 2),
            "2002年的第一场雪"
        );
        assert_eq!(strip_index_prefix("1", 1), "1");
        assert_eq!(strip_index_prefix("Part 1", 1), "Part 1");
    }

    #[test]
    fn season_metadata() {
        let v = view(1, true);
        // single video inside a season: album = season, index from season
        let t = tracks_from_view(&v, None, &ResolveOptions::default(), None);
        assert_eq!(
            (t[0].album.as_str(), t[0].index, t[0].total),
            ("合集", Some(2), Some(3))
        );

        let all = tracks_from_season(&v, v.ugc_season.as_ref().unwrap());
        assert_eq!(all.len(), 3, "episodes of every section");
        assert_eq!(all[0].cover_url, "p1");
        assert_eq!(all[2].index, Some(3));
        assert!(all.iter().all(|t| t.album == "合集" && t.artist == "UP"));
    }
}
