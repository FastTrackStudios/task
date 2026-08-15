//! Named agent identity — model + system prompt + tool /
//! MCP config. Mirrors Hermes's profile directory pattern:
//! `~/.hermes/profiles/<name>/config.yaml` carries the
//! per-profile state, and the `WebUI` swaps profiles via a
//! cookie (`hermes_profile=...`).
//!
//! In Task: profiles live under
//! `<state>/agent/profiles/<id>/` and are referenced by
//! sessions through [`session::Session::profile_id`]. A
//! session's profile is captured at session-creation time;
//! changing the profile later requires explicit migration.

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Profile {
    /// Stable id — `"default"`, `"coder"`, `"writer"`,
    /// `"hermes-personal"`, ... Lowercase + hyphens.
    pub id: String,
    /// Human-facing label.
    pub label: String,
    /// Which backend ([`crate::backend::AgentBackend::id`])
    /// this profile dispatches through.
    pub backend_id: String,
    /// Model selection (provider-namespaced, e.g.
    /// `"anthropic/claude-opus-4-7"`,
    /// `"openai/gpt-4o"`).
    pub model: ModelConfig,
    /// Active personality. Multiple personalities can be
    /// defined; only one is active at a time. Picked from
    /// `personalities[]`.
    pub active_personality: String,
    /// Available personalities. Empty = use a built-in
    /// "neutral" default.
    pub personalities: Vec<Personality>,
    /// Tool allow-list. Empty = backend default (typically
    /// all available tools).
    pub toolset: ToolsetConfig,
    /// MCP servers this profile mounts. Each entry is a
    /// backend-interpretable launch spec.
    pub mcp_servers: Vec<McpServerSpec>,
    /// When the profile was created.
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct ModelConfig {
    /// Provider-namespaced model id.
    pub id: String,
    /// Display name (`"Claude Opus 4.7"`).
    pub label: String,
    /// Reasoning effort hint:
    /// `"none" | "minimal" | "low" | "medium" | "high"`.
    /// Empty string = backend default.
    pub reasoning_effort: String,
    /// Provider-specific overrides (temperature, top-p,
    /// max tokens, ...). Opaque JSON.
    pub options_json: String,
}

/// One named system prompt + tone. A profile can carry
/// several (e.g. `"formal"`, `"casual"`, `"socratic"`)
/// and switch between them per session.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct Personality {
    pub id: String,
    pub label: String,
    /// The system prompt sent to the model. Sent verbatim.
    pub system_prompt: String,
    /// Optional tone tag, useful for UI.
    pub tone: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ToolsetConfig {
    /// Enabled toolset ids (`"fs"`, `"shell"`, `"web"`,
    /// `"task"`, `"wiki"`, ...). Empty = all available.
    pub enabled: Vec<String>,
    /// Disabled tools (overrides `enabled`). Useful for
    /// "all except shell" patterns.
    pub disabled: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct McpServerSpec {
    /// Server id (free-form; profile-scoped uniqueness).
    pub id: String,
    /// Launch command (e.g. `"python -m mcp_servers.fs"`).
    pub command: String,
    /// Env vars to set when launching.
    pub env: Vec<KvPair>,
    /// Resource roots the server is allowed to read.
    pub roots: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct KvPair {
    pub key: String,
    pub value: String,
}
