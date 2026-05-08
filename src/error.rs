use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("sqlite error: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("invalid query: {0}")]
    InvalidQuery(String),

    #[error("invalid update: {0}")]
    InvalidUpdate(String),

    #[error("invalid identifier: {0}")]
    InvalidIdentifier(String),

    #[error("invalid index spec: {0}")]
    InvalidIndex(String),

    #[error("schema validation failed: {0}")]
    ValidationFailed(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("lock poisoned")]
    Poisoned,
}

pub type Result<T> = std::result::Result<T, Error>;
