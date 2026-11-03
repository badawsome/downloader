//! HTTP client for the bilibili web API.

use std::sync::RwLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use reqwest::header::{self, HeaderMap, HeaderValue};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use tokio::sync::{Mutex, OnceCell, Semaphore};

use crate::cookie::CookieJar;
use crate::id::VideoId;
use crate::models::*;
use crate::wbi;
use crate::{Error, Result};

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/130.0.0.0 Safari/537.36";
pub const REFERER: &str = "https://www.bilibili.com/";

const API: &str = "https://api.bilibili.com";
const PASSPORT: &str = "https://passport.bilibili.com";
const API_TIMEOUT: Duration = Duration::from_secs(20);
const WBI_KEY_TTL: Duration = Duration::from_secs(60 * 60);
/// `fnval`: DASH(16) | HDR(64) | 4K(128) | Dolby audio(256) | Dolby
/// vision(512) | 8K(1024) | AV1(2048) — asks for every available stream.
const FNVAL_ALL: u32 = 4048;

#[derive(Debug, Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: serde_json::Value,
}

/// Status of a QR code login.
#[derive(Debug, Clone)]
pub enum QrStatus {
    /// Not scanned yet.
    Waiting,
    /// Scanned, waiting for confirmation in the app.
    Scanned,
    Expired,
    /// Logged in; the jar holds the new session cookies.
    Success(CookieJar),
}

/// A QR code to scan with the bilibili app.
#[derive(Debug, Clone, Deserialize)]
pub struct QrCode {
    pub url: String,
    pub qrcode_key: String,
}

