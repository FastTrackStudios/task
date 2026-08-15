use thiserror::Error;

pub type Result<T, E = LinkError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum LinkError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
