use std::io;

/// Errors returned by this crate's own APIs (repo setup, checkpointing,
/// chain derivation). Errors from inside the [`jj_lib::backend::Backend`]
/// impl itself are reported as `jj_lib::backend::BackendError` per that
/// trait's contract, not through this type.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] io::Error),

    #[error("chunk store error: {0}")]
    ChunkStore(#[from] crate::chunk::Error),

    #[error("malformed version-store object: {0}")]
    Object(String),

    #[error("unknown object: {0}")]
    UnknownObject(String),

    #[error("invalid repo path {0:?}: {1}")]
    InvalidPath(String, String),

    #[error("jj-lib backend error: {0}")]
    Backend(#[from] jj_lib::backend::BackendError),

    #[error("jj-lib repo error: {0}")]
    Repo(String),
}

pub type Result<T> = std::result::Result<T, Error>;
