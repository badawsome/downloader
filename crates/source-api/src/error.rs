use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("network error: {0}")]
    Http(#[from] reqwest::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("unexpected response: {0}")]
    Json(#[from] serde_json::Error),
    #[error("source API error {code}: {message}")]
    Api { code: i64, message: String },
    #[error("login required (code -101); run `dc login` first")]
    NotLoggedIn,
    #[error("request blocked by source risk control ({code}: {message}); retry later or log in")]
    RiskControl { code: i64, message: String },
    #[error("unexpected response: {0}")]
    Unexpected(String),
    #[error("invalid id or url: {0}")]
    InvalidId(String),
}

impl Error {
    /// Content does not exist / was removed / is not visible.
    pub fn is_not_found(&self) -> bool {
        matches!(self, Error::Api { code, .. } if matches!(code, -404 | 62002 | 62004 | 62012))
    }
}

pub type Result<T> = std::result::Result<T, Error>;
