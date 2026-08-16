//! This crate's own error type (repo open/init, registry I/O, live-tree
//! scans). [`FilesBackend`](crate::FilesBackend) maps every variant onto
//! [`files_proto::FilesError`] at the RPC boundary — see
//! `backend::to_files_error`.

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("registry json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("version store: {0}")]
    VersionStore(#[from] files_store::version::Error),
    /// A failure reported by a jj backend itself — this crate talks to
    /// both Root flavors' backends through the `Backend` trait (ADR
    /// 0001: media on the CAS store, software on stock git), so their
    /// errors arrive as `BackendError` rather than through the
    /// version-store crate's own type.
    #[error("jj backend: {0}")]
    JjBackend(#[from] jj_lib::backend::BackendError),
    #[error("jj repo: {0}")]
    Repo(String),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
}

pub type Result<T> = std::result::Result<T, Error>;
