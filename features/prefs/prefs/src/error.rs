//! Internal error type. Converts cleanly into the proto's
//! [`prefs_proto::PrefsError`] at the trait boundary.

use prefs_proto::PrefsError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum PrefsDbError {
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),

    #[error("invalid: {0}")]
    Invalid(String),
}

impl From<PrefsDbError> for PrefsError {
    fn from(value: PrefsDbError) -> Self {
        match value {
            PrefsDbError::Invalid(msg) => PrefsError::Invalid(msg),
            db @ PrefsDbError::Db(_) => PrefsError::Backend(db.to_string()),
        }
    }
}
