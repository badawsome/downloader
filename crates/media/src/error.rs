use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid media file: {0}")]
    Invalid(String),
    #[error("no audio track found in media file")]
    NoAudioTrack,
    #[error("unsupported media: {0}")]
    Unsupported(String),
}

impl Error {
    pub(crate) fn invalid(msg: impl Into<String>) -> Self {
        Error::Invalid(msg.into())
    }
}

pub type Result<T> = std::result::Result<T, Error>;
