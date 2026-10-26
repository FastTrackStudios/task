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

    /// Host entity kind — `"task"` | `"project"` | `"doc"` (an app doc
    /// in the org's catalog, by its id) today, extensible
    /// (`"forge_issue"`, `"pull_request"`, `"attachment"`, …). Together
    /// with [`Self::entity_id`] this is the polymorphic anchor.
    #[architect(filterable)]
    pub entity_type: String,

    /// Host entity id — the task/project frontmatter `id:` UUID.
    #[architect(filterable)]
    pub entity_id: Uuid,

    /// A place inside the host entity the thread is about, as the
    /// entity's owner spells it: `entries/<uuid>`, `form/3`, a heading
    /// slug. Opaque to the threads feature; `None` is the entity as a
    /// whole. Segments are `/`-separated, so a listing can take a place
    /// and everything under it ([`crate::ThreadsService::list_threads_at`]).
    #[architect(filterable)]
    pub anchor_path: Option<String>,

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

/// An anchor path as stored: surrounding slashes and whitespace dropped,
/// and `None` when nothing is left.
#[must_use]
pub fn anchor_path(path: Option<&str>) -> Option<String> {
    path.map(|p| p.trim().trim_matches('/'))
        .filter(|p| !p.is_empty())
        .map(str::to_owned)
}

/// Whether a thread at `held` sits at `place` or under it. An empty
/// `place` holds everything.
#[must_use]
pub fn anchor_within(held: Option<&str>, place: &str) -> bool {
    let place = place.trim().trim_matches('/');
    place.is_empty()
        || held.is_some_and(|h| {
            h == place || (h.starts_with(place) && h.as_bytes().get(place.len()) == Some(&b'/'))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_is_trimmed_and_empty_is_none() {
        assert_eq!(anchor_path(Some("/entries/a/")), Some("entries/a".into()));
        assert_eq!(anchor_path(Some(" / ")), None);
        assert_eq!(anchor_path(None), None);
    }

    #[test]
    fn a_place_holds_itself_and_what_is_under_it() {
        assert!(anchor_within(Some("entries/a"), "entries/a"));
        assert!(anchor_within(Some("entries/a/notes"), "/entries/a/"));
        assert!(!anchor_within(Some("entries/ab"), "entries/a"));
        assert!(!anchor_within(None, "entries/a"));
        assert!(anchor_within(None, ""));
    }
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
