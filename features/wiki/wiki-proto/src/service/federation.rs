//! Multi-wiki peer federation.

use crate::error::WikiError;
use crate::federation::{CrossWikiPageRef, PeerPullResult, PeerWiki};
use chrono::{DateTime, Utc};

#[architect::rpc]
pub trait Federation {
    fn add_peer(&self, wiki_id: &str, peer: PeerWiki) -> Result<(), WikiError>;
    fn remove_peer(&self, wiki_id: &str, peer_id: &str) -> Result<(), WikiError>;
    fn list_peers(&self, wiki_id: &str) -> Result<Vec<PeerWiki>, WikiError>;
    fn pull_from_peer(
        &self,
        wiki_id: &str,
        peer_id: &str,
        since: Option<DateTime<Utc>>,
    ) -> Result<PeerPullResult, WikiError>;
    fn resolve_cross_wiki_link(
        &self,
        wiki_id: &str,
        link: &str,
    ) -> Result<Option<CrossWikiPageRef>, WikiError>;
}
