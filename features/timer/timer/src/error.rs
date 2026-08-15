//! Internal error type. Converts cleanly into the proto's
//! [`timer_proto::TimerError`] at the trait boundary.

use thiserror::Error;
use timer_proto::TimerError;

#[derive(Debug, Error)]
pub enum TimerDbError {
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),

    #[error("not found: {kind} {id}")]
    NotFound { kind: &'static str, id: String },

    /// Another open session already exists for this user.
    #[error("timer already running for user: {user_id}")]
    AlreadyRunning { user_id: String },

    #[error("no active timer for user: {user_id}")]
    NoActiveTimer { user_id: String },

    #[error("currency mismatch: {0}")]
    CurrencyMismatch(String),

    #[error("invalid: {0}")]
    Invalid(String),

    #[error("backend: {0}")]
    Backend(String),
}

impl From<TimerDbError> for TimerError {
    fn from(value: TimerDbError) -> Self {
        match value {
            TimerDbError::NotFound { kind: "client", id } => TimerError::ClientNotFound(id),
            TimerDbError::NotFound {
                kind: "work_session",
                id,
            } => TimerError::WorkSessionNotFound(id),
            TimerDbError::NotFound { kind: "tag", id } => TimerError::TagNotFound(id),
            TimerDbError::NotFound { id, .. } => TimerError::WorkSessionNotFound(id),
            TimerDbError::AlreadyRunning { user_id } => TimerError::AlreadyRunning(user_id),
            TimerDbError::NoActiveTimer { user_id } => TimerError::NoActiveTimer(user_id),
            TimerDbError::CurrencyMismatch(msg) => TimerError::CurrencyMismatch { message: msg },
            TimerDbError::Invalid(msg) => TimerError::Invalid(msg),
            other => TimerError::Backend(other.to_string()),
        }
    }
}
