//! Discovery — what a backend can do, for UIs to render pickers
//! and panels from live data instead of hardcoded lists.
//!
//! Backends answer from their own source of truth: the Hermes
//! backend proxies its gateway's `/v1/models` + `/v1/skills` +
//! `/v1/capabilities`; Codex reports its static model set. A
//! router merges across backends, tagging each row with its
//! `backend_id`.

use crate::error::AgentError;
use facet::Facet;

/// One selectable model, as reported by a backend.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct ModelInfo {
    /// Backend that serves it (`"hermes"`, `"codex"`).
    pub backend_id: String,
    /// Id to pass as `DispatchTurn.model_override`.
    pub id: String,
    /// Display label; falls back to `id` when empty.
    pub label: String,
    /// True for the backend's default model.
    pub is_default: bool,
    /// Context window in tokens; `0` = unknown (UIs hide the
    /// context gauge's percentage and show raw counts).
    pub context_length: u64,
    /// Catalog provider id (`"openai"`, `"anthropic"`) — groups the
    /// picker; empty = the backend itself is the provider.
    pub provider_id: String,
    /// Display name for the provider; falls back to `provider_id`.
    pub provider_name: String,
    /// The model exposes a reasoning/thinking mode.
    pub reasoning: bool,
    /// Cost per million input tokens in USD; `0.0` = unknown/free.
    pub cost_in_per_mtok: f64,
    /// Cost per million output tokens in USD; `0.0` = unknown/free.
    pub cost_out_per_mtok: f64,
}

/// One agent skill (Hermes's self-improving skill library).
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct SkillInfo {
    pub backend_id: String,
    pub name: String,
    pub description: String,
    /// Whether the skill is currently enabled/loadable.
    pub enabled: bool,
}

/// Feature flags a backend reports (shape mirrors Hermes's
/// `/v1/capabilities`, flattened to labeled booleans so the UI can
/// list them without knowing the backend).
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct CapabilityFlag {
    pub backend_id: String,
    pub name: String,
    pub enabled: bool,
}

pub use crate::backend::BackendHealth;

#[architect::rpc]
pub trait Discovery {
    /// Models across all configured backends (or one, when
    /// `backend_id` is non-empty).
    fn list_models(&self, backend_id: &str) -> Result<Vec<ModelInfo>, AgentError>;

    /// Skills across all configured backends. Backends without a
    /// skill system return an empty list.
    fn list_skills(&self, backend_id: &str) -> Result<Vec<SkillInfo>, AgentError>;

    /// Capability flags across all configured backends.
    fn list_capabilities(&self, backend_id: &str) -> Result<Vec<CapabilityFlag>, AgentError>;

    /// Live health per backend — Hermes answers from its gateway's
    /// `/health/detailed` (state, connected platforms, in-flight
    /// agents), local backends report themselves. Never fails for
    /// an unreachable backend: that's a row with `reachable: false`
    /// whose `status_text` says *why* the agent isn't answering.
    fn backend_health(&self, backend_id: &str) -> Result<Vec<BackendHealth>, AgentError>;
}
