//! The typed-link model: a link is a first-class object carrying its own
//! relation type, confidence, visibility, and provenance.
//!
//! Vocabularies follow the knowledge-primitives research:
//! SKOS topic relations + argument-mapping epistemic relations +
//! Breadcrumbs hierarchy + scripture-specific kinds; Gwern/Kesselman
//! ordinal confidence; opt-in (private-default) visibility.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::node::NodeRef;

/// What kind of relationship the link asserts (directional; each has a
/// conceptual inverse via [`Relation::inverse`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum Relation {
    // ── associative / topic (SKOS) ──
    Related,
    Broader,
    Narrower,
    // ── structure (Breadcrumbs) ──
    Up,
    Down,
    Next,
    Prev,
    // ── definitional ──
    Defines,
    InstanceOf,
    ExampleOf,
    // ── epistemic (argument mapping) ──
    Supports,
    Refutes,
    Cites,
    // ── scripture-specific ──
    CrossRef,
    Quotes,
    Fulfills,
    Mentions,
    Tagged,
    /// An indirect echo — a lyric, note, or text evokes a verse / concept
    /// without quoting it. The dominant relation for worship lyrics and
    /// literary allusion (where [`Self::Quotes`] is a verbatim citation).
    AlludesTo,
}

impl Relation {
    /// The inverse relation (for traversing the edge the other way).
    /// Symmetric relations return themselves.
    #[must_use]
    pub fn inverse(self) -> Self {
        match self {
            Self::Broader => Self::Narrower,
            Self::Narrower => Self::Broader,
            Self::Up => Self::Down,
            Self::Down => Self::Up,
            Self::Next => Self::Prev,
            Self::Prev => Self::Next,
            // Symmetric / no distinct inverse modeled yet.
            other => other,
        }
    }

    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Related => "related",
            Self::Broader => "broader",
            Self::Narrower => "narrower",
            Self::Up => "up",
            Self::Down => "down",
            Self::Next => "next",
            Self::Prev => "prev",
            Self::Defines => "defines",
            Self::InstanceOf => "instance-of",
            Self::ExampleOf => "example-of",
            Self::Supports => "supports",
            Self::Refutes => "refutes",
            Self::Cites => "cites",
            Self::CrossRef => "cross-ref",
            Self::Quotes => "quotes",
            Self::Fulfills => "fulfills",
            Self::Mentions => "mentions",
            Self::Tagged => "tagged",
            Self::AlludesTo => "alludes-to",
        }
    }
}

/// How sure we are about a node or link — an ordinal scale (Gwern /
/// Kesselman). Variant order is ascending so `>=` filtering works
/// (`Certain > Likely > … > Speculative`).
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Facet,
)]
#[repr(u8)]
pub enum Confidence {
    /// A hunch / unstructured thought.
    Speculative,
    Unlikely,
    Possible,
    Likely,
    /// Established / strictly proven.
    Certain,
}

/// Publishing visibility. Opt-in: `Private` is the safe default, so
/// nothing leaks unless explicitly promoted.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Facet,
)]
#[repr(u8)]
pub enum Visibility {
    Private,
    /// Reachable by link but not listed/indexed.
    Unlisted,
    Public,
}

/// Where a link came from — for trust and for the publish/provenance
/// graph (nanopublication shape).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet, Default)]
pub struct Provenance {
    /// ISO-8601 UTC timestamp (string on the wire — `chrono` stays a
    /// consumer concern).
    pub created_at: String,
    /// Who/what asserted it (user id, agent, dataset name). Empty = unknown.
    pub created_by: String,
    /// A citation / source reference (e.g. `openbible-crossref`). Empty = none.
    pub source_ref: String,
    /// Auto-derived (from a dataset/algorithm) vs human-asserted.
    pub derived: bool,
}

/// One typed link in the knowledge graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct TypedLink {
    /// Stable id (uuid string). Empty on create ⇒ the backend assigns one.
    pub id: String,
    pub source: NodeRef,
    pub target: NodeRef,
    pub relation: Relation,
    pub confidence: Confidence,
    pub visibility: Visibility,
    pub provenance: Provenance,
    /// Optional free-text annotation on the link.
    pub note: String,
}

impl TypedLink {
    /// A new private, human-asserted link with the given confidence.
    #[must_use]
    pub fn new(
        source: NodeRef,
        target: NodeRef,
        relation: Relation,
        confidence: Confidence,
    ) -> Self {
        Self {
            id: String::new(),
            source,
            target,
            relation,
            confidence,
            visibility: Visibility::Private,
            provenance: Provenance::default(),
            note: String::new(),
        }
    }

    /// Does this link touch `node` (either endpoint)?
    #[must_use]
    pub fn touches(&self, node: &NodeRef) -> bool {
        &self.source == node || &self.target == node
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::NodeRef;

    #[test]
    fn confidence_orders_for_filtering() {
        assert!(Confidence::Certain > Confidence::Likely);
        assert!(Confidence::Likely > Confidence::Speculative);
        // "only links >= likely"
        let keep = [
            Confidence::Certain,
            Confidence::Likely,
            Confidence::Possible,
        ]
        .into_iter()
        .filter(|c| *c >= Confidence::Likely)
        .count();
        assert_eq!(keep, 2);
    }

    #[test]
    fn relation_inverses() {
        assert_eq!(Relation::Broader.inverse(), Relation::Narrower);
        assert_eq!(Relation::Supports.inverse(), Relation::Supports); // symmetric for now
    }

    #[test]
    fn link_touches_either_end() {
        let a = NodeRef::verse("John.3.16");
        let b = NodeRef::verse("Romans.5.8");
        let l = TypedLink::new(a.clone(), b.clone(), Relation::CrossRef, Confidence::Likely);
        assert!(l.touches(&a) && l.touches(&b));
        assert!(!l.touches(&NodeRef::verse("Genesis.1.1")));
        assert_eq!(l.visibility, Visibility::Private); // opt-in default
    }
}
