//! This crate's own error types. [`Error`] is what the backend's internals
//! fail with (repo open/init, registry I/O, live-tree scans); every lane
//! maps it onto the wire's [`files_proto::FilesFault`].
//!
//! [`FilesError`] is the coarse four-way error the backend's synchronous
//! store seams (`sync_*`, `read_source_content`, `with_version_store`)
//! report to the in-process crates built on them — `files-sync`, the
//! daemon. It is not on any wire.

/// The coarse error of the backend's in-process store seams.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FilesError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

impl From<FilesError> for files_proto::error::FilesFault {
    fn from(err: FilesError) -> Self {
        use files_proto::error::FilesFault as F;
        match err {
            FilesError::NotFound(m) | FilesError::BadRequest(m) => F::Invalid(m),
            FilesError::AlreadyExists(m) => F::AlreadyRoot(m),
            FilesError::Io(m) => F::Io(m),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("registry json: {0}")]
    /// A stored-JSON failure, as text. `files` no longer speaks serde
    /// at rest — facet's serializer and deserializer have distinct error
    /// types, and neither belongs in a variant that outlives the codec.
    #[allow(clippy::doc_markdown)]
    Json(String),
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
            Error::Json(e) => F::Internal(e),
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
