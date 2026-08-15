//! Internal error type. Converts into the proto's
//! [`threads_proto::ThreadsError`] at the trait boundary.

use thiserror::Error;
use threads_proto::ThreadsError;

#[derive(Debug, Error)]
pub enum ThreadsDbError {
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),

    #[error("not found: {kind} {id}")]
    NotFound { kind: &'static str, id: String },

    #[error("invalid: {0}")]
    Invalid(String),
}

impl From<ThreadsDbError> for ThreadsError {
    fn from(value: ThreadsDbError) -> Self {
        match value {
            ThreadsDbError::NotFound { kind: "thread", id } => ThreadsError::ThreadNotFound(id),
            ThreadsDbError::NotFound {
                kind: "message",
                id,
            } => ThreadsError::MessageNotFound(id),
            ThreadsDbError::NotFound { id, .. } => ThreadsError::ThreadNotFound(id),
            ThreadsDbError::Invalid(msg) => ThreadsError::Invalid(msg),
            ThreadsDbError::Db(e) => ThreadsError::Backend(e.to_string()),
        }
    }
}
