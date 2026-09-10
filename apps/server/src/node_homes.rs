//! Resolving a reference that names another organisation — ADR 0003's
//! read path.
//!
//! [`links::NodeHomes`] asks one question: this reader is holding
//! `guest.example/song:hosanna`, may they follow it, and where does it
//! live? Answering needs three things the link store deliberately does
//! not hold — the map from federation domain to org, the reader's own
//! subscriptions, and the org roots on disk — so it is answered here,
//! and injected.
//!
//! # A reference addresses; it never authorises
//!
//! The whole safety of the qualified form rests on this. Being able to
//! *write* `guest.example/song:hosanna` grants nothing: resolution
//! succeeds only where the reader already had access by a mechanism that
//! predates ADR 0003 — a subscription to the source that publishes the
//! node. Nothing here widens what anyone can read; it only lets them
//! name it.
//!
//! # Which source publishes a node
//!
//! Library material lives at `<org>/resources/<kind>s/` (ADR 0003), and
//! a Resource subscription materialises `<org>/resources/<slug>/`. So
//! the source that publishes `chart:doxology` is the one whose slug is
//! `charts`, and a reader subscribed to `guest.example/charts` may
//! follow any `guest.example/chart:*`. One subscription per library,
//! which is the granularity a person actually chooses in.
//!
//! # ADR 0004 moved charts out from under that, and it costs
//!
//! A chart is a vault document now (`<org>/vault/Assets/Charts/`), and
//! **the vault has no cross-organisation reach**. The subscription
//! vocabulary, the materialiser and the media route all name
//! `resources/`; nothing names a vault path, and nothing should — a
//! vault is an org's private tree and opening a route onto it is a
//! decision, not a fix.
//!
//! So the subscription slug `charts` still gates the *permission*
//! (nothing about "a reference addresses, it never authorises" changed),
//! [`LocalHomes::locate`] still resolves the reference by looking on the
//! shelf, and the reader still cannot fetch the bytes. What breaks,
//! concretely:
//!
//! - `wiki_live::materialize::refresh_resource` copies
//!   `<org>/resources/<slug>/` into a subscriber's held tree. A chart
//!   library subscription now materialises the frozen ADR 0003 snapshot
//!   the migration left, or an empty directory for an org that never
//!   had one.
//! - `GET /org/{slug}/media/{*path}` (`per_org_media_handler`) serves
//!   the org's `resources/` tree. A chart's `rel_path` no longer names
//!   anything it can reach.
//!
//! ADR 0004 says nothing should land before an Assets-tier reach path
//! exists. It landed anyway, with the gap recorded in
//! `docs/spec/unmet.md` rather than discovered later.
//!
//! # What is deliberately not distinguished
//!
//! [`Reach::NotPermitted`] covers both "you are not subscribed" and, for
//! an unsubscribed reader, "there is nothing there". Telling an outsider
//! which slugs exist is exactly the enumeration a private source
//! refuses, and `wiki.access.visibility` already draws that line.

use std::collections::HashMap;
use std::path::PathBuf;

use links::{NodeHomes, NodeKind, NodeRef, Reach, ResolvedNode};
use wiki_proto::subscription::{SourceKind, Subscriber};

/// The directory under `resources/` that publishes each node kind, and
/// therefore the subscription slug that admits a reader to it.
///
/// `None` for the kinds that are not library material: a note or a block
/// is vault-internal, and a verse comes from the scripture spine, which
/// has its own core subscription.
#[must_use]
pub fn library_of(kind: NodeKind) -> Option<&'static str> {
    Some(match kind {
        NodeKind::Song => "songs",
        NodeKind::Sermon => "sermons",
        NodeKind::Video => "videos",
        NodeKind::Chart => "charts",
        NodeKind::Patch => "patches",
        NodeKind::Sample => "samples",
        NodeKind::Lighting => "lighting",
        NodeKind::Verse
        | NodeKind::Note
        | NodeKind::Wiki
        | NodeKind::Topic
        | NodeKind::Entity
        | NodeKind::Block
        | NodeKind::External => return None,
    })
}

/// Resolves qualified references against the orgs this deployment holds.
///
/// The local half of federation, and the only half that exists: a domain
/// naming an org on another server parses and does not resolve, which is
/// the boundary ADR 0003 records rather than hides.
pub struct LocalHomes {
    data_root: PathBuf,
    /// Domain → org slug. A name, not an address (`wiki.ref.redirect`),
    /// so it is a lookup rather than DNS. Built by
    /// [`crate::wiki_domains`], the same map wiki subscriptions use.
    domains: HashMap<String, String>,
    /// The org doing the reading. Its own domain resolves as local.
    reader_org: String,
    reader_root: PathBuf,
}

