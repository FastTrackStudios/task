//! Session lifecycle. Backends that maintain conversations
//! implement this; UIs use it for sidebar listings + per-
//! session CRUD.

use crate::attachment::AttachmentRef;
use crate::error::AgentError;
use crate::session::Session;
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct CreateSession {
    pub project_id: String,
    pub profile_id: String,
    /// Target backend id (`"codex"`, `"hermes"`). Empty ⇒ the
    /// server's default backend. Routers use this to pick which
    /// backend owns the new session; single-backend servers ignore
    /// it.
    pub backend_id: String,
    /// Optional title. Empty ⇒ backend auto-generates.
    pub title: String,
    /// Override workspace path (defaults to project.path).
    pub workspace_path: String,
    /// Optional sub-agent nickname.
    pub subagent_nickname: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct SessionFilter {
    pub project_id: String,
    pub backend_id: String,
    pub profile_id: String,
    pub include_archived: bool,
    pub only_pinned: bool,
    /// Result cap. `0` = backend default.
    pub limit: u32,
    /// Opaque pagination cursor.
    pub cursor: String,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct SessionPage {
    pub sessions: Vec<Session>,
    pub next_cursor: String,
    pub has_more: bool,
}

#[architect::rpc]
pub trait Sessions {
    fn create_session(&self, args: CreateSession) -> Result<Session, AgentError>;
    fn read_session(&self, session_id: &str) -> Result<Session, AgentError>;
    fn list_sessions(&self, filter: SessionFilter) -> Result<SessionPage, AgentError>;
    fn rename_session(&self, session_id: &str, title: &str) -> Result<Session, AgentError>;
    fn pin_session(&self, session_id: &str, pinned: bool) -> Result<Session, AgentError>;
    fn archive_session(&self, session_id: &str, archived: bool) -> Result<Session, AgentError>;
    fn delete_session(&self, session_id: &str) -> Result<(), AgentError>;
    /// Persist a composer draft. Auto-called as the user
    /// types. Returns the updated session.
    fn save_composer_draft(
        &self,
        session_id: &str,
        text: &str,
        attachments: Vec<AttachmentRef>,
    ) -> Result<Session, AgentError>;
}
