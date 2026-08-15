//! One error vocabulary for vault-backed entities.
//!
//! The slices had converged on two hand-copied templates —
//! `NotFound/Invalid/Backend` and `Io/BadRequest/NotFound/AlreadyExists`
//! — repeated across ~31 crates. `EntityError` is the union; a slice's
//! own public error type converts from it with a one-line `From` impl,
//! so wire contracts stay slice-specific while the store logic is shared.

use thiserror::Error;

/// Failure reading or writing an entity page.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum EntityError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("io: {0}")]
    Io(String),
}

impl EntityError {
    /// Wrap any displayable backend failure as [`EntityError::Io`].
    pub fn io(e: impl std::fmt::Display) -> Self {
        Self::Io(e.to_string())
    }

    /// The standard "id isn't a uuid" rejection.
    pub fn bad_id(e: impl std::fmt::Display) -> Self {
        Self::BadRequest(format!("id: {e}"))
    }
}

/// Failure turning a `VaultPage` into an entity.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ParseError {
    #[error("page has no frontmatter")]
    NoFrontmatter,
    #[error("frontmatter is not a YAML mapping")]
    NotAMapping,
    #[error("frontmatter parse: {0}")]
    Yaml(String),
    #[error("{0}")]
    Field(String),
}

/// Failure turning an entity back into markdown.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum WriteError {
    #[error("yaml: {0}")]
    Yaml(String),
    #[error("io: {0}")]
    Io(String),
    #[error("file exists at {0}; refusing to overwrite (pass overwrite=true)")]
    Exists(String),
    #[error("bad path: {0}")]
    BadPath(String),
}

/// Define a slice-local `from_entity_error` bridging [`EntityError`]
/// into the slice's own wire error.
///
/// A slice can't write `impl From<EntityError> for XError` itself —
/// both types are foreign to it, since the wire error lives in the
/// `*-proto` crate — so the conversion is a free function instead.
///
/// The four-variant form matches the `Io / BadRequest / NotFound /
/// AlreadyExists` shape most slices use:
///
/// ```ignore
/// vault_entity::entity_error_bridge!(BodyError);
/// ```
///
/// The three-variant form matches the `NotFound / Invalid / Backend`
/// shape used by the proto-first slices; `AlreadyExists` folds into
/// `Invalid` because those contracts have nowhere else to put it:
///
/// ```ignore
/// vault_entity::entity_error_bridge!(InboxError, invalid = Invalid, backend = Backend);
/// ```
#[macro_export]
macro_rules! entity_error_bridge {
    ($target:ty) => {
        /// Convert a shared store failure into this slice's wire error.
        fn from_entity_error(e: $crate::EntityError) -> $target {
            match e {
                $crate::EntityError::NotFound(s) => <$target>::NotFound(s),
                $crate::EntityError::BadRequest(s) => <$target>::BadRequest(s),
                $crate::EntityError::AlreadyExists(s) => <$target>::AlreadyExists(s),
                $crate::EntityError::Io(s) => <$target>::Io(s),
            }
        }
    };
    ($target:ty, invalid = $invalid:ident, backend = $backend:ident) => {
        /// Convert a shared store failure into this slice's wire error.
        fn from_entity_error(e: $crate::EntityError) -> $target {
            match e {
                $crate::EntityError::NotFound(s) => <$target>::NotFound(s),
                $crate::EntityError::BadRequest(s) => <$target>::$invalid {
                    field: "request".into(),
                    reason: s,
                },
                $crate::EntityError::AlreadyExists(s) => <$target>::$invalid {
                    field: "path".into(),
                    reason: format!("already exists: {s}"),
                },
                $crate::EntityError::Io(s) => <$target>::$backend { message: s },
            }
        }
    };
}

impl From<ParseError> for EntityError {
    fn from(e: ParseError) -> Self {
        Self::BadRequest(e.to_string())
    }
}

impl From<WriteError> for EntityError {
    fn from(e: WriteError) -> Self {
        match e {
            WriteError::Exists(p) => Self::AlreadyExists(p),
            WriteError::BadPath(p) => Self::BadRequest(p),
            other => Self::Io(other.to_string()),
        }
    }
}
