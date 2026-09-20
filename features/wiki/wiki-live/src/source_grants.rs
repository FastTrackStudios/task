//! Which other servers may read which of this org's sources.
//!
//! The publisher's half of a cross-server subscription. A subscriber on
//! another server has no session here and no membership row — it is a
//! server, not a person — so the two read calls it makes are reachable
//! without a session, and the rule for that surface is that a service on
//! it **authenticates its own callers**. A secret is how this one does.
//!
//! # Why a secret and not the source's visibility alone
//!
//! Visibility answers "may this source be read by an outsider". It is a
//! fact about the *resource*, and a resource cannot authenticate anybody
//! — leaning on it alone would have let any caller that could reach the
//! server ask for any unlisted source, which is a different and weaker
//! claim than the one `wiki.access.visibility` makes. Both checks run:
//! the secret says *who*, visibility says *whether*, and a grant never
//! overrides a wiki turned private.
//!
//! # Shape
//!
//! One secret per source, stored beside the subscriptions rather than in
//! the source, because it is the publisher's state about a relationship
//! and not content anybody should sync. Revocation is deleting the row;
//! it binds on the subscriber's next call because the secret is checked
//! on every one.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wiki_proto::subscription::SourceKind;

/// `"<kind>/<slug>"` — how a grant is keyed on disk.
///
/// Kind *and* slug, because the tiers are separate namespaces: an asset
/// shelf called `songs` and a wiki called `songs` are two sources, and a
/// grant on one must not read the other.
fn key(kind: SourceKind, slug: &str) -> String {
    format!("{}/{slug}", kind.tier_dir())
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Grants(BTreeMap<String, String>);

/// The publisher's grant table for one org.
pub struct SourceGrants {
    path: PathBuf,
}

impl SourceGrants {
    /// Open the table for an org root. Nothing is read until asked.
    #[must_use]
    pub fn open(org_root: &Path) -> Self {
        Self {
            path: org_root
                .join("subscribed")
                .join(".state")
                .join("granted.json"),
        }
    }

    fn load(&self) -> Grants {
        // A table that will not parse is treated as empty rather than
        // fatal, and the consequence is a refusal: every call needs a
        // secret to match, so an unreadable table denies rather than
        // admits. That is the safe direction for this file.
        std::fs::read(&self.path)
            .ok()
            .and_then(|b| serde_json::from_slice(&b).ok())
            .unwrap_or_default()
    }

    fn save(&self, grants: &Grants) -> std::io::Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(grants).map_err(std::io::Error::other)?;
        std::fs::write(&self.path, bytes)
    }

    /// The secret for this source, minting one if it has none.
    ///
    /// # Errors
    ///
    /// A failure writing the table.
    pub fn grant(&self, kind: SourceKind, slug: &str) -> std::io::Result<String> {
        let mut grants = self.load();
        let k = key(kind, slug);
        if let Some(existing) = grants.0.get(&k) {
            return Ok(existing.clone());
        }
        // A v4 uuid is 122 bits of randomness, which is what an offer
        // secret is too. Guessing one is not the attack to worry about;
        // leaking one is, and that is what revocation is for.
        let secret = uuid::Uuid::new_v4().to_string();
        grants.0.insert(k, secret.clone());
        self.save(&grants)?;
        Ok(secret)
    }

    /// Forget this source's grant. Revoking one that is not there
    /// succeeds, so a cleanup that runs twice is harmless.
    ///
    /// # Errors
    ///
    /// A failure writing the table.
    pub fn revoke(&self, kind: SourceKind, slug: &str) -> std::io::Result<()> {
        let mut grants = self.load();
        if grants.0.remove(&key(kind, slug)).is_some() {
            self.save(&grants)?;
        }
        Ok(())
    }

    /// Whether `secret` is the grant for exactly this source.
    ///
    /// Scoped deliberately: a secret for one source says nothing about
    /// another, so a subscriber cannot walk from the shelf it was given
    /// to the wiki it was not. Same property `Offer.path` has on the
    /// files lane — a grant on a folder says nothing about its parents.
    #[must_use]
    pub fn admits(&self, kind: SourceKind, slug: &str, secret: &str) -> bool {
        if secret.is_empty() {
            return false;
        }
        self.load().0.get(&key(kind, slug)).is_some_and(|granted| {
            // Length-independent equality is not the concern here (the
            // secret is not a MAC and the comparison is not a timing
            // oracle worth the dependency), but an empty granted value
            // matching an empty secret would be, and the guard above
            // rules it out.
            granted == secret
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SourceGrants) {
        let dir = tempfile::tempdir().unwrap();
        let store = SourceGrants::open(dir.path());
        (dir, store)
    }

    #[test]
    fn a_granted_secret_admits_its_own_source_and_no_other() {
        let (_dir, store) = store();
        let secret = store.grant(SourceKind::Assets, "songs").unwrap();

        assert!(store.admits(SourceKind::Assets, "songs", &secret));
        assert!(
            !store.admits(SourceKind::Wiki, "songs", &secret),
            "a shelf grant read a wiki of the same name: the tiers are \
             separate namespaces and a grant must not cross them"
        );
        assert!(
            !store.admits(SourceKind::Assets, "charts", &secret),
            "a grant on one shelf admitted another"
        );
    }

    #[test]
    fn granting_twice_returns_one_secret() {
        let (_dir, store) = store();
        let first = store.grant(SourceKind::Assets, "songs").unwrap();
        let again = store.grant(SourceKind::Assets, "songs").unwrap();
        assert_eq!(first, again, "a second grant minted a secret nobody holds");
    }

    #[test]
    fn revoking_ends_it_and_is_idempotent() {
        let (_dir, store) = store();
        let secret = store.grant(SourceKind::Assets, "songs").unwrap();
        store.revoke(SourceKind::Assets, "songs").unwrap();
        assert!(!store.admits(SourceKind::Assets, "songs", &secret));
        store
            .revoke(SourceKind::Assets, "songs")
            .expect("revoking twice is harmless");
    }

    /// The empty secret is the one a caller sends by sending nothing, so
    /// it must never match — including against a table that has never
    /// been written.
    #[test]
    fn nothing_is_admitted_by_an_empty_secret() {
        let (_dir, store) = store();
        assert!(!store.admits(SourceKind::Assets, "songs", ""));
        store.grant(SourceKind::Assets, "songs").unwrap();
        assert!(!store.admits(SourceKind::Assets, "songs", ""));
    }
}