pub struct Client {
    http: reqwest::Client,
    api: String,
    passport: String,
    cookies: RwLock<CookieJar>,
    wbi_key: Mutex<Option<(String, Instant)>>,
    buvid: OnceCell<()>,
    /// Cleared once `nav` reports that the session cookie is not logged in
    /// (expired), so requests fall back to anonymous behaviour.
    session_valid: std::sync::atomic::AtomicBool,
    api_slots: Semaphore,
}

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl Client {
    /// Creates a client; pass an empty jar for anonymous access.
    pub fn new(cookies: CookieJar) -> Result<Self> {
        let http = reqwest::Client::builder()
            .default_headers(Self::browser_headers())
            .connect_timeout(Duration::from_secs(10))
            .pool_max_idle_per_host(16)
            .build()?;
        // Endpoint overrides, for API mirrors / reverse proxies and testing.
        let env = |k: &str, default: &str| {
            std::env::var(k)
                .ok()
                .filter(|v| !v.trim().is_empty())
                .map(|v| v.trim().trim_end_matches('/').to_owned())
                .unwrap_or_else(|| default.to_owned())
        };
        Ok(Self {
            http,
            api: env("BILI_API_BASE", API),
            passport: env("BILI_PASSPORT_BASE", PASSPORT),
            cookies: RwLock::new(cookies),
            wbi_key: Mutex::new(None),
            buvid: OnceCell::new(),
            api_slots: Semaphore::new(4),
            session_valid: std::sync::atomic::AtomicBool::new(true),
        })
    }

    /// Headers needed by the bilibili CDN (referer check) and API.
    pub fn browser_headers() -> HeaderMap {
        let mut h = HeaderMap::new();
        h.insert(header::USER_AGENT, HeaderValue::from_static(USER_AGENT));
        h.insert(header::REFERER, HeaderValue::from_static(REFERER));
        h
    }

    /// The underlying HTTP client (shares the connection pool).
    pub fn http(&self) -> &reqwest::Client {
        &self.http
    }

    pub fn cookies(&self) -> CookieJar {
        self.cookies.read().unwrap().clone()
    }

    /// Has a session cookie that has not been found to be expired.
    pub fn is_logged_in(&self) -> bool {
        self.has_session()
            && self
                .session_valid
                .load(std::sync::atomic::Ordering::Relaxed)
    }

    pub fn has_session(&self) -> bool {
        self.cookies.read().unwrap().has_session()
    }

    fn cookie_header(&self) -> Option<HeaderValue> {
        let jar = self.cookies.read().unwrap();
        if jar.is_empty() {
            return None;
        }
        HeaderValue::from_str(&jar.header_value()).ok()
    }

    /// Fetches anonymous device cookies (`buvid3`/`buvid4`); without them
    /// some endpoints answer with risk control errors. Best effort.
    async fn ensure_buvid(&self) {
        self.buvid
            .get_or_init(|| async {
                if self.cookies.read().unwrap().contains("buvid3") {
                    return;
                }
                #[derive(Deserialize)]
                struct Spi {
                    b_3: String,
                    b_4: String,
                }
                let res = async {
                    let env: Envelope = self
                        .http
                        .get(format!("{}/x/frontend/finger/spi", self.api))
                        .timeout(API_TIMEOUT)
                        .send()
                        .await?
                        .json()
                        .await?;
                    Ok::<Spi, Error>(serde_json::from_value(env.data)?)
                }
                .await;
                if let Ok(spi) = res {
                    let mut jar = self.cookies.write().unwrap();
                    jar.set("buvid3", &spi.b_3);
                    jar.set("buvid4", &spi.b_4);
                }
            })
            .await;
    }

    async fn mixin_key(&self, refresh: bool) -> Result<String> {
        let mut guard = self.wbi_key.lock().await;
        if let Some((key, at)) = guard.as_ref() {
            if !refresh && at.elapsed() < WBI_KEY_TTL {
                return Ok(key.clone());
            }
        }
        // Fetched directly (not through `envelope`) to avoid recursion.
        let mut req = self
            .http
            .get(format!("{}/x/web-interface/nav", self.api))
            .timeout(API_TIMEOUT);
        if let Some(c) = self.cookie_header() {
            req = req.header(header::COOKIE, c);
        }
        let env: Envelope = req.send().await?.json().await?;
        let nav: NavInfo = serde_json::from_value(env.data)?;
        let img = nav
            .wbi_img
            .ok_or_else(|| Error::Unexpected("nav response without wbi_img".into()))?;
        let key = wbi::mixin_key(
            wbi::key_from_url(&img.img_url),
            wbi::key_from_url(&img.sub_url),
        );
        if key.len() != 32 {
            return Err(Error::Unexpected("invalid wbi keys".into()));
        }
        *guard = Some((key.clone(), Instant::now()));
        Ok(key)
    }

    /// Performs a GET returning the raw envelope. Network failures, HTTP 412
    /// and risk control codes are retried with backoff.
    async fn envelope(
        &self,
        url: &str,
        params: &[(String, String)],
        sign: bool,
    ) -> Result<Envelope> {
        self.ensure_buvid().await;
        let _slot = self.api_slots.acquire().await;
        let mut last: Option<Error> = None;
        for attempt in 0..4u32 {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(600 << attempt)).await;
            }
            let query = if sign {
                let key = self.mixin_key(attempt > 0).await?;
                wbi::sign(params, &key, now_secs())
            } else {
                wbi::build_query(params)
            };
            let full = if query.is_empty() {
                url.to_owned()
            } else {
                format!("{url}?{query}")
            };
            let mut req = self.http.get(&full).timeout(API_TIMEOUT);
            if let Some(c) = self.cookie_header() {
                req = req.header(header::COOKIE, c);
            }
            let resp = match req.send().await {
                Ok(r) => r,
                Err(e) => {
                    last = Some(e.into());
                    continue;
                }
            };
            let status = resp.status();
            let body = match resp.bytes().await {
                Ok(b) => b,
                Err(e) => {
                    last = Some(e.into());
                    continue;
                }
            };
            if status.as_u16() == 412 {
                last = Some(Error::RiskControl {
                    code: 412,
                    message: "HTTP 412 Precondition Failed".into(),
                });
                continue;
            }
            let env: Envelope = match serde_json::from_slice(&body) {
                Ok(e) => e,
                Err(_) if !status.is_success() => {
                    last = Some(Error::Unexpected(format!("HTTP {status} from {url}")));
                    continue;
                }
                Err(e) => return Err(e.into()),
            };
            match env.code {
                -352 | -412 | -799 | -509 => {
                    last = Some(Error::RiskControl {
                        code: env.code,
                        message: env.message,
                    });
                    continue;
                }
                // WBI keys rotate; refresh and retry once.
                -403 if sign && attempt == 0 => {
                    last = Some(Error::Api {
                        code: env.code,
                        message: env.message,
                    });
                    continue;
                }
                _ => return Ok(env),
            }
        }
        Err(last.unwrap_or_else(|| Error::Unexpected("request failed".into())))
    }

    async fn get_json<T: DeserializeOwned>(
        &self,
        url: &str,
        params: &[(String, String)],
        sign: bool,
    ) -> Result<T> {
        let env = self.envelope(url, params, sign).await?;
        match env.code {
            0 if env.data.is_null() => Err(Error::Unexpected(format!("{url}: missing data"))),
            0 => Ok(serde_json::from_value(env.data)?),
            -101 => Err(Error::NotLoggedIn),
            code => Err(Error::Api {
                code,
                message: env.message,
            }),
        }
    }

    fn params(pairs: &[(&str, String)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    // ------------------------------------------------------------ account

    /// Login state, VIP state and WBI keys. Works without login.
    pub async fn nav(&self) -> Result<NavInfo> {
        let env = self
            .envelope(&format!("{}/x/web-interface/nav", self.api), &[], false)
            .await?;
        match env.code {
            0 | -101 => {
                let nav: NavInfo = serde_json::from_value(env.data)?;
                if self.has_session() {
                    self.session_valid
                        .store(nav.is_login, std::sync::atomic::Ordering::Relaxed);
                }
                // Reuse the WBI keys so signing needs no second request.
                if let Some(img) = &nav.wbi_img {
                    let key = wbi::mixin_key(
                        wbi::key_from_url(&img.img_url),
                        wbi::key_from_url(&img.sub_url),
                    );
                    if key.len() == 32 {
                        *self.wbi_key.lock().await = Some((key, Instant::now()));
                    }
                }
                Ok(nav)
            }
            code => Err(Error::Api {
                code,
                message: env.message,
            }),
        }
    }

    /// Starts a QR code login.
    pub async fn qr_generate(&self) -> Result<QrCode> {
        self.get_json(
            &format!("{}/x/passport-login/web/qrcode/generate", self.passport),
            &[],
            false,
        )
        .await
    }

    /// Polls a QR code login. On success the session is also stored in this
    /// client.
    pub async fn qr_poll(&self, qrcode_key: &str) -> Result<QrStatus> {
        let resp = self
            .http
            .get(format!(
                "{}/x/passport-login/web/qrcode/poll",
                self.passport
            ))
            .query(&[("qrcode_key", qrcode_key)])
            .timeout(API_TIMEOUT)
            .send()
            .await?;
        let mut jar = self.cookies();
        for v in resp.headers().get_all(header::SET_COOKIE) {
            if let Ok(v) = v.to_str() {
                jar.add_set_cookie(v);
            }
        }
        let env: Envelope = resp.json().await?;
        if env.code != 0 {
            return Err(Error::Api {
                code: env.code,
                message: env.message,
            });
        }
        #[derive(Deserialize)]
        struct Poll {
            code: i64,
            #[serde(default)]
            url: String,
            #[serde(default)]
            message: String,
        }
        let poll: Poll = serde_json::from_value(env.data)?;
        Ok(match poll.code {
            0 => {
                // The redirect url carries the cookies as query parameters too.
                if !jar.has_session() {
                    if let Some((_, q)) = poll.url.split_once('?') {
                        for kv in q.split('&') {
                            if let Some((k, v)) = kv.split_once('=') {
                                if matches!(
                                    k,
                                    "SESSDATA" | "bili_jct" | "DedeUserID" | "DedeUserID__ckMd5"
                                ) {
                                    jar.set(k, v);
                                }
                            }
                        }
                    }
                }
                if !jar.has_session() {
                    return Err(Error::Unexpected(
                        "login succeeded but no session cookie received".into(),
                    ));
                }
                *self.cookies.write().unwrap() = jar.clone();
                self.session_valid
                    .store(true, std::sync::atomic::Ordering::Relaxed);
                QrStatus::Success(jar)
            }
            86101 => QrStatus::Waiting,
            86090 => QrStatus::Scanned,
            86038 => QrStatus::Expired,
            code => {
                return Err(Error::Api {
                    code,
                    message: poll.message,
                })
            }
        })
    }

    // ------------------------------------------------------------ videos

    /// Full video information (pages, owner, season ...).
    pub async fn view(&self, id: &VideoId) -> Result<VideoView> {
        let (k, v) = id.query_pair();
        self.get_json(
            &format!("{}/x/web-interface/view", self.api),
            &[(k, v)],
            false,
        )
        .await
    }

    /// Stream URLs for one page (`cid`) of a video. Asks for every stream
    /// type, so Dolby / Hi-Res audio is included when the account may use it.
    pub async fn playurl(&self, id: &VideoId, cid: u64) -> Result<PlayUrl> {
        let (k, v) = id.query_pair();
        let mut params = Self::params(&[
            (&k, v),
            ("cid", cid.to_string()),
            ("qn", "127".into()),
            ("fnver", "0".into()),
            ("fnval", FNVAL_ALL.to_string()),
            ("fourk", "1".into()),
        ]);
        if !self.is_logged_in() {
            // lets anonymous users get the higher "trial" qualities
            params.push(("try_look".into(), "1".into()));
        }
        match self
            .get_json(&format!("{}/x/player/wbi/playurl", self.api), &params, true)
            .await
        {
            Ok(p) => Ok(p),
            // Fall back to the legacy, unsigned endpoint.
            Err(Error::RiskControl { .. }) | Err(Error::Api { code: -403, .. }) => {
                self.get_json(&format!("{}/x/player/playurl", self.api), &params, false)
                    .await
            }
            Err(e) => Err(e),
        }
    }

    /// Background music recognised by bilibili for a video page, if any.
    pub async fn bgm_info(&self, id: &VideoId, cid: u64) -> Result<Option<BgmInfo>> {
        #[derive(Deserialize)]
        struct Player {
            #[serde(default)]
            bgm_info: Option<BgmInfo>,
        }
        let (k, v) = id.query_pair();
        let p: Player = self
            .get_json(
                &format!("{}/x/player/wbi/v2", self.api),
                &Self::params(&[(&k, v), ("cid", cid.to_string())]),
                true,
            )
            .await?;
        Ok(p.bgm_info.filter(|b| !b.song_title().is_empty()))
    }

    // ------------------------------------------------------------ lists

    /// All videos of a favorites folder (`media_id` / `fid`).
    pub async fn favorites(&self, media_id: u64) -> Result<VideoList> {
        #[derive(Deserialize)]
        struct Page {
            info: Info,
            #[serde(default)]
            medias: Option<Vec<Media>>,
            #[serde(default)]
            has_more: bool,
        }
        #[derive(Deserialize)]
        struct Info {
            #[serde(default)]
            title: String,
            #[serde(default)]
            upper: Owner,
        }
        #[derive(Deserialize)]
        struct Media {
            id: u64,
            #[serde(default, rename = "type")]
            kind: u32,
            #[serde(default)]
            title: String,
            #[serde(default)]
            bvid: String,
            #[serde(default)]
            attr: u32,
        }
        let mut list = VideoList {
            title: String::new(),
            owner: String::new(),
            items: Vec::new(),
        };
        for pn in 1.. {
            let page: Page = self
                .get_json(
                    &format!("{}/x/v3/fav/resource/list", self.api),
                    &Self::params(&[
                        ("media_id", media_id.to_string()),
                        ("pn", pn.to_string()),
                        ("ps", "20".into()),
                        ("platform", "web".into()),
                    ]),
                    false,
                )
                .await
                .map_err(|e| match e {
                    // code 0 without data: unknown id or a private folder
                    Error::Unexpected(_) | Error::Api { code: -403, .. } => Error::Api {
                        code: -404,
                        message: format!(
                            "favorites folder {media_id} not found or private (private folders need the owner's login)"
                        ),
                    },
                    e => e,
                })?;
            list.title = page.info.title;
            list.owner = page.info.upper.name;
            for m in page.medias.unwrap_or_default() {
                // type 2 = video; attr != 0 marks removed / invalid entries
                if m.kind == 2 && m.attr == 0 && !m.bvid.is_empty() {
                    list.items.push(ListItem {
                        aid: m.id,
                        bvid: m.bvid,
                        title: m.title,
                    });
                }
            }
            if !page.has_more || pn >= 500 {
                break;
            }
        }
        Ok(list)
    }

    /// All videos of a collection (合集) given its owner and season id.
    pub async fn collection(&self, mid: u64, season_id: u64) -> Result<VideoList> {
        #[derive(Deserialize)]
        struct Page {
            #[serde(default)]
            archives: Option<Vec<Archive>>,
            meta: Meta,
            page: PageInfo,
        }
        #[derive(Deserialize)]
        struct Archive {
            aid: u64,
            bvid: String,
            #[serde(default)]
            title: String,
        }
        #[derive(Deserialize)]
        struct Meta {
            #[serde(default)]
            name: String,
        }
        #[derive(Deserialize)]
        struct PageInfo {
            #[serde(default)]
            total: u64,
        }
        let mut list = VideoList {
            title: String::new(),
            owner: String::new(),
            items: Vec::new(),
        };
        for pn in 1.. {
            let page: Page = self
                .get_json(
                    &format!("{}/x/polymer/web-space/seasons_archives_list", self.api),
                    &Self::params(&[
                        ("mid", mid.to_string()),
                        ("season_id", season_id.to_string()),
                        ("sort_reverse", "false".into()),
                        ("page_num", pn.to_string()),
                        ("page_size", "30".into()),
                    ]),
                    false,
                )
                .await?;
            list.title = page.meta.name;
            let archives = page.archives.unwrap_or_default();
            let empty = archives.is_empty();
            list.items.extend(archives.into_iter().map(|a| ListItem {
                aid: a.aid,
                bvid: a.bvid,
                title: a.title,
            }));
            if empty || list.items.len() as u64 >= page.page.total || pn >= 500 {
                break;
            }
        }
        Ok(list)
    }

    // ------------------------------------------------------------ search

    /// Searches videos (20 results per page).
    pub async fn search_videos(
        &self,
        keyword: &str,
        opts: &crate::SearchOptions,
    ) -> Result<crate::SearchPage> {
        let mut params = Self::params(&[
            ("search_type", "video".into()),
            ("keyword", keyword.trim().to_owned()),
            ("page", opts.page.max(1).to_string()),
            ("order", opts.order.param().into()),
        ]);
        if opts.music_only {
            params.push(("tids".into(), "3".into()));
        }
        // A fresh session is sometimes answered with a captcha voucher and
        // no results; it clears up after a moment.
        for attempt in 0..4u64 {
            let raw: crate::search::RawSearch = self
                .get_json(
                    &format!("{}/x/web-interface/wbi/search/type", self.api),
                    &params,
                    true,
                )
                .await?;
            if raw.v_voucher.is_none() || raw.result.as_ref().is_some_and(|r| !r.is_empty()) {
                return Ok(raw.into_page());
            }
            tokio::time::sleep(Duration::from_millis(800 * (attempt + 1))).await;
        }
        Err(Error::RiskControl {
            code: -352,
            message: "search was answered with a captcha challenge; retry later or log in".into(),
        })
    }

    // ------------------------------------------------------------ helpers

    /// Follows a short link (`https://b23.tv/xxxx`) and returns the target.
    pub async fn resolve_short_url(&self, url: &str) -> Result<String> {
        let resp = self.http.get(url).timeout(API_TIMEOUT).send().await?;
        Ok(resp.url().to_string())
    }

    /// Downloads a small resource such as a cover image (tries `https://`
    /// first, then the url as given).
    pub async fn fetch_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let secure = normalize_url(url);
        match self.fetch_once(&secure).await {
            Err(_) if secure != url.trim() && !url.trim().starts_with("//") => {
                self.fetch_once(url.trim()).await
            }
            res => res,
        }
    }

    async fn fetch_once(&self, url: &str) -> Result<Vec<u8>> {
        let resp = self.http.get(url).timeout(API_TIMEOUT).send().await?;
        if !resp.status().is_success() {
            return Err(Error::Unexpected(format!(
                "HTTP {} for {url}",
                resp.status()
            )));
        }
        Ok(resp.bytes().await?.to_vec())
    }
}

