//! Where an org's subscriptions live on disk.
//!
//! One file per org, `<org>/subscriptions.json`, keyed by subscriber —
//! the org's vault, and each of its wikis. One file rather than one per
//! subscriber because the whole set is read on nearly every resolution
//! and written rarely, and because "what does this org hold" should be
//! answerable by opening one thing.
//!
//! Subscriptions are *not* wiki state. They belong to the subscriber,
//! not to the source, so they do not live under any wiki's `_state/`:
//! a wiki that is deleted must not take its holder's subscriptions with
//! it, and the org vault has no `_state/` at all.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

/// The on-disk shape: subscriber key → what it holds.
///
/// A `BTreeMap` so the file is stable across writes — a store that
/// reorders itself on every save turns a subscription change into an
/// unreadable diff in anyone's sync client.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct Held(BTreeMap<String, Vec<Subscription>>);

#[derive(Debug, thiserror::Error)]
pub enum SubscriptionError {
    #[error("{path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("already subscribed to `{0}`")]
    Duplicate(String),
    #[error("not subscribed to `{0}`")]
    NotHeld(String),
}

/// An org's subscription store.
#[derive(Debug, Clone)]
pub struct SubscriptionStore {
    path: PathBuf,
}

impl SubscriptionStore {
    /// Open the store for an org root. Creates nothing until a write.
    #[must_use]
    pub fn open(org_root: &Path) -> Self {
        Self {
            path: org_root.join("subscriptions.json"),
        }
    }

    fn load(&self) -> Result<Held, SubscriptionError> {
        match std::fs::read(&self.path) {
            Ok(bytes) if bytes.is_empty() => Ok(Held::default()),
            Ok(bytes) => {
                serde_json::from_slice(&bytes).map_err(|source| SubscriptionError::Parse {
                    path: self.path.display().to_string(),
                    source,
                })
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Held::default()),
            Err(source) => Err(SubscriptionError::Io {
                path: self.path.display().to_string(),
                source,
            }),
        }
    }

    /// Write via temp+rename, so a reader never sees half a set.
    fn save(&self, held: &Held) -> Result<(), SubscriptionError> {
        let io = |source| SubscriptionError::Io {
            path: self.path.display().to_string(),
            source,
        };
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let body = serde_json::to_vec_pretty(held).map_err(|source| SubscriptionError::Parse {
            path: self.path.display().to_string(),
            source,
        })?;
        let tmp = self
            .path
            .with_extension(format!("tmp.{}", std::process::id()));
        std::fs::write(&tmp, &body).map_err(io)?;
        std::fs::rename(&tmp, &self.path).map_err(io)
    }

    /// Everything a subscriber holds, declined entries included.
    ///
    /// Resolution filters to the active ones itself
    /// (`wiki_proto::resolve`); a caller listing subscriptions for a
    /// person to look at wants the declined ones too, or turning one
    /// back on would be impossible.
    ///
    /// # Errors
    ///
    /// A store that exists but cannot be read or parsed.
    pub fn list(&self, subscriber: &Subscriber) -> Result<Vec<Subscription>, SubscriptionError> {
        Ok(self
            .load()?
            .0
            .get(&subscriber.key())
            .cloned()
            .unwrap_or_default())
    }

    /// The active subscriptions a subscriber holds — what a reference
    /// resolves against.
    ///
    /// # Errors
    ///
    /// As [`Self::list`].
    pub fn active(&self, subscriber: &Subscriber) -> Result<Vec<Subscription>, SubscriptionError> {
        Ok(self
            .list(subscriber)?
            .into_iter()
            .filter(Subscription::is_active)
            .collect())
    }

    /// Every subscriber in the store, for a sweep.
    ///
    /// # Errors
    ///
    /// As [`Self::list`].
    pub fn subscribers(&self) -> Result<Vec<Subscriber>, SubscriptionError> {
        Ok(self
            .load()?
            .0
            .keys()
            .filter_map(|k| Subscriber::from_key(k))
            .collect())
    }

