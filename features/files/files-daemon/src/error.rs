pub use crate::service::DaemonError;

pub type Result<T> = std::result::Result<T, DaemonError>;

impl From<files_proto::FilesError> for DaemonError {
    fn from(e: files_proto::FilesError) -> Self {
        match e {
            files_proto::FilesError::NotFound(m) => DaemonError::NotFound(m),
            files_proto::FilesError::AlreadyExists(m) | files_proto::FilesError::BadRequest(m) => {
                DaemonError::BadRequest(m)
            }
            files_proto::FilesError::Io(m) => DaemonError::Io(m),
        }
    }
}

impl From<files_sync::SyncError> for DaemonError {
    fn from(e: files_sync::SyncError) -> Self {
        match e {
            files_sync::SyncError::NotFound(m) => DaemonError::NotFound(m),
            files_sync::SyncError::BadRequest(m) => DaemonError::BadRequest(m),
            files_sync::SyncError::Io(m) => DaemonError::Io(m),
        }
    }
}
