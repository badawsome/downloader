use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("future error: {0}")]
    FutureErr(String),
    #[error("io error: {0}")]
    IOError(#[from] std::io::Error),
    #[error("channel unexpected: {0}")]
    ChannelError(String),
    #[error("unexpected resp")]
    UnexpectedResp,
    #[error("api err,code: {0}, msg: {1}")]
    APIErr(i32, String),
    #[error("reqwest err: {0}")]
    ReqwestErr(#[from] reqwest::Error),
    #[error("unknown err: {0}")]
    Unknown(String),
}

pub type Result<T> = std::result::Result<T, Error>;