impl LocalHomes {
    #[must_use]
    pub fn new(
        data_root: PathBuf,
        domains: HashMap<String, String>,
        reader_org: impl Into<String>,
        reader_root: PathBuf,
    ) -> Self {
        Self {
            data_root,
            domains,
            reader_org: reader_org.into(),
            reader_root,
        }
    }

    /// Whether the reader subscribes to the library that publishes this
    /// node. A missing or unreadable store is "no subscription" — a
    /// refusal, never an accidental allow.
    fn subscribes_to(&self, domain: &str, library: &str) -> bool {
        let store = wiki_live::subscriptions::SubscriptionStore::open(&self.reader_root);
        let Ok(subs) = store.active(&Subscriber::Vault) else {
            return false;
        };
        subs.iter()
            .any(|s| s.kind == SourceKind::Resource && s.domain == domain && s.slug == library)
    }

    /// Where a node's content sits inside its own org, if it is there.
    ///
    /// Every kind but one is a directory (or a manifest) under
    /// `<org>/resources/<library>/`. **Charts are the exception, and it
    /// is ADR 0004's cost showing up here**: a chart is a vault
    /// document now (`<org>/vault/Assets/Charts/<slug>.md`), so this
    /// looks on the shelf first and falls back to the frozen ADR 0003
    /// copies the migration deliberately left behind.
    ///
    /// The shelf branch keeps *resolution* working — a subscriber can
    /// still name and follow a foreign `chart:<slug>`, and the
    /// subscription is still what decides it, which is the security
    /// claim `tests/integration/tests/setlist.rs` watches change. What
    /// it does **not** restore is *fetching*: the returned `rel_path`
    /// names a path under `vault/`, and no cross-org route serves that.
    /// `GET /org/{slug}/media/{*path}` reads `resources/`, and
    /// `SourceKind::Resource` materialisation copies
    /// `<org>/resources/<slug>/`. So a subscribed reader learns a chart
    /// exists and cannot read its bytes.
    ///
    /// That is a regression against what ADR 0003 shipped, it is
    /// recorded in `docs/spec/unmet.md`, and ADR 0004 says plainly that
    /// nothing should land before an Assets-tier reach path exists.
    /// Left legible here rather than papered over: a `Reach::Reachable`
    /// pointing at an unservable path is a bug somebody can find, where
    /// a blanket `NotFound` would look like the chart was never there.
    fn locate(&self, org: &str, library: &str, node: &NodeRef) -> Option<(String, PathBuf)> {
        let org_dir = self.data_root.join("orgs").join(org);
        if node.kind == NodeKind::Chart {
            let rel = resources_proto::assets::chart_path(&node.id);
            let path = org_dir.join("vault").join(&rel);
            if path.exists() {
                return Some((format!("vault/{rel}"), path));
            }
        }
        let base = org_dir.join("resources").join(library);
        let candidates = match node.kind {
            NodeKind::Chart => vec![format!("{}.kf", node.id), node.id.clone()],
            _ => vec![node.id.clone(), format!("{}.md", node.id)],
        };
        candidates.into_iter().find_map(|name| {
            let path = base.join(&name);
            path.exists()
                .then(|| (format!("resources/{library}/{name}"), path))
        })
    }
}

