//! The link record itself. Mirrors the fields the existing
//! `task-core::EmailRef` carries, but decoupled from any
//! particular entity feature — we just store
//! `(message_id, entity_ref, linked_at, linked_by, tags)`.

use crate::entity::EntityRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// One link from a Message-ID to an entity. Many-to-many: a
/// message can link to many entities, an entity can list many
/// messages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmailLink {
    /// RFC 2822 Message-ID, stored with or without angle
    /// brackets. The `bare_message_id` helper normalizes.
    pub message_id: String,
    pub entity: EntityRef,
    /// When the link was recorded. `None` for legacy rows that
    /// pre-date the column.
    pub linked_at: Option<DateTime<Utc>>,
    /// Who made the link. `"user"` for manual, `"jarvis"` (or
    /// any bot name) for auto-linked, `"rule"` for filter-driven.
    pub linked_by: Option<String>,
    /// Free-form user tags layered on top of whatever the mail
    /// client tagged the message with. Stored as JSON in the
    /// row; deserialized into `Vec<String>` here.
    pub user_tags: Vec<String>,
}

impl EmailLink {
    /// Strip angle brackets from `message_id` if present.
    /// Helpers and callers can rely on this to compare ids
    /// without worrying about wrapping.
    #[must_use]
    pub fn bare_message_id(&self) -> &str {
        bare_message_id(&self.message_id)
    }
}

pub fn bare_message_id(s: &str) -> &str {
    s.strip_prefix('<')
        .and_then(|s| s.strip_suffix('>'))
        .unwrap_or(s)
}
