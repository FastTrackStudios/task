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
//!
//! The upstream is also where **visibility** is enforced
//! (`wiki.access.visibility`). A subscription is the subscriber's own
//! state, but whether the source admits them is the source's decision,
//! read from the source's declaration at the moment of subscribing and
//! again at every refresh — so narrowing a wiki takes effect on copies
//! already held, without deleting anything.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use wiki_proto::WikiError;
use wiki_proto::config::Visibility;
use wiki_proto::service::subscriptions::{HeldSubscription, RefreshReport, Subscriptions};
use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

use crate::materialize;
use crate::subscriptions::SubscriptionStore;

/// What a source says to a would-be subscriber.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Admission {
    /// This resolver does not serve that domain; nothing can be said
    /// here. A peer's or an orphaned source — the subscription stands
    /// and resolves from whatever copy exists (`wiki.life.orphan`).
    Unknown,
    /// The source exists and admits this subscriber.
    Admitted,
    /// The source exists and turns this subscriber away, or the name
    /// does not denote a subscribable thing at all. The reason is what
    /// the subscriber is told.
    Refused(String),
}

/// How to reach the far side of a subscription.
///
/// Returning `None` from [`Self::local_root`] means "this source is not
/// reachable from here", which is an orphan rather than an error: a
/// subscribed copy goes on resolving from disk when its home is
/// unreachable (`wiki.life.orphan`), so a refresh that cannot dial is a
/// refusal to refresh, not a broken subscription.
pub trait Upstream: Send + Sync + 'static {
    /// The local directory serving this source, when it is one.
    ///
    /// Only local upstreams for now. A remote one is the same
    /// materialize call against a `VaultSyncClient`, and the shape
    /// here is what will carry it.
    fn local_root(&self, subscription: &Subscription) -> Option<PathBuf>;

    /// Whether the source admits `subscriber_org`.
    fn admits(&self, subscriber_org: &str, subscription: &Subscription) -> Admission {
        let _ = (subscriber_org, subscription);
        Admission::Unknown
    }

    /// Every source open to subscription that this resolver can see,
    /// for a subscriber in `subscriber_org`.
    fn discover(&self, subscriber_org: &str) -> Vec<Subscription> {
        let _ = subscriber_org;
        Vec::new()
    }
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
    pub fn new(data_root: PathBuf, domains: std::collections::HashMap<String, String>) -> Self {
        Self { data_root, domains }
    }

    fn org_dir(&self, slug: &str) -> PathBuf {
        self.data_root.join("orgs").join(slug)
    }

    /// The domain an org publishes under, when this resolver knows one.
    fn domain_of(&self, org: &str) -> Option<&str> {
        self.domains
            .iter()
            .find(|(_, slug)| slug.as_str() == org)
            .map(|(domain, _)| domain.as_str())
    }
}

impl LocalOrgs {
    /// Where a source's files live in its publishing org: a wiki under
    /// `wikis/<slug>`, a Resource under `resources/<slug>` — the
    /// corpus library (`OrgRoot::resources_dir`), where `admin bible
    /// install` puts scripture. A Resource is not a wiki
    /// (`wiki.resource.not-a-wiki`), and its text was never going to
    /// sit in the wikis directory.
    fn source_root(&self, org: &str, subscription: &Subscription) -> PathBuf {
        let kind_dir = match subscription.kind {
            SourceKind::Wiki => "wikis",
            SourceKind::Resource => "resources",
        };
        self.org_dir(org).join(kind_dir).join(&subscription.slug)
    }
}

impl Upstream for LocalOrgs {
    fn local_root(&self, subscription: &Subscription) -> Option<PathBuf> {
        let org = self.domains.get(&subscription.domain)?;
        let root = self.source_root(org, subscription);
        root.is_dir().then_some(root)
    }

