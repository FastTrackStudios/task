//! `AgentRef` — who did the thing.
//!
//! Every workflow event records an actor. The actor is either
//! a human (identified by their user id in the org's auth
//! system) or an agent (identified by name + the model version
//! that produced the action). The split matters for two
//! reasons:
//!
//! - **Audit.** "What did claude-opus-4-7 do in cycle 12?" is
//!   a real query the user runs. The model version is part of
//!   the identity because the same agent name with a different
//!   model behaves differently.
//! - **Capability boundaries.** The orchestrator gates verbs
//!   by `AgentRef`; a `review-bot` agent can't call `merge`
//!   even though both verbs flow through the same trait.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Who performed a workflow action.
///
/// Serialized as a tagged enum:
///
/// ```yaml
/// actor:
///   kind: agent
///   name: claude-opus-4-7
///   model_version: claude-opus-4-7
/// ```
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize,
)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[repr(u8)]
pub enum AgentRef {
    /// A real human, identified by their auth-system user id.
    /// `display_name` is the rendered form if the audit UI
    /// wants to show a friendly label without an extra lookup.
    Human {
        user_id: String,
        display_name: Option<String>,
    },

    /// A non-human actor. `name` is the agent slug
    /// (`claude-opus-4-7`, `codex-gpt-5`, `triage-bot`).
    /// `model_version` is the underlying LLM identifier when
    /// the agent is LLM-backed; rule-based agents can leave it
    /// `None`.
    Agent {
        name: String,
        model_version: Option<String>,
    },
}

impl AgentRef {
    /// Convenience: build a human ref with no display name.
    pub fn human(user_id: impl Into<String>) -> Self {
        Self::Human {
            user_id: user_id.into(),
            display_name: None,
        }
    }

    /// Convenience: build an agent ref without a model version.
    pub fn agent(name: impl Into<String>) -> Self {
        Self::Agent {
            name: name.into(),
            model_version: None,
        }
    }

    /// Convenience: build an agent ref with a model version.
    pub fn agent_versioned(name: impl Into<String>, model: impl Into<String>) -> Self {
        Self::Agent {
            name: name.into(),
            model_version: Some(model.into()),
        }
    }

    /// `true` if this is a non-human actor — used by the
    /// orchestrator to apply different rate limits / RBAC
    /// rules to agents vs people.
    #[must_use]
    pub fn is_agent(&self) -> bool {
        matches!(self, Self::Agent { .. })
    }

    /// Short, log-friendly label. Humans render as their user
    /// id; agents render as `name@version` (or just `name` if
    /// no version is known).
    #[must_use]
    pub fn short_label(&self) -> String {
        match self {
            Self::Human { user_id, .. } => user_id.clone(),
            Self::Agent {
                name,
                model_version: Some(v),
            } => format!("{name}@{v}"),
            Self::Agent {
                name,
                model_version: None,
            } => name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_label_renders_each_case() {
        assert_eq!(AgentRef::human("cody").short_label(), "cody");
        assert_eq!(AgentRef::agent("triage-bot").short_label(), "triage-bot");
        assert_eq!(
            AgentRef::agent_versioned("claude", "opus-4-7").short_label(),
            "claude@opus-4-7"
        );
    }

    #[test]
    fn round_trips_through_json() {
        let a = AgentRef::agent_versioned("claude", "opus-4-7");
        let s = serde_json::to_string(&a).unwrap();
        let b: AgentRef = serde_json::from_str(&s).unwrap();
        assert_eq!(a, b);
    }
}
