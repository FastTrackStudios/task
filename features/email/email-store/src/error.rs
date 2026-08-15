use thiserror::Error;

pub type Result<T, E = StoreError> = std::result::Result<T, E>;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("schema: {0}")]
    Schema(String),
    #[error("parse: {0}")]
    Parse(String),
    #[error("outbox entry {0} not found")]
    OutboxNotFound(u64),
    #[error("outbox entry {id}: invalid transition from {from} via {op}")]
    OutboxTransition {
        id: u64,
        from: &'static str,
        op: &'static str,
    },
}