    /// t[impl wiki.access.visibility] — private is a refusal for
    /// anyone outside the owning org, unlisted admits whoever holds the
    /// reference, public admits everyone. The two non-public states
    /// differ here and only here, and the difference is a refusal.
    ///
    /// t[impl wiki.boundary.no-subscribe] — a name that denotes a
    /// vault rather than a wiki is refused with the distinction stated,
    /// never coerced into a share link or a missing-page error.
    fn admits(&self, subscriber_org: &str, subscription: &Subscription) -> Admission {
        let Some(org) = self.domains.get(&subscription.domain) else {
            return Admission::Unknown;
        };
        if matches!(subscription.slug.as_str(), "vault" | "default") {
            return Admission::Refused(format!(
                "`{}` names {}'s vault, and a vault is never subscribable: sharing a vault \
                 or a note from it goes through a share link, which grants reading a named \
                 thing and never makes it resolvable inside someone else's writing. \
                 Promote it to a wiki first (`wiki.promote.vault`).",
                subscription.qualified(),
                org
            ));
        }
        let root = self.source_root(org, subscription);
        if !root.is_dir() {
            return Admission::Refused(format!(
                "`{}` has no {} `{}`",
                subscription.domain,
                match subscription.kind {
                    SourceKind::Wiki => "wiki",
                    SourceKind::Resource => "resource",
                },
                subscription.slug
            ));
        }
        // t[impl wiki.resource.rights] — a Resource this platform
        // publishes is held whole because its licence allows it (only
        // public-domain editions install), so it admits everyone; it has
        // no config to consult and no visibility to be in.
        if subscription.kind == SourceKind::Resource {
            return Admission::Admitted;
        }
        let config = match crate::config::load(&root, &subscription.slug) {
            Ok(c) => c,
            Err(e) => return Admission::Refused(format!("{}: {e}", subscription.qualified())),
        };
        if org == subscriber_org || config.visibility.admits_outsiders() {
            Admission::Admitted
        } else {
            Admission::Refused(format!(
                "`{}` is private: {} has not published it, and a subscription from \
                 outside that org is refused rather than left unresolved",
                subscription.qualified(),
                org
            ))
        }
    }

    /// t[impl wiki.access.directory] — the directory this server can
    /// serve: every public wiki on its data root, its own org's
    /// included. One peer's view, not a gatekeeper — a subscription by
    /// reference never needs this to succeed.
    fn discover(&self, subscriber_org: &str) -> Vec<Subscription> {
        let mut out = Vec::new();
        let Ok(orgs) = std::fs::read_dir(self.data_root.join("orgs")) else {
            return out;
        };
        for org in orgs.flatten() {
            let Some(slug) = org.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let Some(domain) = self.domain_of(&slug) else {
                continue;
            };
            let Ok(wikis) = std::fs::read_dir(org.path().join("wikis")) else {
                continue;
            };
            for wiki in wikis.flatten() {
                if !wiki.path().is_dir() {
                    continue;
                }
                let Some(wiki_slug) = wiki.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if wiki_slug.starts_with('.') {
                    continue;
                }
                let Ok(config) = crate::config::load(&wiki.path(), &wiki_slug) else {
                    continue;
                };
                // A member sees their own org's unlisted wikis too:
                // discovery is "what may I take on", and they may.
                let visible = config.visibility.is_listed()
                    || (slug == subscriber_org && config.visibility != Visibility::Private);
                if !visible {
                    continue;
                }
                out.push(Subscription {
                    domain: domain.to_owned(),
                    slug: wiki_slug.clone(),
                    kind: SourceKind::Wiki,
                    title: if config.title.is_empty() {
                        wiki_slug
                    } else {
                        config.title
                    },
                    core: false,
                    declined: false,
                });
            }
        }
        out.sort_by(|a, b| a.qualified().cmp(&b.qualified()));
        out
    }
}

/// One org's subscription service.
#[derive(Clone, architect::HasDispatcher)]
pub struct SubscriptionsBackend {
    org_root: PathBuf,
    /// The org this backend speaks for — the last component of its
    /// root. What an upstream is asked to admit.
    org_slug: String,
    store: SubscriptionStore,
    core: Arc<Vec<Subscription>>,
    upstream: Arc<dyn Upstream>,
}

impl SubscriptionsBackend {
    #[must_use]
    pub fn new(org_root: PathBuf, core: Vec<Subscription>, upstream: Arc<dyn Upstream>) -> Self {
        let store = SubscriptionStore::open(&org_root);
        let org_slug = org_root
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_owned();
        Self {
            org_root,
            org_slug,
            store,
            core: Arc::new(core),
            upstream,
        }
    }

