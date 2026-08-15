//! The thing on the other side of the link. We deliberately
//! keep `EntityKind` an open string + an opaque id rather than a
//! sealed enum — the link layer doesn't need to know what a
//! "task" or "project" actually is, and other features can add
//! new entity kinds without touching this crate.

use serde::{Deserialize, Serialize};

/// What kind of thing the email is linked to. Free-form so
/// other features can extend without coordinating: `"task"`,
/// `"project"`, `"note"`, `"person"`, `"meeting"`, etc. Stored
/// lowercase by convention.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityKind(pub String);

impl EntityKind {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into().to_lowercase())
    }
    #[must_use]
    pub fn task() -> Self {
        Self("task".into())
    }
    #[must_use]
    pub fn project() -> Self {
        Self("project".into())
    }
    #[must_use]
    pub fn note() -> Self {
        Self("note".into())
    }
    #[must_use]
    pub fn person() -> Self {
        Self("person".into())
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// One linkable thing — a (kind, id) pair. The id is opaque to
/// this crate; consumers decide what it means (UUID, vault path,
/// slug, etc).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    pub kind: EntityKind,
    pub id: String,
}

impl EntityRef {
    pub fn new(kind: EntityKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
        }
    }

    pub fn task(id: impl Into<String>) -> Self {
        Self::new(EntityKind::task(), id)
    }
    pub fn project(id: impl Into<String>) -> Self {
        Self::new(EntityKind::project(), id)
    }
}
