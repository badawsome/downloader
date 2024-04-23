mod dep;
mod error;

pub use error::*;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

pub struct ProgressManager<'a> {
    list: BreakpointList,
    client: std::sync::Arc<super::Service<'a>>,
    get_url_rx: mpsc::Receiver<&'a super::VideoId>,
    get_url_tx: mpsc::Sender<&'a super::VideoId>,
    download_rx: mpsc::Receiver<&'a str>,
    download_tx: mpsc::Sender<&'a str>,
}

pub type BreakpointList = Vec<Breakpoint>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Breakpoint {
    id: super::VideoId,
    remain_ranges: Vec<std::ops::Bound<u64>>,
    url: Option<String>,
}


pub struct ProgressManagerBuilder {
    
}

impl<'a> ProgressManager<'a> {
    pub async fn load_from_file<'b>(c: &'b super::Service<'b>, f: tokio::fs::File) -> Result<Self> {
        let list = serde_json::from_reader(f.into_std().await)?;
        Ok(ProgressManager {
            list,
            client: std::sync::Arc::new(c.clone()),
            get_url_rx: todo!(),
        })
    }

    // pub async fn write_to_file(&self, mut f: tokio::fs::File) -> Result<()> {
    //     f.write_all(serde_json::json!(self).to_string().as_bytes())
    //         .await?;
    //     f.sync_all().await?;
    //     Ok(())
    // }

    pub fn new(c: &'a super::Service) -> Self {
        Self {
            list: Vec::new(),
            client: std::sync::Arc::new(c.clone()),
            get_url_ch: todo!(),
        }
    }

    // pub fn append_id(id: &super::VideoId) {

    // }

    // pub fn append_id_with_cid(id: &super::VideoId, cid: u64) {

    // }
}
