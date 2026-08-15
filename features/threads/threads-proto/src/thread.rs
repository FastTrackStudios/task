//! [`Thread`] — a topic/conversation container anchored to a host entity.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Kinds a thread can take. Stored as a `String` for tolerant decode of
/// values added later; the UI maps unknown kinds to a neutral rendering.
pub const THREAD_KINDS: &[&str] = &["discussion", "question", "decision", "action", "praise"];

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[architect(table_name = "threads_threads", repo)]
pub struct Thread {
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Owning organization (`architect_auth::AuthOrganization::id`).
    #[architect(filterable)]
    pub org_id: Uuid,

    /// Host entity kind — `"task"` | `"project"` today, extensible
    /// (`"forge_issue"`, `"pull_request"`, `"attachment"`, …). Together
    /// with [`Self::entity_id`] this is the polymorphic anchor.
    #[architect(filterable)]
    pub entity_type: String,

    /// Host entity id — the task/project frontmatter `id:` UUID.
    #[architect(filterable)]
    pub entity_id: Uuid,

    /// Topic / subject line of the conversation.
    #[architect(fulltext)]
    pub title: String,

    /// Thread kind — see [`THREAD_KINDS`].
    #[architect(filterable)]
    pub kind: String,

    #[architect(filterable)]
    pub resolved: bool,
    /// Who resolved it (`AuthUser::id`), if resolved.
    pub resolved_by: Option<Uuid>,

    // ── Conversation-level provenance (future ingestion) ──
    /// `"native"` for in-app threads; `"forge"` | `"email"` | `"imessage"`
    /// | `"matrix"` | … for ingested conversations.
    #[architect(filterable)]
    pub source_kind: String,
    /// Stable id in the source system (forge issue number, email
    /// thread-id / References root, chat room id). `None` for native.
    #[architect(filterable)]
    pub source_ref: Option<String>,
    /// Deep-link back to the original (issue URL, mailbox URI).
    pub source_url: Option<String>,

    /// Who opened the thread (`AuthUser::id`).
    pub created_by: Uuid,

    #[architect(filterable, sortable, exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Thread {
    type Key = uuid::Uuid;
    fn key(&self) -> uuid::Uuid {
        self.id
    }
}
