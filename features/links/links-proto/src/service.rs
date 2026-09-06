//! `LinksService` — the wire surface for the typed-link store.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::link::{Confidence, TypedLink};
use crate::node::NodeRef;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum LinksError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("bad request: {0}")]
    BadRequest(String),
    #[error("io: {0}")]
    Io(String),
}

/// Why a reference does or does not reach the thing it names.
///
/// Four outcomes rather than a boolean, because "no org answers to that
/// domain" and "the org is there and will not show you this" are
/// different answers to the same question, and a reader who cannot tell
/// them apart cannot act on either. `wiki_proto`'s `Unresolved` draws the
/// same distinction for pages, for the same reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum Reach {
    /// The reader's own org — every reference carrying no domain.
    Local,
    /// Another org, and the reader may read it: a membership row there,
    /// or a subscription to the source that publishes it.
    Reachable,
    /// No org on this deployment answers to that domain. Says nothing
    /// about whether the node exists somewhere else in the world.
    UnknownDomain,
    /// The org is known and does not admit this reader. Deliberately not
    /// distinguished from "the node is not there": telling an outsider
    /// which slugs exist is the enumeration a private source refuses.
    NotPermitted,
    /// The reader may look, and there is nothing at that address.
    NotFound,
}

/// One reference, and what became of resolving it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct ResolvedNode {
    /// The reference as asked, echoed so a batch answer can be zipped
    /// back onto its request without the caller tracking order.
    pub node: NodeRef,
    pub reach: Reach,
    /// The org that publishes it, when one was found. Empty otherwise —
    /// and empty for a local node too, whose org is the caller's own.
    #[serde(default)]
    #[facet(default)]
    pub org: String,
    /// A human label for the node, when the publisher offered one.
    #[serde(default)]
    #[facet(default)]
    pub title: String,
    /// Where the content sits in its org, relative to that org's root
    /// (`resources/charts/doxology.kf`). Empty when unresolved.
    #[serde(default)]
    #[facet(default)]
    pub rel_path: String,
}

impl ResolvedNode {
    /// A verdict carrying no content — the shape of every unhappy answer.
    #[must_use]
    pub fn refused(node: NodeRef, reach: Reach) -> Self {
        Self {
            node,
            reach,
            org: String::new(),
            title: String::new(),
            rel_path: String::new(),
        }
    }

    /// True when the caller may follow this reference.
    #[must_use]
    pub fn is_reachable(&self) -> bool {
        matches!(self.reach, Reach::Local | Reach::Reachable)
    }
}

#[architect::rpc]
pub trait LinksService {
    /// Create a link. The backend assigns `id` if empty and stamps
    /// `provenance.created_at` if unset. Returns the persisted link.
    fn create(&self, link: TypedLink) -> Result<TypedLink, LinksError>;

    /// Delete a link by id. `NotFound` if absent.
    fn delete(&self, id: &str) -> Result<(), LinksError>;

    /// One link by id.
    fn get(&self, id: &str) -> Result<TypedLink, LinksError>;

    /// Every link touching `node` (either endpoint).
    fn links_for(&self, node: NodeRef) -> Result<Vec<TypedLink>, LinksError>;

    /// The whole graph at/above a confidence threshold. When
    /// `include_private` is false, only `Unlisted`/`Public` links —
    /// i.e. the publishable, quality-filtered graph.
    fn graph(
        &self,
        min_confidence: Confidence,
        include_private: bool,
    ) -> Result<Vec<TypedLink>, LinksError>;

    /// Resolve references, including ones naming another org
    /// (ADR 0003) — what a setlist drawing on several libraries needs
    /// before it can draw itself.
    ///
    /// Batched because that is the shape of every real caller: a setlist
    /// resolves twenty items at once, and twenty round trips to learn
    /// which are reachable is a design that gets cached badly instead of
    /// being fixed. The answer is positional and total — one
    /// [`ResolvedNode`] per input, in order, with a [`Reach`] that is
    /// never an error, because one unreachable song must not fail the
    /// other nineteen.
    fn resolve_nodes(&self, nodes: Vec<NodeRef>) -> Result<Vec<ResolvedNode>, LinksError>;
}