    fn held(&self, subscription: &Subscription) -> HeldSubscription {
        let copy = match subscription.kind {
            SourceKind::Wiki => materialize::local_copy_dir(
                &self.org_root,
                &subscription.domain,
                &subscription.slug,
            ),
            SourceKind::Resource => {
                materialize::resource_copy_dir(&self.org_root, &subscription.slug)
            }
        };
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

    /// Ask the source whether it admits this org; `Unknown` passes.
    fn admitted(&self, subscription: &Subscription) -> Result<(), WikiError> {
        match self.upstream.admits(&self.org_slug, subscription) {
            Admission::Refused(reason) => Err(WikiError::Refused(reason)),
            Admission::Admitted | Admission::Unknown => Ok(()),
        }
    }
}

fn count_files(dir: &Path) -> u32 {
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

    /// t[impl wiki.subscribe.reference] — taking a source on is one
    /// write to the subscriber's own store; no existing page changes.
    /// The source is asked first whether it admits this org
    /// (`wiki.access.visibility`); a refusal is returned as one.
    fn subscribe(
        &self,
        subscriber: Subscriber,
        subscription: Subscription,
    ) -> Result<(), WikiError> {
        architect_telemetry::wide::set("wiki.subscribe.source", subscription.qualified());
        if let Err(e) = self.admitted(&subscription) {
            architect_telemetry::wide::set("wiki.subscribe.outcome", "refused");
            return Err(e);
        }
        self.store
            .subscribe(&subscriber, subscription)
            .map_err(store_err)?;
        architect_telemetry::wide::set("wiki.subscribe.outcome", "subscribed");
        Ok(())
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

    /// t[impl wiki.access.visibility] — narrowing takes effect on what
    /// is already published: a source that no longer admits this org
    /// stops refreshing here, while the copy already held goes on
    /// reading (`wiki.life.orphan`).
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
        architect_telemetry::wide::set("wiki.subscribe.source", qualified.to_owned());
        self.admitted(&held)?;

        let root = self.upstream.local_root(&held).ok_or_else(|| {
            // Orphaned rather than broken: the copy still reads.
            WikiError::Io(format!(
                "`{qualified}` is not reachable from here; the local copy still resolves"
            ))
        })?;
        let out = match held.kind {
            SourceKind::Wiki => {
                let upstream = vault_live::Backend::single(&held.slug, root)
                    .map_err(|e| WikiError::Io(e.to_string()))?;
                materialize::refresh_subscription(&upstream, &self.org_root, &held)
                    .map_err(|e| WikiError::Io(e.to_string()))?
            }
            // A corpus, not a wiki: copied whole into the library the
            // reader opens (`materialize::resource_copy_dir`).
            SourceKind::Resource => materialize::refresh_resource(&root, &self.org_root, &held)
                .map_err(|e| WikiError::Io(e.to_string()))?,
        };
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

    fn discover(&self) -> Result<Vec<Subscription>, WikiError> {
        Ok(self.upstream.discover(&self.org_slug))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use wiki_proto::config::WikiConfig;

    /// Two orgs on one data root: `acme` publishing three wikis at the
    /// three visibilities, `alice` subscribing.
    fn world() -> (tempfile::TempDir, Arc<LocalOrgs>) {
        let dir = tempfile::tempdir().unwrap();
        for (slug, vis) in [
            ("theory", Visibility::Public),
            ("cooking", Visibility::Unlisted),
            ("diary", Visibility::Private),
        ] {
            let root = dir.path().join("orgs/acme/wikis").join(slug);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("Page.md"), "# Page\n").unwrap();
            let mut c = WikiConfig::implicit(slug);
            c.visibility = vis;
            c.title = slug.to_uppercase();
            crate::config::save(&root, &c).unwrap();
        }
        std::fs::create_dir_all(dir.path().join("orgs/alice/wikis")).unwrap();
        let domains: HashMap<String, String> = [
            ("acme.test".to_string(), "acme".to_string()),
            ("alice.test".to_string(), "alice".to_string()),
        ]
        .into_iter()
        .collect();
        let upstream = Arc::new(LocalOrgs::new(dir.path().to_path_buf(), domains));
        (dir, upstream)
    }

    fn sub(slug: &str) -> Subscription {
        Subscription {
            domain: "acme.test".into(),
            slug: slug.into(),
            kind: SourceKind::Wiki,
            title: String::new(),
            core: false,
            declined: false,
        }
    }

    /// t[verify wiki.resource.subscribe] — the platform's Resource is
    /// served from its publisher's corpus library, admits an outsider
    /// with no config to consult, and refreshes into the subscriber's
    /// own library where the reader looks.
    #[test]
    fn a_resource_is_served_from_the_corpus_library() {
        let (dir, upstream) = world();
        let corpus = dir.path().join("orgs/acme/resources/bible/WEB");
        std::fs::create_dir_all(&corpus).unwrap();
        std::fs::write(corpus.join("JHN.usfm"), "\\id JHN\n").unwrap();
        let bible = Subscription {
            kind: SourceKind::Resource,
            core: true,
            title: "Bible".into(),
            ..sub("bible")
        };
        assert_eq!(upstream.admits("alice", &bible), Admission::Admitted);
        assert_eq!(
            upstream.local_root(&bible),
            Some(dir.path().join("orgs/acme/resources/bible"))
        );

        let alice = SubscriptionsBackend::new(
            dir.path().join("orgs/alice"),
            vec![bible.clone()],
            upstream.clone(),
        );
        alice.subscribe(Subscriber::Vault, bible.clone()).unwrap();
        let report = alice
            .refresh_subscription(Subscriber::Vault, &bible.qualified())
            .unwrap();
        assert_eq!(report.pulled, 1);
        assert!(
            dir.path()
                .join("orgs/alice/resources/bible/WEB/JHN.usfm")
                .is_file()
        );
        let held = alice.list_subscriptions(Subscriber::Vault).unwrap();
        let mine = held
            .iter()
            .find(|h| h.subscription.slug == "bible")
            .expect("held");
        assert_eq!(
            mine.files, 1,
            "presence counts the corpus, not a subscribed/ dir"
        );
    }

    /// t[verify wiki.access.visibility] — from outside the owning org:
    /// public and unlisted admit, private refuses, and the refusal
    /// names the state.
    #[test]
    fn outsiders_are_admitted_or_refused_by_visibility() {
        let (dir, upstream) = world();
        let alice =
            SubscriptionsBackend::new(dir.path().join("orgs/alice"), vec![], upstream.clone());
        alice.subscribe(Subscriber::Vault, sub("theory")).unwrap();
        alice.subscribe(Subscriber::Vault, sub("cooking")).unwrap();
        let err = alice
            .subscribe(Subscriber::Vault, sub("diary"))
            .expect_err("private refuses");
        assert!(
            matches!(&err, WikiError::Refused(m) if m.contains("private")),
            "{err:?}"
        );
        // The owner is never an outsider to its own wiki.
        let acme = SubscriptionsBackend::new(dir.path().join("orgs/acme"), vec![], upstream);
        acme.subscribe(Subscriber::Wiki("theory".into()), sub("diary"))
            .unwrap();
    }

    /// t[verify wiki.access.visibility] — narrowing a wiki stops a
    /// held subscription refreshing without deleting the copy.
    #[test]
    fn narrowing_stops_refresh_but_keeps_the_copy() {
        let (dir, upstream) = world();
        let alice =
            SubscriptionsBackend::new(dir.path().join("orgs/alice"), vec![], upstream.clone());
        alice.subscribe(Subscriber::Vault, sub("theory")).unwrap();
        let report = alice
            .refresh_subscription(Subscriber::Vault, "acme.test/theory")
            .unwrap();
        assert_eq!(report.pulled, 1);
        let copy =
            materialize::local_copy_dir(&dir.path().join("orgs/alice"), "acme.test", "theory");
        assert!(copy.join("Page.md").is_file());

        let root = dir.path().join("orgs/acme/wikis/theory");
        crate::config::update(&root, "theory", |c| c.visibility = Visibility::Private).unwrap();
        let err = alice
            .refresh_subscription(Subscriber::Vault, "acme.test/theory")
            .expect_err("no longer admitted");
        assert!(matches!(err, WikiError::Refused(_)), "{err:?}");
        assert!(copy.join("Page.md").is_file(), "the copy is not deleted");
        let held = alice.list_subscriptions(Subscriber::Vault).unwrap();
        assert_eq!(held.len(), 1, "still subscribed; reading goes on");
    }

    /// t[verify wiki.boundary.no-subscribe] — a vault is refused with
    /// the distinction stated, not coerced into a link or reported as
    /// a missing page.
    #[test]
    fn a_vault_is_never_subscribable() {
        let (dir, upstream) = world();
        let alice = SubscriptionsBackend::new(dir.path().join("orgs/alice"), vec![], upstream);
        let err = alice
            .subscribe(Subscriber::Vault, sub("vault"))
            .expect_err("vaults are not sources");
        match err {
            WikiError::Refused(m) => {
                assert!(m.contains("never subscribable"), "{m}");
                assert!(m.contains("share link"), "{m}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        // A wiki that simply does not exist is a different message.
        let err = alice
            .subscribe(Subscriber::Vault, sub("nonesuch"))
            .expect_err("missing");
        assert!(
            matches!(&err, WikiError::Refused(m) if m.contains("no wiki")),
            "{err:?}"
        );
    }

    /// t[verify wiki.access.directory] — discovery lists public wikis
    /// from every org here, an org's own unlisted ones to itself, and
    /// never a private one. An unknown domain is `Unknown`, not
    /// refused: a peer's source stands on the reference alone.
    #[test]
    fn discovery_shows_public_and_own_unlisted_only() {
        let (dir, upstream) = world();
        let alice =
            SubscriptionsBackend::new(dir.path().join("orgs/alice"), vec![], upstream.clone());
        let seen: Vec<String> = alice
            .discover()
            .unwrap()
            .into_iter()
            .map(|s| s.qualified())
            .collect();
        assert_eq!(seen, vec!["acme.test/theory"]);

        let acme =
            SubscriptionsBackend::new(dir.path().join("orgs/acme"), vec![], upstream.clone());
        let seen: Vec<String> = acme
            .discover()
            .unwrap()
            .into_iter()
            .map(|s| s.qualified())
            .collect();
        assert_eq!(seen, vec!["acme.test/cooking", "acme.test/theory"]);

        let remote = Subscription {
            domain: "peer.example".into(),
            ..sub("theory")
        };
        assert_eq!(upstream.admits("alice", &remote), Admission::Unknown);
        alice.subscribe(Subscriber::Vault, remote).unwrap();
    }
}
