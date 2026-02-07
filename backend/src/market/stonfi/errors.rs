use thiserror::Error;

#[derive(Error, Debug)]
pub enum StonfiError {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("invalid response from ston.fi")]
    InvalidResponse,

    #[error("numeric parse error: {0}")]
    ParseInt(#[from] std::num::ParseIntError),
}
