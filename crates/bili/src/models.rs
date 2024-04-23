use std::collections::HashMap;

use derive_builder::Builder;
use derive_getters::Getters;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize, Getters, Builder)]
pub struct SeasonList {
    season_id: u64,
    season_name: String,
    owner: Owner,
    sections: Vec<BasicView>,
}

#[derive(Debug, Clone, Deserialize, Getters)]
pub struct BasicView {
    aid: u64,
    bvid: String,
    cid: u64,
    title: String,
}

#[derive(Debug, Clone, Deserialize, Getters)]
pub struct View {
    aid: u64,
    bvid: String,
    cid: u64,
    title: String,
    owner: Owner,
    #[serde(rename = "pic")]
    pic_url: String,
    is_season_display: Option<bool>,
    season_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum VideoId {
    AID(u64),
    BVID(String),
}

impl std::fmt::Display for VideoId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoId::AID(id) => write!(f, "{}", id),
            VideoId::BVID(id) => write!(f, "{}", id),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Getters)]
pub struct Owner {
    #[serde(rename = "mid")]
    uid: u64,
    name: String,
    #[serde(rename = "face")]
    face_url: String,
}

#[derive(Debug, Deserialize, Getters)]
pub struct VideoMetadata {
    // aid: u64,
    cid: u64,
    #[serde(rename = "part")]
    title: String,
}

#[derive(Debug, Deserialize, Getters)]
pub struct DownloadInfo {
    // accept_quality: Vec<u32>,
    pub durl: Vec<DurlInfo>,
}

#[derive(Debug, Clone, Deserialize, Getters)]
pub struct DurlInfo {
    size: u64,
    url: String,
}

#[derive(Debug, Getters)]
pub struct DownloadParam {
    pub info: DurlInfo,
    pub chunk_size: Option<u64>,
    pub conn_pool: Option<u8>,
}

#[derive(Debug)]
pub struct GetDownloadInfoParam {
    pub id: VideoId,
    pub cid: u64,
    pub clarity: Clarity,
}

impl GetDownloadInfoParam {
    pub(crate) fn get_query(&self) -> HashMap<&str, String> {
        let mut mp = HashMap::new();
        // --- deal with static
        mp.insert("fnver", "0".to_owned());
        // --- deal with id
        match self.id {
            VideoId::AID(id) => mp.insert("avid", id.to_string()),
            VideoId::BVID(ref id) => mp.insert("bvid", id.clone()),
        };
        // --- deal with cid
        mp.insert("cid", self.cid.to_string());
        // --- deal with clarity
        match self.clarity {
            Clarity::High => {
                // TODO login
                mp.insert("fnval", "1".to_owned());
                mp.insert("qn", "112".to_owned());
            }
            Clarity::Low => {
                mp.insert("fnval", "1".to_owned());
                mp.insert("qn", "16".to_owned());
            }
            Clarity::Default => {
                mp.insert("fnval", "1".to_owned());
                mp.insert("qn", "16".to_owned());
            }
        };
        mp
    }
}

#[derive(Debug)]
pub enum Clarity {
    High,
    Low,
    Default,
}

#[derive(Debug, Clone)]
pub enum Protocol {
    HTTP,
    HTTPS,
}

impl Default for Protocol {
    fn default() -> Self {
        Protocol::HTTPS
    }
}

impl Protocol {
    pub fn get_prefix(&self) -> &str {
        match self {
            Protocol::HTTP => "http://",
            Protocol::HTTPS => "https://",
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(bound = "T: serde::de::DeserializeOwned")]
pub struct PackInfo<T: serde::de::DeserializeOwned> {
    code: i32,
    message: String,
    data: Option<T>,
}

impl<T> PackInfo<T>
where
    T: serde::de::DeserializeOwned,
{
    pub fn as_result(self) -> super::Result<T> {
        match self.code {
            0 => self.data.ok_or(super::Error::UnexpectedResp),
            code => Err(super::Error::APIErr(code, self.message)),
        }
    }
}
