use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("chunk store: {0}")]
    ChunkStore(#[from] files_store::chunk::Error),
    #[error("rendition index: {0}")]
    Index(String),
    #[error("transcode: {0}")]
    Transcode(String),
    #[error("not media: {0}")]
    NotMedia(String),
}

pub type Result<T> = std::result::Result<T, Error>;
