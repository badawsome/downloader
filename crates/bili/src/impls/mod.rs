mod downloader;
mod music;
mod video;
mod season;

pub use music::*;
pub use video::*;
pub use season::*;

use super::*;

#[derive(Debug, Default, Clone)]
pub struct Service<'a> {
    api_host: &'a str,
    protocol: Protocol,
    client: reqwest::Client,
}

impl<'a> Service<'a> {
    pub fn new() -> Self {
        Self {
            api_host: consts::HOST,
            protocol: Protocol::HTTPS,
            client: reqwest::Client::new(),
        }
    }
}