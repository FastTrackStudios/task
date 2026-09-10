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
//! # ADR 0004 moved charts and songs onto their own shelves
//!
//! A chart is an asset-group document now
//! (`<org>/assets/charts/<slug>.md`), and **the group name is the
//! subscription slug** — the same string `library_of` already returned.
//! So nothing about this module's rule changed: the source that
//! publishes `chart:doxology` is still the one whose slug is `charts`,
//! and a reader subscribed to `guest.example/charts` may still follow
//! any `guest.example/chart:*`. What changed is only which of the two
//! trees on disk that slug names, and [`SourceKind::Assets`] is how the
//! subscription says which.
//!
//! An earlier draft filed these under `<vault>/Assets/` instead, and
//! that draft could not do this at all: a vault is never subscribable
//! (`wiki.boundary.no-subscribe`), so a chart inside one was resolvable
//! and unfetchable — a `Reach::Reachable` naming a path nothing served.
//! Moving the shelf out of the vault is what makes the two halves agree
//! again, and it is why `wiki_live::materialize::refresh_assets` can
//! bring a foreign song library down onto disk where
//! `refresh_resource` never could.
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
        // Either kind admits: `charts` is an asset group and `patches`
        // is a resource library, and which one a slug denotes is the
        // publisher's business rather than the reader's. Matching on
        // the slug and not the kind is what keeps a subscription taken
        // out before ADR 0004 working after it.
        subs.iter().any(|s| {
            matches!(s.kind, SourceKind::Resource | SourceKind::Assets)
                && s.domain == domain
                && s.slug == library
        })
    }

    /// Where a node's content sits inside its own org, if it is there.
    ///
    /// Two trees, and which one is asked first follows ADR 0004's tier
    /// rule rather than the node's kind: an **asset group**
    /// (`<org>/assets/<library>/`) holds what people type into, a
    /// **resource library** (`<org>/resources/<library>/`) holds what
    /// they import. Charts and songs moved; patches, samples and
    /// lighting did not, and nobody co-edits a WAV.
    ///
    /// The asset shelf is tried first and the resources tier is the
    /// fallback, because the migration **copies and deletes nothing**
    /// (`ResourcesBackend::migrate_charts`): both trees hold a chart on
    /// a migrated deployment, one of them frozen at migration time. The
    /// live one has to win, and the live one is the shelf.
    ///
    /// The returned `rel_path` is org-relative, so a caller can see
    /// which tier answered — `assets/charts/hosanna.md` against
    /// `resources/patches/warm-pad/patch.md`.
    fn locate(&self, org: &str, library: &str, node: &NodeRef) -> Option<(String, PathBuf)> {
        let org_dir = self.data_root.join("orgs").join(org);
        if matches!(node.kind, NodeKind::Chart | NodeKind::Song) {
            let rel = format!("{}.md", node.id);
            let path = org_dir.join("assets").join(library).join(&rel);
            if path.exists() {
                return Some((format!("assets/{library}/{rel}"), path));
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
    /// comes back with is one another organisation can actually reach.
    ///
    /// This assertion is the inversion of the one it replaces. Under
    /// the `<vault>/Assets/` draft the same test ended
    /// `assert!(!rel_path.starts_with("resources/"))` with a comment
    /// explaining that nothing served the path it *did* start with —
    /// the regression ADR 0004 said should not have landed. It names an
    /// asset group now, which is a shelf a subscription materialises
    /// (`wiki_live::materialize::refresh_assets`), so resolving and
    /// fetching agree again.
    #[test]
    fn a_chart_resolves_off_its_asset_shelf_and_names_a_reachable_path() {
        let tmp = tempfile::tempdir().unwrap();
        let shelf = tmp.path().join("orgs/guest/assets/charts");
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
                    kind: SourceKind::Assets,
                    title: "Charts".into(),
                    core: false,
                    declined: false,
                    selection: Default::default(),
                },
            )
            .expect("subscribe");

        let homes = LocalHomes::new(tmp.path().to_path_buf(), domains, "mine", reader_root);
        let node = NodeRef::new(NodeKind::Chart, "hosanna").in_domain("guest.example");
        let resolved = homes.resolve(&node);
        assert_eq!(resolved.reach, Reach::Reachable);
        assert_eq!(
            resolved.rel_path, "assets/charts/hosanna.md",
            "the asset group is where a chart is now, and it is subscribable"
        );
        assert!(
            !resolved.rel_path.starts_with("vault/"),
            "a vault is never subscribable, so a shelf inside one could not be reached"
        );
    }
}
