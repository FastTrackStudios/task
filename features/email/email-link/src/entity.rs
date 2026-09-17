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

/// One linkable thing — an (org, kind, id) triple. The id is
/// opaque to this crate; consumers decide what it means (UUID,
/// vault path, slug, etc).
///
/// **`org` is what lets a personal mailbox serve every org.** Mail
/// belongs to a person, projects belong to organisations, and the
/// two do not live in the same vault. Without a qualifier a link
/// could only ever name something in the org holding the link
/// store, which would mean either scattering copies of one mailbox
/// across every org or giving up on filing mail against shared
/// work. Naming the org instead keeps the links in the one place
/// that is private to their owner while still pointing anywhere.
///
/// This mirrors the decision ADR 0003 already took for the general
/// link graph, where a node reference may name another
/// organisation. Email links were built as a parallel, unqualified
/// id space; this is them joining it.
///
/// Empty means "the org this link store belongs to", which is what
/// every row written before the column existed meant.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EntityRef {
    #[serde(default)]
    pub org: String,
    pub kind: EntityKind,
    pub id: String,
}

impl EntityRef {
    pub fn new(kind: EntityKind, id: impl Into<String>) -> Self {
        Self {
            org: String::new(),
            kind,
            id: id.into(),
        }
    }

    /// The same reference, qualified to an organisation.
    #[must_use]
    pub fn in_org(mut self, org: impl Into<String>) -> Self {
        self.org = org.into();
        self
    }

    pub fn task(id: impl Into<String>) -> Self {
        Self::new(EntityKind::task(), id)
    }
    pub fn project(id: impl Into<String>) -> Self {
        Self::new(EntityKind::project(), id)
    }
}
