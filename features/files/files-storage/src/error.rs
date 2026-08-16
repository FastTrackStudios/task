//! Mapping the failures this crate's dependencies produce onto the wire
//! error, [`files_storage_proto::StorageError`].
//!
//! There is deliberately no second, internal error enum. There used to
//! be one mirroring all seven domain variants of the wire type with
//! identical `String` payloads, which meant a new variant had to be
//! added in three places and every method paid a 13-arm translation on
//! the way out (PR #284 review). `StorageCore` returns the wire type
//! directly; these helpers are the only translation left.

use files_storage_proto::StorageError;
use files_store::PathError;

/// I/O failure, with the operation that caused it.
pub fn io(context: &str, err: impl std::fmt::Display) -> StorageError {
    StorageError::Io(format!("{context}: {err}"))
}

/// A confinement refusal. A rejected or escaping path is a bad request —
/// the caller asked for something it may not have — while an underlying
/// I/O fault is reported as one.
pub fn path(err: PathError) -> StorageError {
    match err {
        PathError::Io(e) => StorageError::Io(e.to_string()),
        other => StorageError::BadRequest(other.to_string()),
    }
}

/// Version-store / jj failure.
pub fn store(err: impl std::fmt::Display) -> StorageError {
    StorageError::Io(format!("version store: {err}"))
}

/// The panic mapper for [`files_store::blocking`].
pub fn panicked(message: String) -> StorageError {
    StorageError::Io(format!("blocking task panicked: {message}"))
}

pub type Result<T> = std::result::Result<T, StorageError>;
