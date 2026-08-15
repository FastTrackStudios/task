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
}
