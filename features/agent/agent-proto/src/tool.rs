//! Tool calls — distilled from messages into a query-able
//! audit trail. Each `ToolCall` corresponds to one
//! `ContentBlock::ToolUse` + the matching `ToolResult` block.
//! Backends typically maintain both representations so the UI
//! can render tool history independently of the conversation
//! transcript (matches `CodexMonitor`'s `ConversationItem::Tool`).

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct ToolCall {
    pub id: String,
    pub session_id: String,
    /// Message that issued the call.
    pub message_id: String,
    pub name: String,
    /// Optional category (`"fs"`, `"shell"`, `"web"`,
    /// `"wiki"`, ...). Backends pick this from the tool's
    /// own metadata.
    pub category: String,
    /// JSON-encoded args.
    pub input_json: String,
    /// Optional short summary the agent supplied
    /// alongside the call (`"List repo files"`).
    pub title: String,
    /// Status of the call.
    pub status: ToolStatus,
    /// JSON-encoded result. Empty until status is
    /// `Done` or `Error`.
    pub output_json: String,
    /// Free-form preview line (rendered while the tool
    /// runs).
    pub preview: String,
    /// File-change manifest if the tool modified the
    /// workspace.
    pub changes: Vec<FileChange>,
    /// How long the tool ran. `0` while pending.
    pub duration_ms: u32,
    pub started_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
    /// Sub-agent provenance — set when one agent
    /// delegated to another via this tool call.
    pub collab: Option<CollabRouting>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum ToolStatus {
    Pending,
    InProgress,
    Done,
    Error,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct FileChange {
    /// Vault-relative path.
    pub path: String,
    pub kind: FileChangeKind,
    /// Unified diff. Empty for non-text changes.
    pub diff: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum FileChangeKind {
    Create,
    Write,
    Delete,
    Rename,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct CollabRouting {
    /// Session id that initiated the delegation.
    pub sender_session_id: String,
    /// Receiving session id(s). Multi-receiver supports
    /// fan-out delegations.
    pub receiver_session_ids: Vec<String>,
}
