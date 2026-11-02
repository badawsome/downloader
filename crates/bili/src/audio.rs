//! Audio stream discovery and quality selection.

use std::fmt;
use std::str::FromStr;

use crate::models::PlayUrl;

/// Known bilibili audio qualities, ordered from worst to best.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AudioQuality {
    /// 64 kbps AAC (`30216`)
    K64,
    /// 132 kbps AAC (`30232`)
    K132,
    /// 192 kbps AAC (`30280`), the best quality without login
    K192,
    /// Dolby Atmos E-AC-3 (`30250`), needs login (usually VIP)
    Dolby,
    /// Hi-Res lossless FLAC (`30251`), needs VIP login
    HiRes,
}

impl AudioQuality {
    pub const ALL: [AudioQuality; 5] = [
        AudioQuality::K64,
        AudioQuality::K132,
        AudioQuality::K192,
        AudioQuality::Dolby,
        AudioQuality::HiRes,
    ];

    pub fn from_id(id: u32) -> Option<Self> {
        Some(match id {
            30216 => AudioQuality::K64,
            30232 => AudioQuality::K132,
            30280 => AudioQuality::K192,
            30250 => AudioQuality::Dolby,
            30251 => AudioQuality::HiRes,
            _ => return None,
        })
    }

    pub fn id(self) -> u32 {
        match self {
            AudioQuality::K64 => 30216,
            AudioQuality::K132 => 30232,
            AudioQuality::K192 => 30280,
            AudioQuality::Dolby => 30250,
            AudioQuality::HiRes => 30251,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AudioQuality::K64 => "64K AAC",
            AudioQuality::K132 => "132K AAC",
            AudioQuality::K192 => "192K AAC",
            AudioQuality::Dolby => "Dolby Atmos",
            AudioQuality::HiRes => "Hi-Res FLAC",
        }
    }

    /// Qualities above 192K are only served to logged in (VIP) accounts.
    pub fn requires_login(self) -> bool {
        self > AudioQuality::K192
    }
}

impl fmt::Display for AudioQuality {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for AudioQuality {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s.trim().to_ascii_lowercase().as_str() {
            "64" | "64k" => AudioQuality::K64,
            "132" | "132k" => AudioQuality::K132,
            "192" | "192k" => AudioQuality::K192,
            "dolby" | "atmos" => AudioQuality::Dolby,
            "hires" | "hi-res" | "flac" | "lossless" => AudioQuality::HiRes,
            other => {
                return match other.parse::<u32>().ok().and_then(AudioQuality::from_id) {
                    Some(q) => Ok(q),
                    None => Err(format!(
                        "unknown audio quality '{s}' (expected 64k, 132k, 192k, dolby, hires)"
                    )),
                }
            }
        })
    }
}

/// Where a stream comes from in the playurl response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamSource {
    /// `dash.audio`
    Dash,
    /// `dash.dolby.audio`
    Dolby,
    /// `dash.flac.audio`
    Flac,
    /// `durl`: progressive file containing video; audio is extracted later.
    Progressive,
}

/// A downloadable audio stream.
#[derive(Debug, Clone)]
pub struct AudioStream {
    pub id: u32,
    pub quality: Option<AudioQuality>,
    pub source: StreamSource,
    pub codecs: String,
    pub bandwidth: u64,
    /// Primary URL and mirrors, most reliable first.
    pub urls: Vec<String>,
    pub size: Option<u64>,
}

impl AudioStream {
    pub fn label(&self) -> String {
        match (self.quality, self.source) {
            (_, StreamSource::Progressive) => "progressive (audio extracted from video)".into(),
            (Some(q), _) => q.label().into(),
            (None, _) => format!("audio {} ({} kbps)", self.id, self.bandwidth / 1000),
        }
    }

    fn rank(&self) -> (i32, u64) {
        let tier = match (self.source, self.quality) {
            (StreamSource::Progressive, _) => -1,
            (_, Some(q)) => q as i32 + 1,
            (_, None) => 0,
        };
        (tier, self.bandwidth)
    }
}

/// Puts PCDN / MCDN mirrors (often slow or flaky) behind regular CDN hosts.
fn sort_urls(urls: &mut [String]) {
    fn penalty(u: &str) -> u8 {
        let host = u
            .split("://")
            .nth(1)
            .unwrap_or(u)
            .split('/')
            .next()
            .unwrap_or("");
        let ip_like = host
            .split(':')
            .next()
            .is_some_and(|h| h.chars().all(|c| c.is_ascii_digit() || c == '.'));
        if host.contains("mcdn") || host.contains("szbdyd") || ip_like || host.contains(':') {
            1
        } else {
            0
        }
    }
    urls.sort_by_key(|u| penalty(u));
}

impl PlayUrl {
    /// Every audio stream offered, best first.
    pub fn audio_streams(&self) -> Vec<AudioStream> {
        let mut out = Vec::new();
        if let Some(dash) = &self.dash {
            let mut push = |s: &crate::models::DashStream, source: StreamSource| {
                let mut urls = s.urls();
                if urls.is_empty() {
                    return;
                }
                sort_urls(&mut urls);
                out.push(AudioStream {
                    id: s.id,
                    quality: AudioQuality::from_id(s.id),
                    source,
                    codecs: s.codecs.clone(),
                    bandwidth: s.bandwidth,
                    urls,
                    size: s.size,
                });
            };
            for s in dash.audio.iter().flatten() {
                push(s, StreamSource::Dash);
            }
            if let Some(dolby) = &dash.dolby {
                for s in dolby.audio.iter().flatten() {
                    push(s, StreamSource::Dolby);
                }
            }
            if let Some(s) = dash.flac.as_ref().and_then(|f| f.audio.as_ref()) {
                push(s, StreamSource::Flac);
            }
        }
        // A single-segment progressive file can be demuxed as well.
        if let Some([durl]) = self.durl.as_deref() {
            let mut urls = vec![durl.url.clone()];
            urls.extend(durl.backup_url.iter().cloned());
            sort_urls(&mut urls);
            out.push(AudioStream {
                id: self.quality,
                quality: None,
                source: StreamSource::Progressive,
                codecs: String::new(),
                bandwidth: 0,
                urls,
                size: Some(durl.size).filter(|s| *s > 0),
            });
        }
        out.sort_by_key(|s| std::cmp::Reverse(s.rank()));
        out
    }
}

