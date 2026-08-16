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

impl From<files_domain::cadence::Error> for Error {
    fn from(err: files_domain::cadence::Error) -> Self {
        match err {
            files_domain::cadence::Error::Io(e) => Self::Io(e),
            files_domain::cadence::Error::Json(e) => Self::Json(e),
        }
    }
}

/// Onto the v2 fault type.
///
/// The v1 `FilesError` had four `String` variants, so this was lossy by
/// construction. `FilesFault` carries what a caller branches on, and the
/// variants that still arrive as prose are the ones that genuinely are —
/// an io error and a jj backend failure describe themselves.
impl From<Error> for files_proto::error::FilesFault {
    fn from(err: Error) -> Self {
        use files_proto::error::FilesFault as F;
        match err {
            Error::Io(e) => F::Io(e.to_string()),
            Error::Json(e) => F::Internal(e.to_string()),
            Error::VersionStore(e) => F::Store(e.to_string()),
            Error::JjBackend(e) => F::Store(e.to_string()),
            Error::Repo(m) => F::Store(m),
            Error::NotFound(m) => F::Invalid(m),
            Error::AlreadyExists(m) => F::AlreadyRoot(m),
            Error::BadRequest(m) => F::Invalid(m),
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
