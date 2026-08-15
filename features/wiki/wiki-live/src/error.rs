use thiserror::Error;

#[derive(Debug, Error)]
pub enum WikiLiveError {
    #[error("not bootstrapped — call WikiLive::bootstrap first")]
    NotBootstrapped,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("yaml: {0}")]
    Yaml(String),

    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("illegal state: {0}")]
    IllegalState(String),

    #[error("path {path} is under the immutable raw layer; record_pages rejects it")]
    RawIsImmutable { path: String },

    #[error("path {path} escapes the wiki root")]
    PathEscape { path: String },

    #[error("ingest task not found: {0}")]
    TaskNotFound(String),
}
