//! Slim domain trait — the anchored reads + writes the UI / CLI / agents
//! drive.
//!
//! Plain per-entity CRUD is the architect-emitted `ThreadRepo` /
//! `MessageRepo`. This trait carries what that generic surface can't yet
//! express cleanly: listing threads by their `(entity_type, entity_id)`
//! anchor and messages by `thread_id` (architect's generic `Repo::list`
//! filter is still a raw-string placeholder), plus the convenience writes
//! that stamp provenance. Owned-`String` params throughout — the
//! convention `#[architect::rpc]`'s async dispatcher emits cleanly.

use chrono::{DateTime, Utc};
use uuid::Uuid;

use crate::error::ThreadsError;
use crate::message::Message;
use crate::thread::Thread;

#[architect::rpc]
pub trait ThreadsService {
    /// Every thread anchored to `(entity_type, entity_id)` — e.g. all
    /// conversations on a given task or project. Newest first.
    async fn list_threads(
        &self,
        entity_type: String,
        entity_id: Uuid,
    ) -> Result<Vec<Thread>, ThreadsError>;

    async fn get_thread(&self, id: Uuid) -> Result<Thread, ThreadsError>;

    /// Open a new thread. The backend assigns `id` + timestamps.
    async fn create_thread(&self, req: CreateThreadRequest) -> Result<Thread, ThreadsError>;

    /// All messages in a thread, oldest first (by `posted_at`).
    async fn list_messages(&self, thread_id: Uuid) -> Result<Vec<Message>, ThreadsError>;

    /// Append a message to a thread. The agent/human write path; the
    /// request carries provenance so ingested messages keep their origin.
    async fn post_message(&self, req: PostMessageRequest) -> Result<Message, ThreadsError>;

    /// Flip a thread's resolved flag.
    async fn set_resolved(
        &self,
        thread_id: Uuid,
        resolved: bool,
        by: Option<Uuid>,
    ) -> Result<Thread, ThreadsError>;

    /// Delete a thread and its messages (cascade). Idempotent.
    async fn delete_thread(&self, id: Uuid) -> Result<(), ThreadsError>;

    /// Delete a single message. Idempotent.
    async fn delete_message(&self, id: Uuid) -> Result<(), ThreadsError>;
}

/// Args for [`ThreadsService::create_thread`].
#[derive(::facet::Facet, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct CreateThreadRequest {
    pub org_id: Uuid,
    /// `"task"` | `"project"` | … — the host entity kind.
    pub entity_type: String,
    pub entity_id: Uuid,
    pub title: String,
    /// See [`crate::thread::THREAD_KINDS`]. Empty ⇒ backend defaults to
    /// `"discussion"`.
    pub kind: String,
    pub created_by: Uuid,
    /// `"native"` for in-app; an integration name when ingested.
    pub source_kind: String,
    pub source_ref: Option<String>,
    pub source_url: Option<String>,
}

/// Args for [`ThreadsService::post_message`].
#[derive(::facet::Facet, serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[repr(C)]
pub struct PostMessageRequest {
    pub thread_id: Uuid,
    pub org_id: Uuid,
    pub author_id: Option<Uuid>,
    pub author_label: String,
    pub body: String,
    pub reply_to: Option<Uuid>,
    /// `"native"` for in-app; an integration name when ingested.
    pub source_kind: String,
    pub external_id: Option<String>,
    pub original_text: Option<String>,
    pub source_url: Option<String>,
    /// Source timestamp. `None` ⇒ now.
    pub posted_at: Option<DateTime<Utc>>,
}
