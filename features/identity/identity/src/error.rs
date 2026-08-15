//! Internal error type for the identity locker store.

use auth::crypto::SecretBoxError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum IdentityDbError {
    #[error("db: {0}")]
    Db(#[from] sea_orm::DbErr),

    #[error("crypto: {0}")]
    Crypto(#[from] SecretBoxError),

    #[error("invalid: {0}")]
    Invalid(String),
}
