//! Derivations — cached per-message computed facts (triage).
//!
//! Keyed `(account, message_id, kind, version)`: the account
//! scopes the store (odysseus's hard-won cross-tenant lesson —
//! Message-IDs are global, a newsletter shares one Message-ID
//! across every recipient), the version stamps the computing
//! code so bumping it invalidates stale rows.
//!
//! v1 kinds are heuristic-only (`urgency`, `tags`); the LLM
//! kinds (summary, draft reply, fold boundaries) arrive when an
//! engine implements `DerivationEngine::derive_llm` in
//! `email-product`.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Version of the derivation-computing code. Bump to invalidate
/// every cached row (they're recomputed lazily by the triage
/// pass).
pub const DERIVATION_VERSION: u32 = 1;

/// The fixed v1 tag taxonomy (payload values of
/// [`DerivationKind::Tags`]).
pub const TAG_TAXONOMY: [&str; 7] = [
    "action-needed",
    "waiting",
    "newsletter",
    "receipt",
    "calendar",
    "social",
    "other",
];

/// What was derived.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum DerivationKind {
    /// How urgent the message is, 0–3. Payload = the score as a
    /// decimal string (`"0"`…`"3"`).
    Urgency,
    /// Triage tags from [`TAG_TAXONOMY`]. Payload =
    /// comma-separated tag names.
    Tags,
}

impl DerivationKind {
    #[must_use]
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Urgency => "urgency",
            Self::Tags => "tags",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "urgency" => Self::Urgency,
            "tags" => Self::Tags,
            _ => return None,
        })
    }
}

/// One cached derivation row.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct Derivation {
    pub message_id: String,
    pub kind: DerivationKind,
    /// The [`DERIVATION_VERSION`] the row was computed under.
    pub version: u32,
    /// Kind-specific payload — see [`DerivationKind`].
    pub payload: String,
}

impl Derivation {
    /// Convenience: the urgency score, when this is an urgency
    /// row with a well-formed payload.
    #[must_use]
    pub fn urgency(&self) -> Option<u8> {
        (self.kind == DerivationKind::Urgency)
            .then(|| self.payload.parse().ok())
            .flatten()
    }

    /// Convenience: the tag list, when this is a tags row.
    #[must_use]
    pub fn tags(&self) -> Vec<&str> {
        if self.kind != DerivationKind::Tags {
            return Vec::new();
        }
        self.payload
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect()
    }
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Derivation, DerivationKind};
    unsafe impl vox_types::Reborrow for DerivationKind {
        type Ref<'a> = DerivationKind;
    }
    unsafe impl vox_types::Reborrow for Derivation {
        type Ref<'a> = Derivation;
    }
}
