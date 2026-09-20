//! Which other servers this org can reach, and with what.
//!
//! The subscriber's half of a cross-server subscription, and the mirror
//! of [`crate::source_grants`]: that one is what this org hands out,
//! this one is what it has been handed. Two facts per source, and they
//! arrive together because neither is any use alone:
//!
//! - **Where the publishing domain is.** A subscription names its source
//!   by domain, and a domain is a *name* rather than an address
//!   (`wiki.ref.redirect`) — `LocalOrgs` resolves one by looking for an
//!   org of that name on this disk, and a publisher on another server is
//!   not on this disk. So the domain has to resolve to something
//!   dialable, and the thing this deployment dials is an endpoint id.
//! - **The secret the publisher minted** with `grant_source_read`.
//!   Presented on every call back, exactly as an `Offer`'s secret is on
//!   the files lane; holding it is the whole of this org's authority
//!   there.
//!
//! # Why the subscriber writes this down at all
//!
//! Because the publisher cannot. Carrying a grant to the subscriber is
//! "a message, not a protocol" — the stance `FederationService::offer`
//! takes and the reason `grant_source_read` returns an inert string
//! rather than delivering anything. Somebody copies two facts across a
//! boundary; this is where they land.
//!
//! # Where it lives, and why not in `subscriptions.json`
//!
//! Beside the subscriptions, not inside them. A subscription is a thing
//! a person chose and a thing the vault's sync client carries around; a
//! secret is a credential, and putting one in a file that syncs is how a
//! credential ends up somewhere nobody meant it to be. So this sits in
//! the org's own `.state` directory next to `granted.json`, which holds
//! the other half of the same relationship for the same reason.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wiki_proto::subscription::SourceKind;

/// `"<kind>/<slug>"` — how a source is keyed within a domain.
///
/// Kind *and* slug, for the reason [`crate::source_grants`] keys the
/// same way: the tiers are separate namespaces, and a secret for a shelf
/// called `songs` must not present itself for a wiki of that name.
fn key(kind: SourceKind, slug: &str) -> String {
    format!("{}/{slug}", kind.tier_dir())
}

/// One publishing domain: where it is, and what this org holds for it.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Peer {
    /// The endpoint id to dial. A bare id — no host, no port — because
    /// that is the whole of what a deployment is given and the whole of
    /// what it needs (`files.peering.by-id`).
    endpoint: String,
    /// `"<kind>/<slug>"` → the secret minted for it.
    secrets: BTreeMap<String, String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Peers(BTreeMap<String, Peer>);

/// This org's peer table.
pub struct SourcePeers {
    path: PathBuf,
}

impl SourcePeers {
    /// Open the table for an org root. Nothing is read until asked.
    #[must_use]
    pub fn open(org_root: &Path) -> Self {
        Self {
            path: org_root
                .join("subscribed")
                .join(".state")
                .join("peers.json"),
        }
    }