    /// Take on a subscription.
    ///
    /// Re-subscribing to something already held is an error rather
    /// than a silent no-op, because the two look identical afterwards
    /// and only one of them was what the caller meant. Re-subscribing
    /// to a *declined* source is not: that is someone changing their
    /// mind, and it clears the decline.
    ///
    /// # Errors
    ///
    /// [`SubscriptionError::Duplicate`] when already held and active,
    /// or any store read/write failure.
    pub fn subscribe(
        &self,
        subscriber: &Subscriber,
        subscription: Subscription,
    ) -> Result<(), SubscriptionError> {
        let mut held = self.load()?;
        let entry = held.0.entry(subscriber.key()).or_default();
        let id = subscription.qualified();
        match entry.iter_mut().find(|s| s.qualified() == id) {
            Some(existing) if existing.declined => {
                existing.declined = false;
                existing.title = subscription.title;
            }
            Some(_) => return Err(SubscriptionError::Duplicate(id)),
            None => entry.push(subscription),
        }
        entry.sort_by(|a, b| a.qualified().cmp(&b.qualified()));
        self.save(&held)
    }

    /// Drop a subscription.
    ///
    /// A core one is *declined* rather than removed, so a later
    /// resubscription sweep re-offers it instead of silently restoring
    /// it (`wiki.core.optional`). A non-core one is removed outright.
    ///
    /// # Errors
    ///
    /// [`SubscriptionError::NotHeld`], or a store failure.
    pub fn unsubscribe(
        &self,
        subscriber: &Subscriber,
        qualified: &str,
    ) -> Result<(), SubscriptionError> {
        let mut held = self.load()?;
        let entry = held
            .0
            .get_mut(&subscriber.key())
            .ok_or_else(|| SubscriptionError::NotHeld(qualified.to_owned()))?;
        let Some(pos) = entry.iter().position(|s| s.qualified() == qualified) else {
            return Err(SubscriptionError::NotHeld(qualified.to_owned()));
        };
        if entry[pos].core {
            entry[pos].declined = true;
        } else {
            entry.remove(pos);
        }
        self.save(&held)
    }

    /// Hand a subscriber the deployment's core set, minus what it has
    /// already declined.
    ///
    /// t[impl wiki.core.default] — a vault carries the core set from
    /// the moment it exists, with nobody opting in.
    /// t[impl wiki.core.retroactive] — this runs over subscribers that
    /// already exist, so adding to the core set reaches them rather
    /// than only orgs created afterwards.
    /// t[impl wiki.core.optional] — a decline is honoured on every
    /// later sweep, which is what stops "on by default" quietly
    /// becoming "cannot be removed".
    ///
    /// Returns what was newly added.
    ///
    /// # Errors
    ///
    /// Any store read/write failure.
    pub fn ensure_core(
        &self,
        subscriber: &Subscriber,
        core: &[Subscription],
    ) -> Result<Vec<String>, SubscriptionError> {
        let mut held = self.load()?;
        let entry = held.0.entry(subscriber.key()).or_default();
        let mut added = Vec::new();
        for source in core {
            let id = source.qualified();
            if entry.iter().any(|s| s.qualified() == id) {
                // Held already, declined or not. A decline survives
                // the sweep — that is the whole point of keeping it.
                continue;
            }
            let mut fresh = source.clone();
            fresh.core = true;
            fresh.declined = false;
            entry.push(fresh);
            added.push(id);
        }
        if added.is_empty() {
            return Ok(added);
        }
        entry.sort_by(|a, b| a.qualified().cmp(&b.qualified()));
        self.save(&held)?;
        Ok(added)
    }
}