impl NodeHomes for LocalHomes {
    fn resolve(&self, node: &NodeRef) -> ResolvedNode {
        let refused = |reach| ResolvedNode::refused(node.clone(), reach);

        let Some(org) = self.domains.get(&node.domain) else {
            return refused(Reach::UnknownDomain);
        };
        // An org may hold a reference qualified with its own domain —
        // written by someone reading it from elsewhere, or carried in by
        // a copied setlist. That is not a foreign reference.
        if org == &self.reader_org {
            return ResolvedNode {
                node: node.clone(),
                reach: Reach::Local,
                org: org.clone(),
                title: String::new(),
                rel_path: String::new(),
            };
        }
        let Some(library) = library_of(node.kind) else {
            // Nothing outside the library kinds is published across an
            // org boundary today: a note or a block is vault-internal.
            return refused(Reach::NotPermitted);
        };
        if !self.subscribes_to(&node.domain, library) {
            return refused(Reach::NotPermitted);
        }
        match self.locate(org, library, node) {
            Some((rel_path, _)) => ResolvedNode {
                node: node.clone(),
                reach: Reach::Reachable,
                org: org.clone(),
                title: String::new(),
                rel_path,
            },
            None => refused(Reach::NotFound),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every library kind maps to the directory that publishes it, and
    /// the vault-internal kinds map to nothing — which is what makes a
    /// cross-org `note:` a refusal rather than a path traversal.
    #[test]
    fn library_kinds_have_a_home_and_the_rest_do_not() {
        assert_eq!(library_of(NodeKind::Chart), Some("charts"));
        assert_eq!(library_of(NodeKind::Song), Some("songs"));
        assert_eq!(library_of(NodeKind::Patch), Some("patches"));
        assert_eq!(library_of(NodeKind::Sample), Some("samples"));
        assert_eq!(library_of(NodeKind::Lighting), Some("lighting"));
        for kind in [NodeKind::Note, NodeKind::Block, NodeKind::Wiki] {
            assert_eq!(library_of(kind), None);
        }
    }

    /// A domain nobody answers to is `UnknownDomain`, and the reader's
    /// own domain is local — neither is a refusal, and neither touches
    /// the filesystem.
    #[test]
    fn an_unknown_domain_and_the_readers_own_are_distinguished() {
        let tmp = tempfile::tempdir().unwrap();
        let mut domains = HashMap::new();
        domains.insert("mine.test".to_owned(), "mine".to_owned());
        let homes = LocalHomes::new(
            tmp.path().to_path_buf(),
            domains,
            "mine",
            tmp.path().join("orgs/mine"),
        );

        let stranger = NodeRef::song("hosanna").in_domain("nobody.test");
        assert_eq!(homes.resolve(&stranger).reach, Reach::UnknownDomain);

        let own = NodeRef::song("hosanna").in_domain("mine.test");
        let resolved = homes.resolve(&own);
        assert_eq!(resolved.reach, Reach::Local);
        assert_eq!(resolved.org, "mine");
    }

    /// A known org the reader does not subscribe to is refused, and the
    /// refusal says nothing about whether the node is there.
    #[test]
    fn a_known_org_without_a_subscription_is_refused() {
        let tmp = tempfile::tempdir().unwrap();
        let chart = tmp.path().join("orgs/guest/resources/charts");
        std::fs::create_dir_all(&chart).unwrap();
        std::fs::write(chart.join("hosanna.kf"), "| C |").unwrap();
        let mut domains = HashMap::new();
        domains.insert("guest.example".to_owned(), "guest".to_owned());
        domains.insert("mine.test".to_owned(), "mine".to_owned());
        let homes = LocalHomes::new(
            tmp.path().to_path_buf(),
            domains,
            "mine",
            tmp.path().join("orgs/mine"),
        );

        let node = NodeRef::new(NodeKind::Chart, "hosanna").in_domain("guest.example");
        let resolved = homes.resolve(&node);
        assert_eq!(resolved.reach, Reach::NotPermitted);
        // The chart exists, and the answer does not admit that.
        assert_eq!(resolved.rel_path, "");
    }

    /// A chart on the ADR 0004 shelf is located there, and the path it
    /// comes back with is the vault one.
    ///
    /// The second half of this test is the honest part: that path is
    /// not servable across an org boundary by anything that exists, so
    /// what a subscriber gets is a resolvable reference to bytes they
    /// cannot fetch. Pinned rather than hidden — when the Assets reach
    /// path lands, this assertion is what changes.
    #[test]
    fn a_chart_resolves_off_the_vault_shelf_and_names_an_unservable_path() {
        let tmp = tempfile::tempdir().unwrap();
        let shelf = tmp
            .path()
            .join("orgs/guest/vault")
            .join(resources_proto::assets::charts_dir());
        std::fs::create_dir_all(&shelf).unwrap();
        std::fs::write(
            shelf.join("hosanna.md"),
            "---\ntype: asset\nasset_kind: chart\nslug: hosanna\n---\n",
        )
        .unwrap();

        let mut domains = HashMap::new();
        domains.insert("guest.example".to_owned(), "guest".to_owned());
        let reader_root = tmp.path().join("orgs/mine");
        std::fs::create_dir_all(&reader_root).unwrap();
        let store = wiki_live::subscriptions::SubscriptionStore::open(&reader_root);
        store
            .subscribe(
                &Subscriber::Vault,
                wiki_proto::subscription::Subscription {
                    domain: "guest.example".into(),
                    slug: "charts".into(),
                    kind: SourceKind::Resource,
                    title: "Charts".into(),
                    core: false,
                    declined: false,
                },
            )
            .expect("subscribe");

        let homes = LocalHomes::new(tmp.path().to_path_buf(), domains, "mine", reader_root);
        let node = NodeRef::new(NodeKind::Chart, "hosanna").in_domain("guest.example");
        let resolved = homes.resolve(&node);
        assert_eq!(resolved.reach, Reach::Reachable);
        assert_eq!(
            resolved.rel_path, "vault/Assets/Charts/hosanna.md",
            "the shelf is where a chart is now"
        );
        assert!(
            !resolved.rel_path.starts_with("resources/"),
            "and `/org/{{slug}}/media/` serves only `resources/` — \
             the gap ADR 0004 names, recorded in docs/spec/unmet.md"
        );
    }
}
