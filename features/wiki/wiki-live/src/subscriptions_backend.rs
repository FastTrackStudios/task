//! The `Subscriptions` service, over one org's store.
//!
//! Separate from [`WikiBackend`](crate::WikiBackend) on purpose: that
//! one is keyed by wiki, and subscriptions are keyed by *subscriber* —
//! the org's vault holds them too, and it is not a wiki. Bolting them
//! onto the wiki backend would have meant a wiki id that sometimes
//! means "the vault".
//!
//! Refresh needs an upstream to pull from, and where that comes from
//! differs per source: a wiki in the same org is a local directory, a
//! peer's is a vox client. So the backend takes an [`Upstream`]
//! resolver rather than assuming either — the materialize layer
//! already works against anything implementing `VaultSync`.

use std::path::PathBuf;
use std::sync::Arc;

use wiki_proto::service::subscriptions::{HeldSubscription, RefreshReport, Subscriptions};
use wiki_proto::subscription::{Subscriber, Subscription};
use wiki_proto::WikiError;

use crate::materialize;
use crate::subscriptions::SubscriptionStore;

/// How to reach the far side of a subscription.
///
/// Returning `None` means "this source is not reachable from here",
/// which is an orphan rather than an error: a subscribed copy goes on
/// resolving from disk when its home is unreachable
/// (`wiki.life.orphan`), so a refresh that cannot dial is a refusal to
/// refresh, not a broken subscription.
pub trait Upstream: Send + Sync + 'static {
    /// The local directory serving this source, when it is one.
    ///
    /// Only local upstreams for now. A remote one is the same
    /// materialize call against a `VaultSyncClient`, and the shape
    /// here is what will carry it.
    fn local_root(&self, subscription: &Subscription) -> Option<PathBuf>;
}

/// An [`Upstream`] that resolves sources published by orgs on this
/// same data root — which is what the demo and the suite exercise.
pub struct LocalOrgs {
    data_root: PathBuf,
    /// Domain → org slug. A domain is a *name* rather than an address
    /// (`wiki.ref.redirect`), so this is a lookup and not a DNS
    /// resolution.
    domains: std::collections::HashMap<String, String>,
}

impl LocalOrgs {
    #[must_use]
    pub fn new(
        data_root: PathBuf,
        domains: std::collections::HashMap<String, String>,
    ) -> Self {
        Self { data_root, domains }
    }
}

impl Upstream for LocalOrgs {
    fn local_root(&self, subscription: &Subscription) -> Option<PathBuf> {
        let org = self.domains.get(&subscription.domain)?;
        let root = self
            .data_root
            .join("orgs")
            .join(org)
            .join("wikis")
            .join(&subscription.slug);
        root.is_dir().then_some(root)
    }
}

/// One org's subscription service.
#[derive(Clone, architect::HasDispatcher)]
pub struct SubscriptionsBackend {
    org_root: PathBuf,
    store: SubscriptionStore,
    core: Arc<Vec<Subscription>>,
    upstream: Arc<dyn Upstream>,
}

impl SubscriptionsBackend {
    #[must_use]
    pub fn new(
        org_root: PathBuf,
        core: Vec<Subscription>,
        upstream: Arc<dyn Upstream>,
    ) -> Self {
        let store = SubscriptionStore::open(&org_root);
        Self {
            org_root,
            store,
            core: Arc::new(core),
            upstream,
        }
    }

    fn held(&self, subscription: &Subscription) -> HeldSubscription {
        let copy = materialize::local_copy_dir(
            &self.org_root,
            &subscription.domain,
            &subscription.slug,
        );
        let files = count_files(&copy);
        HeldSubscription {
            subscription: subscription.clone(),
            files,
            // Divergence is only known after a refresh compares the
            // two sides; reporting zero here would claim "no local
            // work" on evidence we do not have, so a caller that needs
            // the number refreshes.
            local_changes: 0,
            conflicts: 0,
        }
    }
}

fn count_files(dir: &std::path::Path) -> u32 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    let mut n = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            n += count_files(&path);
        } else {
            n += 1;
        }
    }
    n
}

fn store_err(e: crate::subscriptions::SubscriptionError) -> WikiError {
    WikiError::Io(e.to_string())
}

impl Subscriptions for SubscriptionsBackend {
    fn list_subscriptions(
        &self,
        subscriber: Subscriber,
    ) -> Result<Vec<HeldSubscription>, WikiError> {
        Ok(self
            .store
            .list(&subscriber)
            .map_err(store_err)?
            .iter()
            .map(|s| self.held(s))
            .collect())
    }

    fn subscribe(
        &self,
        subscriber: Subscriber,
        subscription: Subscription,
    ) -> Result<(), WikiError> {
        self.store
            .subscribe(&subscriber, subscription)
            .map_err(store_err)
    }

    fn unsubscribe(
        &self,
        subscriber: Subscriber,
        qualified: &str,
        force: bool,
    ) -> Result<(), WikiError> {
        // t[impl wiki.subscribe.local-copy] — unsubscribing from a
        // copy carrying unpushed work says so and takes an answer,
        // rather than discarding the work to tidy up.
        if !force {
            if let Some(held) = self
                .store
                .list(&subscriber)
                .map_err(store_err)?
                .into_iter()
                .find(|s| s.qualified() == qualified)
            {
                if let Some(root) = self.upstream.local_root(&held) {
                    let upstream = vault_live::Backend::single(&held.slug, root)
                        .map_err(|e| WikiError::Io(e.to_string()))?;
                    if let Ok(report) =
                        materialize::refresh_subscription(&upstream, &self.org_root, &held)
                    {
                        if report.has_local_work() {
                            return Err(WikiError::Io(format!(
                                "`{qualified}` has {} local change(s) and {} conflict(s) that \
                                 upstream has not seen; push them, or unsubscribe with force to \
                                 discard the copy",
                                report.local_only.len(),
                                report.conflicted.len()
                            )));
                        }
                    }
                }
            }
        }
        self.store
            .unsubscribe(&subscriber, qualified)
            .map_err(store_err)
    }

    fn refresh_subscription(
        &self,
        subscriber: Subscriber,
        qualified: &str,
    ) -> Result<RefreshReport, WikiError> {
        let held = self
            .store
            .active(&subscriber)
            .map_err(store_err)?
            .into_iter()
            .find(|s| s.qualified() == qualified)
            .ok_or_else(|| WikiError::Io(format!("not subscribed to `{qualified}`")))?;

        let root = self.upstream.local_root(&held).ok_or_else(|| {
            // Orphaned rather than broken: the copy still reads.
            WikiError::Io(format!(
                "`{qualified}` is not reachable from here; the local copy still resolves"
            ))
        })?;
        let upstream = vault_live::Backend::single(&held.slug, root)
            .map_err(|e| WikiError::Io(e.to_string()))?;
        let out = materialize::refresh_subscription(&upstream, &self.org_root, &held)
            .map_err(|e| WikiError::Io(e.to_string()))?;
        Ok(RefreshReport {
            qualified: qualified.to_owned(),
            pulled: u32::try_from(out.pulled).unwrap_or(u32::MAX),
            in_sync: u32::try_from(out.in_sync).unwrap_or(u32::MAX),
            local_only: out.local_only,
            conflicted: out.conflicted,
        })
    }

    fn core_set(&self) -> Result<Vec<Subscription>, WikiError> {
        Ok((*self.core).clone())
    }
}
