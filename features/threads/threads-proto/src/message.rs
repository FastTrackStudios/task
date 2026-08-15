//! [`Message`] — one entry in a [`crate::Thread`].

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[architect(table_name = "threads_messages", repo)]
pub struct Message {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Parent thread (`Thread::id`).
    #[architect(filterable)]
    pub thread_id: Uuid,

    /// Owning organization — denormalized from the thread for cheap
    /// org-scoped queries.
    #[architect(filterable)]
    pub org_id: Uuid,

    /// Resolved local user (`AuthUser::id`). `None` for an ingested
    /// message whose author couldn't be matched to a local account —
    /// the human-readable original stays in [`Self::author_label`].
    pub author_id: Option<Uuid>,
    /// Display name of the author (`"Cody"`, `"alice@example.com"`).
    pub author_label: String,

    /// Rendered message body (markdown).
    #[architect(fulltext)]
    pub body: String,

    /// In-thread reply target (forge / chat reply chains). `None` = top.
    #[architect(filterable)]
    pub reply_to: Option<Uuid>,

    // ── Per-message provenance / original-text linkage ──
    /// `"native"` | `"forge"` | `"email"` | … (mirrors the thread).
    #[architect(filterable)]
    pub source_kind: String,
    /// Per-message external id (email `Message-ID`, forge comment id).
    #[architect(filterable)]
    pub external_id: Option<String>,
    /// Verbatim original payload, untouched — the paper trail back to the
    /// ingested source even after `body` is normalized.
    pub original_text: Option<String>,
    pub source_url: Option<String>,

    /// Source timestamp (when the message was originally sent). For
    /// native posts = creation time.
    #[architect(filterable, sortable)]
    pub posted_at: DateTime<Utc>,

    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Message {
    type Key = uuid::Uuid;
    fn key(&self) -> uuid::Uuid {
        self.id
    }
}