/// Upgrades `http://` and protocol relative (`//i0.hdslb.com`) urls.
pub fn normalize_url(url: &str) -> String {
    let url = url.trim();
    if let Some(rest) = url.strip_prefix("//") {
        format!("https://{rest}")
    } else if let Some(rest) = url.strip_prefix("http://") {
        format!("https://{rest}")
    } else {
        url.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            normalize_url("http://i0.hdslb.com/a.jpg"),
            "https://i0.hdslb.com/a.jpg"
        );
        assert_eq!(
            normalize_url("//i0.hdslb.com/a.jpg"),
            "https://i0.hdslb.com/a.jpg"
        );
        assert_eq!(normalize_url("https://x"), "https://x");
    }

    // Live API tests; run with `cargo test -- --ignored` on a network that
    // can reach bilibili.
    #[tokio::test]
    #[ignore]
    async fn live_view_and_playurl() -> anyhow::Result<()> {
        let c = Client::new(CookieJar::new())?;
        let id = VideoId::Bvid("BV1Vh4y1v7qn".into());
        let view = c.view(&id).await?;
        assert!(!view.pages.is_empty());
        let play = c.playurl(&id, view.cid).await?;
        let streams = play.audio_streams();
        assert!(!streams.is_empty());
        let bgm = c.bgm_info(&id, view.cid).await?;
        assert_eq!(bgm.map(|b| b.song_title()).as_deref(), Some("一样的月光"));
        Ok(())
    }

    #[tokio::test]
    #[ignore]
    async fn live_season() -> anyhow::Result<()> {
        let c = Client::new(CookieJar::new())?;
        let view = c.view(&VideoId::Bvid("BV13m421J7fM".into())).await?;
        assert!(view.ugc_season.unwrap().episodes().count() > 0);
        let single = c.view(&VideoId::Bvid("BV1nr421t7KX".into())).await?;
        assert!(single.ugc_season.is_none());
        Ok(())
    }
}