/// A convenience for the common case: a Resource everyone gets.
#[must_use]
pub fn core_resource(domain: &str, slug: &str, title: &str) -> Subscription {
    Subscription {
        domain: domain.to_owned(),
        slug: slug.to_owned(),
        kind: SourceKind::Resource,
        title: title.to_owned(),
        core: true,
        declined: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> (tempfile::TempDir, SubscriptionStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = SubscriptionStore::open(tmp.path());
        (tmp, store)
    }

    fn wiki_sub(domain: &str, slug: &str) -> Subscription {
        Subscription {
            domain: domain.into(),
            slug: slug.into(),
            kind: SourceKind::Wiki,
            title: slug.into(),
            core: false,
            declined: false,
        }
    }

    #[test]
    fn an_empty_store_holds_nothing_and_is_not_an_error() {
        let (_tmp, store) = store();
        assert!(store.list(&Subscriber::Vault).unwrap().is_empty());
        assert!(store.subscribers().unwrap().is_empty());
    }

    #[test]
    fn subscriptions_are_per_subscriber() {
        let (_tmp, store) = store();
        let vault = Subscriber::Vault;
        let cooking = Subscriber::Wiki("cooking".into());
        store
            .subscribe(&vault, wiki_sub("acme.test", "music-theory"))
            .unwrap();
        store
            .subscribe(&cooking, wiki_sub("alice.test", "bible-study"))
            .unwrap();

        assert_eq!(store.list(&vault).unwrap().len(), 1);
        assert_eq!(store.list(&cooking).unwrap()[0].slug, "bible-study");
        // The vault did not gain the wiki's subscription.
        assert!(
            store
                .list(&vault)
                .unwrap()
                .iter()
                .all(|s| s.slug != "bible-study")
        );
        assert_eq!(store.subscribers().unwrap().len(), 2);
    }

    #[test]
    fn subscribing_twice_is_refused_rather_than_silently_ignored() {
        let (_tmp, store) = store();
        let s = wiki_sub("acme.test", "music-theory");
        store.subscribe(&Subscriber::Vault, s.clone()).unwrap();
        assert!(matches!(
            store.subscribe(&Subscriber::Vault, s),
            Err(SubscriptionError::Duplicate(_))
        ));
    }

    /// t[verify wiki.core.default] — the core set arrives without
    /// anyone opting in, and is idempotent across sweeps.
    #[test]
    fn the_core_set_is_handed_over_once() {
        let (_tmp, store) = store();
        let core = [core_resource("fasttrackstudio.app", "bible", "Bible")];
        let added = store.ensure_core(&Subscriber::Vault, &core).unwrap();
        assert_eq!(added, vec!["fasttrackstudio.app/bible".to_owned()]);
        // Sweeping again adds nothing.
        assert!(
            store
                .ensure_core(&Subscriber::Vault, &core)
                .unwrap()
                .is_empty()
        );
        assert_eq!(store.active(&Subscriber::Vault).unwrap().len(), 1);
    }

    /// t[verify wiki.core.optional] — declining a core subscription
    /// keeps it declined across later sweeps, and affects only the
    /// subscriber that declined.
    #[test]
    fn a_declined_core_subscription_stays_declined() {
        let (_tmp, store) = store();
        let core = [core_resource("fasttrackstudio.app", "bible", "Bible")];
        let vault = Subscriber::Vault;
        let cooking = Subscriber::Wiki("cooking".into());
        store.ensure_core(&vault, &core).unwrap();
        store.ensure_core(&cooking, &core).unwrap();

        store
            .unsubscribe(&vault, "fasttrackstudio.app/bible")
            .unwrap();
        assert!(store.active(&vault).unwrap().is_empty());
        // Kept, not deleted — otherwise the next sweep restores it.
        assert_eq!(store.list(&vault).unwrap().len(), 1);

        assert!(store.ensure_core(&vault, &core).unwrap().is_empty());
        assert!(store.active(&vault).unwrap().is_empty());

        // The other subscriber is untouched.
        assert_eq!(store.active(&cooking).unwrap().len(), 1);
    }

    #[test]
    fn resubscribing_to_a_declined_source_clears_the_decline() {
        let (_tmp, store) = store();
        let vault = Subscriber::Vault;
        let core = [core_resource("fasttrackstudio.app", "bible", "Bible")];
        store.ensure_core(&vault, &core).unwrap();
        store
            .unsubscribe(&vault, "fasttrackstudio.app/bible")
            .unwrap();
        store.subscribe(&vault, core[0].clone()).unwrap();
        assert_eq!(store.active(&vault).unwrap().len(), 1);
    }

    /// t[verify wiki.core.retroactive] — a subscriber that already
    /// exists gains a newly-core source.
    #[test]
    fn adding_to_the_core_set_reaches_existing_subscribers() {
        let (_tmp, store) = store();
        let vault = Subscriber::Vault;
        let first = [core_resource("fasttrackstudio.app", "bible", "Bible")];
        store.ensure_core(&vault, &first).unwrap();

        let grown = [
            first[0].clone(),
            core_resource("fasttrackstudio.app", "lexicon", "Lexicon"),
        ];
        let added = store.ensure_core(&vault, &grown).unwrap();
        assert_eq!(added, vec!["fasttrackstudio.app/lexicon".to_owned()]);
        assert_eq!(store.active(&vault).unwrap().len(), 2);
    }

    #[test]
    fn a_non_core_subscription_is_removed_outright() {
        let (_tmp, store) = store();
        let vault = Subscriber::Vault;
        store
            .subscribe(&vault, wiki_sub("acme.test", "music-theory"))
            .unwrap();
        store.unsubscribe(&vault, "acme.test/music-theory").unwrap();
        assert!(store.list(&vault).unwrap().is_empty());
        assert!(matches!(
            store.unsubscribe(&vault, "acme.test/music-theory"),
            Err(SubscriptionError::NotHeld(_))
        ));
    }
}