/// Which audio stream to pick.
#[derive(Debug, Clone, Copy)]
pub struct AudioPreference {
    /// Best quality allowed; `None` = no limit.
    pub max_quality: Option<AudioQuality>,
    /// Consider Dolby Atmos (E-AC-3, multichannel; not every player supports it).
    pub allow_dolby: bool,
    /// Consider Hi-Res FLAC.
    pub allow_hires: bool,
}

impl Default for AudioPreference {
    fn default() -> Self {
        Self {
            max_quality: None,
            allow_dolby: true,
            allow_hires: true,
        }
    }
}

impl AudioPreference {
    fn accepts(&self, s: &AudioStream) -> bool {
        match s.quality {
            Some(AudioQuality::Dolby) if !self.allow_dolby => false,
            Some(AudioQuality::HiRes) if !self.allow_hires => false,
            Some(q) => self.max_quality.is_none_or(|max| q <= max),
            None => true,
        }
    }

    /// Picks the best acceptable stream from `streams` (sorted best first,
    /// as returned by [`PlayUrl::audio_streams`]). Falls back to the
    /// worst offered stream when nothing satisfies the preference.
    pub fn select<'a>(&self, streams: &'a [AudioStream]) -> Option<&'a AudioStream> {
        streams
            .iter()
            .find(|s| self.accepts(s))
            .or_else(|| {
                streams
                    .iter()
                    .rev()
                    .find(|s| s.source != StreamSource::Progressive)
            })
            .or_else(|| streams.last())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::*;

    fn stream(id: u32, bw: u64) -> serde_json::Value {
        serde_json::json!({"id": id, "base_url": format!("https://upos-sz.bilivideo.com/{id}"),
            "backup_url": [format!("https://xy1x2x3x4xy.mcdn.bilivideo.cn:4483/{id}")], "bandwidth": bw, "codecs": "mp4a"})
    }

    fn playurl(login: bool) -> PlayUrl {
        let mut dash = serde_json::json!({
            "audio": [stream(30216, 67_000), stream(30280, 320_000), stream(30232, 132_000)],
            "dolby": {"type": 0, "audio": null},
            "flac": {"display": false, "audio": null},
        });
        if login {
            dash["dolby"] = serde_json::json!({"type": 1, "audio": [stream(30250, 448_000)]});
            dash["flac"] = serde_json::json!({"display": true, "audio": stream(30251, 1_500_000)});
        }
        serde_json::from_value(serde_json::json!({"quality": 80, "dash": dash})).unwrap()
    }

    #[test]
    fn anonymous_gets_best_aac() {
        let streams = playurl(false).audio_streams();
        let ids: Vec<u32> = streams.iter().map(|s| s.id).collect();
        assert_eq!(ids, vec![30280, 30232, 30216]);
        let best = AudioPreference::default().select(&streams).unwrap();
        assert_eq!(best.quality, Some(AudioQuality::K192));
        // regular CDN before mcdn mirror
        assert!(best.urls[0].contains("upos-sz"));
    }

    #[test]
    fn logged_in_prefers_lossless() {
        let streams = playurl(true).audio_streams();
        let pick = |p: AudioPreference| p.select(&streams).unwrap().quality.unwrap();
        assert_eq!(pick(AudioPreference::default()), AudioQuality::HiRes);
        assert_eq!(
            pick(AudioPreference {
                allow_hires: false,
                ..Default::default()
            }),
            AudioQuality::Dolby
        );
        assert_eq!(
            pick(AudioPreference {
                allow_hires: false,
                allow_dolby: false,
                ..Default::default()
            }),
            AudioQuality::K192
        );
        assert_eq!(
            pick(AudioPreference {
                max_quality: Some(AudioQuality::K132),
                ..Default::default()
            }),
            AudioQuality::K132
        );
    }

    #[test]
    fn progressive_fallback() {
        let p: PlayUrl = serde_json::from_value(serde_json::json!({
            "quality": 32, "durl": [{"order": 1, "size": 100, "url": "https://x/v.mp4", "backup_url": null}]
        }))
        .unwrap();
        let streams = p.audio_streams();
        assert_eq!(streams.len(), 1);
        assert_eq!(
            AudioPreference::default().select(&streams).unwrap().source,
            StreamSource::Progressive
        );
    }

    #[test]
    fn parse_quality() {
        assert_eq!(
            "HiRes".parse::<AudioQuality>().unwrap(),
            AudioQuality::HiRes
        );
        assert_eq!("192k".parse::<AudioQuality>().unwrap(), AudioQuality::K192);
        assert_eq!(
            "30250".parse::<AudioQuality>().unwrap(),
            AudioQuality::Dolby
        );
        assert!("320k".parse::<AudioQuality>().is_err());
        assert!(
            AudioQuality::HiRes > AudioQuality::Dolby && AudioQuality::Dolby > AudioQuality::K192
        );
    }
}
