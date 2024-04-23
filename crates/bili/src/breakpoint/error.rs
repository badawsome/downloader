use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("parse failed {0}")]
    Parse(#[from] serde_json::Error),
    #[error("io failed {0}")]
    IO(#[from] std::io::Error),
    #[error("unknown err: {0}")]
    Unknown(String),
}

pub type Result<T> = std::result::Result<T, Error>;