    fn load(&self) -> Peers {
        // Unreadable is treated as empty, and the consequence is that no
        // domain resolves: a refresh reports the source unreachable and
        // the copy already on disk goes on reading (`wiki.life.orphan`).
        // That is the safe direction — the alternative would be dialling
        // somewhere on the strength of a file that would not parse.
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, peers: &Peers) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(peers).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, bytes)
    }

    /// Record a grant: this domain is at this endpoint, and this source
    /// there is readable with this secret.
    ///
    /// Idempotent per source, and last-write-wins on the endpoint — a
    /// domain is at one place, so a second grant naming a different
    /// endpoint is a correction rather than a second peer.
    ///
    /// # Errors
    ///
    /// A failure writing the table.
    pub fn trust(
        &self,
        domain: &str,
        endpoint: &str,
        kind: SourceKind,
        slug: &str,
        secret: &str,
    ) -> std::io::Result<()> {
        let mut peers = self.load();
        let peer = peers.0.entry(domain.to_owned()).or_default();
        peer.endpoint = endpoint.to_owned();
        peer.secrets.insert(key(kind, slug), secret.to_owned());
        self.save(&peers)
    }

    /// Forget one source's secret, and the peer with it once its last
    /// source is gone.
    ///
    /// Forgetting what was never held succeeds, so a cleanup that runs
    /// twice is harmless. The copy on disk is untouched: what ends is
    /// the refreshing, which is the same thing a revocation on the
    /// publisher's side ends.
    ///
    /// # Errors
    ///
    /// A failure writing the table.
    pub fn distrust(&self, domain: &str, kind: SourceKind, slug: &str) -> std::io::Result<()> {
        let mut peers = self.load();
        let Some(peer) = peers.0.get_mut(domain) else {
            return Ok(());
        };
        if peer.secrets.remove(&key(kind, slug)).is_none() {
            return Ok(());
        }
        if peer.secrets.is_empty() {
            peers.0.remove(domain);
        }
        self.save(&peers)
    }

    /// The endpoint this domain is at, when this org has been told.
    #[must_use]
    pub fn endpoint(&self, domain: &str) -> Option<String> {
        self.load()
            .0
            .get(domain)
            .map(|p| p.endpoint.clone())
            .filter(|e| !e.is_empty())
    }

    /// The secret for one source, when this org holds one.
    #[must_use]
    pub fn secret(&self, domain: &str, kind: SourceKind, slug: &str) -> Option<String> {
        self.load()
            .0
            .get(domain)?
            .secrets
            .get(&key(kind, slug))
            .cloned()
            .filter(|s| !s.is_empty())
    }

    /// Every source this org has been granted, without the secrets.
    ///
    /// The secrets stay here deliberately. A member asking "what can we
    /// reach" is asking about relationships, not for a credential they
    /// could carry somewhere else — and the publisher's side answers the
    /// same question the same way: `granted.json` is never read back
    /// over the wire either.
    #[must_use]
    pub fn trusted(&self) -> Vec<(String, String, SourceKind, String)> {
        let mut out = Vec::new();
        for (domain, peer) in self.load().0 {
            for source in peer.secrets.keys() {
                let Some((tier, slug)) = source.split_once('/') else {
                    continue;
                };
                let Some(kind) = SourceKind::from_tier_dir(tier) else {
                    continue;
                };
                out.push((domain.clone(), peer.endpoint.clone(), kind, slug.to_owned()));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peers() -> (tempfile::TempDir, SourcePeers) {
        let dir = tempfile::tempdir().unwrap();
        let peers = SourcePeers::open(dir.path());
        (dir, peers)
    }

    #[test]
    fn an_empty_table_resolves_nothing_and_is_not_an_error() {
        let (_dir, peers) = peers();
        assert_eq!(peers.endpoint("vnt.test"), None);
        assert_eq!(peers.secret("vnt.test", SourceKind::Wiki, "cut"), None);
        assert!(peers.trusted().is_empty());
    }

    #[test]
    fn a_secret_is_scoped_to_one_source_on_one_domain() {
        let (_dir, peers) = peers();
        peers
            .trust("vnt.test", "endpoint-1", SourceKind::Wiki, "cut", "s3cret")
            .unwrap();
        assert_eq!(
            peers.secret("vnt.test", SourceKind::Wiki, "cut").as_deref(),
            Some("s3cret")
        );
        // Another kind with the same slug, another slug, another domain:
        // three different sources, and the secret is for one of them.
        assert_eq!(peers.secret("vnt.test", SourceKind::Assets, "cut"), None);
        assert_eq!(peers.secret("vnt.test", SourceKind::Wiki, "reel"), None);
        assert_eq!(peers.secret("acme.test", SourceKind::Wiki, "cut"), None);
    }

    #[test]
    fn a_second_grant_corrects_the_endpoint_rather_than_adding_a_peer() {
        let (_dir, peers) = peers();
        peers
            .trust("vnt.test", "moved-from", SourceKind::Wiki, "cut", "a")
            .unwrap();
        peers
            .trust("vnt.test", "moved-to", SourceKind::Wiki, "reel", "b")
            .unwrap();
        assert_eq!(peers.endpoint("vnt.test").as_deref(), Some("moved-to"));
        assert_eq!(peers.trusted().len(), 2);
    }

    #[test]
    fn distrusting_the_last_source_forgets_the_peer() {
        let (_dir, peers) = peers();
        peers
            .trust("vnt.test", "endpoint-1", SourceKind::Wiki, "cut", "a")
            .unwrap();
        peers.distrust("vnt.test", SourceKind::Wiki, "cut").unwrap();
        assert_eq!(peers.endpoint("vnt.test"), None);
        // And doing it again is not an error.
        peers.distrust("vnt.test", SourceKind::Wiki, "cut").unwrap();
    }

    #[test]
    fn what_is_listed_back_is_the_relationship_and_not_the_credential() {
        let (_dir, peers) = peers();
        peers
            .trust("vnt.test", "endpoint-1", SourceKind::Wiki, "cut", "s3cret")
            .unwrap();
        let trusted = peers.trusted();
        assert_eq!(
            trusted,
            vec![(
                "vnt.test".to_owned(),
                "endpoint-1".to_owned(),
                SourceKind::Wiki,
                "cut".to_owned()
            )]
        );
    }
}
