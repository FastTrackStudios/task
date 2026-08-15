//! Session — one conversation. Mirrors Hermes's
//! `api/models.py:Session` shape (one-to-one with a thread,
//! no separate thread layer) and `CodexMonitor`'s
//! `ThreadSummary`.
//!
//! Carries: messages (typed via [`crate::message::Message`]),
//! tool-call audit trail, compression state, source tagging,
//! optional worktree backing, composer drafts. Most fields
//! are optional — external-monitor backends only fill what
//! they can observe from on-disk logs.

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Session {
    pub id: String,
    pub title: String,
    /// Which project this session belongs to.
    pub project_id: String,
    /// Which profile the session was started with. Locked
    /// at creation; changing it later requires migration.
    pub profile_id: String,
    /// Backend that dispatched the session.
    pub backend_id: String,
    /// Working directory the agent was invoked with
    /// (matches `Project.path` unless the session opened
    /// a worktree).
    pub workspace_path: String,
    /// Status of the session.
    pub status: SessionStatus,
    /// Where this session came from — useful when a single
    /// backend bridges multiple entry points.
    pub source: SourceTag,
    /// Optional sub-agent badge (for nested agent
    /// delegation). Empty when this is a top-level session.
    pub subagent_nickname: String,
    /// Pinned in the sidebar.
    pub pinned: bool,
    /// Archived (hidden from default list).
    pub archived: bool,
    /// Wall-clock timestamps.
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_message_at: Option<DateTime<Utc>>,
    /// Live-turn metadata. Populated while a turn is in
    /// flight; cleared on `Done` / `Error` / `Cancelled`.
    pub pending: Option<PendingTurn>,
    /// Token + cost accounting.
    pub usage: UsageStats,
    /// Compression state (for sessions long enough to
    /// trigger context trimming).
    pub compression: Option<CompressionState>,
    /// Worktree backing if this session runs in a git
    /// worktree.
    pub worktree: Option<WorktreeBacking>,
    /// Composer draft (auto-saved while the user types).
    pub composer_draft: ComposerDraft,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum SessionStatus {
    /// No turn in flight; awaiting user input.
    Idle,
    /// A turn is being processed by the backend.
    Running,
    /// Backend is awaiting an approval / question response.
    AwaitingUser,
    /// User cancelled the most recent turn mid-flight.
    Cancelled,
    /// Last turn ended with an error; subsequent dispatches
    /// allowed.
    Errored,
}

/// Provenance tag — where this session originated.
/// Cross-cuts backends; CLI sessions get imported and
/// stay flagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub enum SourceTag {
    /// Initiated from the Task UI.
    Webui,
    /// Imported from an external CLI session log.
    Cli,
    /// Originated via a messaging integration (Slack,
    /// Telegram, email).
    Messaging,
    /// Spawned by a scheduled job.
    Cron,
    /// Sub-agent delegated from another session.
    Tool,
    /// Created via the HTTP API.
    Api,
    /// Anything else.
    Other,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct PendingTurn {
    /// The user message text the agent is responding to.
    pub user_message: String,
    /// Attachments associated with the pending message.
    pub attachments: Vec<crate::attachment::AttachmentRef>,
    /// Stream id — clients subscribe to this to receive
    /// live events.
    pub stream_id: String,
    /// When the turn was kicked off.
    pub started_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Copy, PartialEq, Facet)]
#[repr(C)]
pub struct UsageStats {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    /// USD-equivalent rough estimate. `0.0` when the
    /// backend doesn't price.
    pub estimated_cost_usd: f32,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct CompressionState {
    /// Engine name (e.g. `"summarize-anchor"`,
    /// `"sliding-window"`, `"hermes-default"`).
    pub engine: String,
    /// Visible message index where compression begins.
    pub anchor_visible_idx: u32,
    /// Stable key of the anchor message (so re-renders
    /// remain stable as messages get appended).
    pub anchor_message_key: String,
    /// Compressed summary text (replaces older messages
    /// in the agent's context).
    pub anchor_summary: String,
    /// Configured context window in tokens.
    pub context_length: u32,
    /// Threshold at which compression kicks in.
    pub threshold_tokens: u32,
    /// Most recent prompt's actual token count.
    pub last_prompt_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct WorktreeBacking {
    /// Absolute path to the worktree.
    pub path: String,
    /// Branch checked out in the worktree.
    pub branch: String,
    /// Repo root the worktree was forked from.
    pub repo_root: String,
    /// When the worktree was created.
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ComposerDraft {
    pub text: String,
    pub attachments: Vec<crate::attachment::AttachmentRef>,
    pub updated_at: Option<DateTime<Utc>>,
}
