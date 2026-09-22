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
//! again, and it is why `wiki_live::materialize::refresh_shelf` can
//! bring a foreign song library down onto disk where
//! `refresh_resource` never could.
//!
//! # A publisher on another server answers from the reader's own copy
//!
//! The subscription lane crosses a server boundary now
//! (`task_server::federated_orgs`), which means a reader can hold a
//! foreign library without the publishing org being on this disk at all.
//! So resolution looks in two places: the publisher's own tree where
//! there is one, and `subscribed/<domain>/<library>/` — the copy a
//! refresh wrote — where there is not.
//!
//! Nothing about the rule above changes. The copy exists *because* the
//! reader subscribed, so "a subscription is what authorises" is still the
//! whole of it; what is new is that the subscription no longer has to be
//! to somebody on the same machine. A reader with no subscription reaches
//! nothing, and the copy is not consulted, because there is none.
//!
//! # What is deliberately not distinguished
//!
//! [`Reach::NotPermitted`] covers both "you are not subscribed" and, for
//! an unsubscribed reader, "there is nothing there". Telling an outsider
//! which slugs exist is exactly the enumeration a private source
//! refuses, and `wiki.access.visibility` already draws that line.
//!
//! [`Reach::UnknownDomain`] is now the answer only when this deployment
//! knows no org of that domain **and** the reader holds no subscription to
//! it. With a subscription the domain is known by a better authority than
//! the map: the reader took it on.

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
///
/// [`NodeKind::Project`] is `None` too, and for a third reason again —
/// see [`PROJECTS_TIER`]. A project is not published *by* a library; it
/// is itself the thing another org subscribes to, so the subscription
/// slug is the project's own id rather than a directory that holds many
/// of them.
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
        | NodeKind::Project
        // A collection is not library material and is not published by
        // one: it is an org's own arrangement of references, the way a
        // note is its own writing.
        | NodeKind::Collection
        | NodeKind::External => return None,
    })
}

/// The tier a `project:` reference resolves into.
///
/// # A project is its own library
///
/// Every other qualified kind here names a slug inside a *shared*
/// directory: `song:hosanna` is one of many under `assets/songs/`, and
/// subscribing to `songs` admits the reader to all of them. A project is
/// not shaped like that. `<org>/projects/example-album/` is one project
/// and nothing else, and it is the unit somebody publishes — a mix
/// engineer is given a song, not "the projects library".
///
/// So [`library_of`] answers `None` and the subscription slug is the
/// project's own id. `example-album` and `example-album/track-two` are
/// two subscribable things, which is exactly the submodule model
/// `org_proto::OrgRoot::project_shelves` describes: a person may take
/// the album, or take one song out of it, and the slug says which.
pub const PROJECTS_TIER: &str = "projects";

/// Whether a stranger's string may be joined onto a path here.
///
/// A reference is written by anybody — its domain and its id are strings
/// that arrive from another organisation, a copied setlist, a page
/// somebody typed — and resolution joins both onto directories inside this
/// data root. `..`, an absolute path (which `Path::join` honours by
/// *discarding* everything to its left), a bare `.`, a Windows separator:
/// any of them turns "resolve a reference" into "read somewhere nobody
/// published".
///
/// One segment, then, and nothing that is not a name. Not
/// [`org_proto::wiki_slug`], because a domain legitimately contains dots
/// and that function would rewrite `vnt.test` into `vnt-test`; the
/// property needed here is narrower and is exactly traversal.
///
/// A refusal here is [`Reach::NotFound`] or a skipped candidate rather
/// than an error: a reference that cannot name anything does not name
/// anything, and saying more would tell the writer which shapes are
/// interesting.
fn is_one_safe_segment(s: &str) -> bool {
    !s.is_empty()
        && s != "."
        && s != ".."
        && !s.contains('/')
        && !s.contains('\\')
        && !s.contains('\0')
}

/// Resolves qualified references against everything this data root holds
/// — the orgs on it, and the copies its subscriptions brought down.
///
/// Both halves answer the same question and neither widens access. A
/// publisher on this disk answers from its own tree, which is the live
/// one. A publisher on another server answers from **the reader's copy**,
/// which exists only because the reader subscribed — so the rule is
/// unchanged: a reference addresses, a subscription authorises.
///
/// `Reach::Reachable` for both, deliberately. `wiki.subscribe.federated`
/// says that whether a source is local, on a peer or on the central
/// deployment "changes latency and nothing else a reader or writer can
/// observe", and a third reach value would be exactly such an
/// observation.
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
            matches!(
                s.kind,
                SourceKind::Resource | SourceKind::Assets | SourceKind::Projects
            ) && s.domain == domain
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
        // The id is a stranger's string and this is where it meets a
        // directory — see [`is_one_safe_segment`].
        if !is_one_safe_segment(&node.id) {
            return None;
        }
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

        // The publishing org, **if this deployment hosts it**. It may not,
        // and that is no longer the end of the question: a subscription to
        // a source on another server leaves a copy on this disk, and a
        // reference the reader subscribed to resolves from that copy
        // (`wiki.subscribe.resolution`). So an unknown domain is only a
        // refusal once there is no subscription either.
        let org = self.domains.get(&node.domain);
        // An org may hold a reference qualified with its own domain —
        // written by someone reading it from elsewhere, or carried in by
        // a copied setlist. That is not a foreign reference.
        if org.is_some_and(|o| o == &self.reader_org) {
            return ResolvedNode {
                node: node.clone(),
                reach: Reach::Local,
                org: self.reader_org.clone(),
                title: String::new(),
                rel_path: String::new(),
            };
        }
        // A project is its own library, so it takes the same shape one
        // step earlier: the subscription slug IS the project's id, and
        // the content is that project's declaring page. See
        // [`PROJECTS_TIER`] on why it does not go through `library_of`.
        //
        // No copy branch here, and that is the byte-tree decision showing
        // through: a project does not cross as a subscription, because a
        // project *is* its media and a subscription carries names. So the
        // only project a reference can reach is one published on this data
        // root; the rest is a File Root, which is a different question
        // from resolving a name.
        if node.kind == NodeKind::Project {
            let Some(org) = org else {
                return refused(Reach::UnknownDomain);
            };
            if !self.subscribes_to(&node.domain, &node.id) {
                return refused(Reach::NotPermitted);
            }
            let rel = format!("{PROJECTS_TIER}/{}/{}", node.id, org_proto::PROJECT_PAGE);
            let path = self.data_root.join("orgs").join(org).join(&rel);
            return if path.exists() {
                ResolvedNode {
                    node: node.clone(),
                    reach: Reach::Reachable,
                    org: org.clone(),
                    title: String::new(),
                    rel_path: rel,
                }
            } else {
                refused(Reach::NotFound)
            };
        }
        let Some(library) = library_of(node.kind) else {
            // Nothing outside the library kinds is published across an
            // org boundary today: a note or a block is vault-internal.
            return refused(Reach::NotPermitted);
        };
        if !self.subscribes_to(&node.domain, library) {
            // Unqualified by a subscription, the two answers differ in
            // what they admit knowing: a domain this deployment has never
            // heard of, against one it hosts and will not open.
            return refused(if org.is_some() {
                Reach::NotPermitted
            } else {
                Reach::UnknownDomain
            });
        }
        // The publisher's own tree first, where there is one: it is the
        // live copy, and a reader on the same disk should see an edit the
        // moment it lands rather than at their next refresh.
        if let Some(org) = org
            && let Some((rel_path, _)) = self.locate(org, library, node)
        {
            return ResolvedNode {
                node: node.clone(),
                reach: Reach::Reachable,
                org: org.clone(),
                title: String::new(),
                rel_path,
            };
        }
        // Then the reader's own copy, which is the whole of what a
        // subscription to another server leaves behind. `Reachable` and
        // not a third state on purpose: `wiki.subscribe.federated` says
        // whether a source is local, on a peer or on the central
        // deployment changes latency "and nothing else a reader or writer
        // can observe".
        match self.subscribed_copy(library, node) {
            Some(rel_path) => ResolvedNode {
                node: node.clone(),
                reach: Reach::Reachable,
                org: self.reader_org.clone(),
                title: String::new(),
                rel_path,
            },
            // Subscribed and nothing there: either the source does not
            // hold it, or nothing has been refreshed yet. Both are "the
            // reader may look, and there is nothing at that address",
            // which is what `NotFound` says.
            None => refused(Reach::NotFound),
        }
    }
}

impl LocalHomes {
    /// The reader's own copy of a foreign library, when a subscription
    /// brought one down: `subscribed/<domain>/<library>/…`.
    ///
    /// The same candidate names [`Self::locate`] tries, because the copy
    /// is the publisher's tree — a refresh writes what the manifest said,
    /// not a renamed version of it.
    ///
    /// Returned **relative to the reader's own org root**, which is where
    /// the content actually is. That is the invariant every caller of
    /// `ResolvedNode` relies on: `org` names whose root, `rel_path` names
    /// the place inside it, and joining the two reaches the file. For a
    /// publisher on another server the two together name the copy, and
    /// they have to, because the publisher's root is not on this disk.
    fn subscribed_copy(&self, library: &str, node: &NodeRef) -> Option<String> {
        // Two stranger's strings here rather than one: the copy is
        // addressed by the publishing *domain*, so the domain is joined
        // onto a path too. Both are checked.
        if !is_one_safe_segment(&node.domain) || !is_one_safe_segment(&node.id) {
            return None;
        }
        let base = self
            .reader_root
            .join("subscribed")
            .join(&node.domain)
            .join(library);
        let candidates = match node.kind {
            NodeKind::Chart | NodeKind::Song => {
                vec![format!("{}.md", node.id), format!("{}.kf", node.id)]
            }
            _ => vec![node.id.clone(), format!("{}.md", node.id)],
        };
        candidates.into_iter().find_map(|name| {
            base.join(&name)
                .exists()
                .then(|| format!("subscribed/{}/{library}/{name}", node.domain))
        })
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
        assert_eq!(
            library_of(NodeKind::Project),
            None,
            "a project is its own library — see PROJECTS_TIER"
        );
    }

    /// A `project:` reference is a refusal without a subscription and a
    /// path with one, and the path it names is the project's own
    /// declaring page on the Projects tier.
    ///
    /// This is the mechanism behind "a project can be referenced as
    /// needed" — the sentence ADR 0004's fourth root rests on now that a
    /// project is not a vault note. Before this kind existed,
    /// `NodeKind::parse("project")` returned `None` and the reference
    /// did not even parse.
    #[test]
    fn a_project_reference_resolves_to_the_projects_tier() {
        let tmp = tempfile::tempdir().unwrap();
        let mut domains = HashMap::new();
        domains.insert("acme.test".to_owned(), "acme".to_owned());
        let reader = tmp.path().join("orgs/reader");
        std::fs::create_dir_all(&reader).unwrap();
        let homes = LocalHomes::new(tmp.path().to_path_buf(), domains, "reader", reader.clone());

        let node = NodeRef::new(NodeKind::Project, "example-album").in_domain("acme.test");
        assert_eq!(
            homes.resolve(&node).reach,
            Reach::NotPermitted,
            "no subscription, and an outsider is never told whether it exists"
        );

        // Subscribe to that one project, by its own path.
        let store = wiki_live::subscriptions::SubscriptionStore::open(&reader);
        store
            .subscribe(
                &Subscriber::Vault,
                wiki_proto::subscription::Subscription {
                    domain: "acme.test".to_owned(),
                    slug: "example-album".to_owned(),
                    kind: SourceKind::Projects,
                    title: "Example Album".to_owned(),
                    core: false,
                    declined: false,
                    selection: org_proto::Selection::All,
                },
            )
            .expect("hold a subscription");

        assert_eq!(
            homes.resolve(&node).reach,
            Reach::NotFound,
            "subscribed, and the publisher does not hold it — a different answer"
        );

        let page = tmp
            .path()
            .join("orgs/acme/projects/example-album")
            .join(org_proto::PROJECT_PAGE);
        std::fs::create_dir_all(page.parent().unwrap()).unwrap();
        std::fs::write(&page, "---\ntype: project\ntitle: Example Album\n---\n").unwrap();

        let resolved = homes.resolve(&node);
        assert_eq!(resolved.reach, Reach::Reachable);
        assert_eq!(resolved.org, "acme");
        assert_eq!(
            resolved.rel_path, "projects/example-album/project.md",
            "the answer names the publisher's own path, as every other kind's does"
        );
    }

    /// A reference into an org this deployment does **not** host resolves
    /// from the reader's own copy — and only because the reader
    /// subscribed.
    ///
    /// The boundary ADR 0003 recorded, moved. Three states in order, which
    /// is the only way to show that the subscription is what does the
    /// work: no subscription and an unknown domain (nothing is admitted,
    /// and nothing is admitted to *knowing*); subscribed with nothing
    /// pulled yet; subscribed with the copy on disk.
    ///
    /// t[verify wiki.subscribe.resolution] — across a server boundary.
    #[test]
    fn a_reference_to_another_server_resolves_from_the_readers_copy() {
        let tmp = tempfile::tempdir().unwrap();
        // `vnt.test` is deliberately absent from the map: this deployment
        // hosts no org of that domain, which is the whole point.
        let mut domains = HashMap::new();
        domains.insert("acme.test".to_owned(), "reader".to_owned());
        let reader = tmp.path().join("orgs/reader");
        std::fs::create_dir_all(&reader).unwrap();
        let homes = LocalHomes::new(tmp.path().to_path_buf(), domains, "reader", reader.clone());

        let node = NodeRef::new(NodeKind::Song, "reel-theme").in_domain("vnt.test");
        assert_eq!(
            homes.resolve(&node).reach,
            Reach::UnknownDomain,
            "with no subscription this deployment has never heard of that domain"
        );

        let store = wiki_live::subscriptions::SubscriptionStore::open(&reader);
        store
            .subscribe(
                &Subscriber::Vault,
                wiki_proto::subscription::Subscription {
                    domain: "vnt.test".to_owned(),
                    slug: "songs".to_owned(),
                    kind: SourceKind::Assets,
                    title: "VNT songs".to_owned(),
                    core: false,
                    declined: false,
                    selection: org_proto::Selection::All,
                },
            )
            .expect("hold a subscription");
        assert_eq!(
            homes.resolve(&node).reach,
            Reach::NotFound,
            "subscribed and nothing refreshed yet: the reader may look, and \
             there is nothing at that address"
        );

        // What a refresh writes: the publisher's tree, under the address a
        // reference already uses.
        let copy = reader.join("subscribed/vnt.test/songs");
        std::fs::create_dir_all(&copy).unwrap();
        std::fs::write(copy.join("reel-theme.md"), "---\ntitle: Reel Theme\n---\n").unwrap();

        let resolved = homes.resolve(&node);
        assert_eq!(resolved.reach, Reach::Reachable);
        assert_eq!(
            resolved.org, "reader",
            "the content is in the reader's own root, because that is where \
             the copy is — `org` and `rel_path` still join to the file"
        );
        assert_eq!(resolved.rel_path, "subscribed/vnt.test/songs/reel-theme.md");
        assert!(
            tmp.path()
                .join("orgs")
                .join(&resolved.org)
                .join(&resolved.rel_path)
                .is_file(),
            "the answer must name a file that is actually there"
        );
    }

    /// A reference whose id or domain would climb out of the tree names
    /// nothing, on either route.
    ///
    /// The id has been joined onto a path here since before this module
    /// resolved anything remote, and the subscription gate is not what
    /// stops it: a reader may hold a perfectly ordinary subscription and
    /// still write `song:../../../vault/Private`. What stops it is the
    /// name having to be a name.
    #[test]
    fn a_reference_that_would_climb_out_of_the_tree_resolves_to_nothing() {
        let tmp = tempfile::tempdir().unwrap();
        let mut domains = HashMap::new();
        domains.insert("acme.test".to_owned(), "acme".to_owned());
        let reader = tmp.path().join("orgs/reader");
        std::fs::create_dir_all(&reader).unwrap();
        // Something worth reaching, one level above the library.
        std::fs::create_dir_all(tmp.path().join("orgs/acme/assets")).unwrap();
        std::fs::write(tmp.path().join("orgs/acme/assets/secret.md"), "private\n").unwrap();
        std::fs::create_dir_all(reader.join("subscribed/acme.test")).unwrap();
        std::fs::write(reader.join("subscribed/acme.test/secret.md"), "private\n").unwrap();
        let homes = LocalHomes::new(tmp.path().to_path_buf(), domains, "reader", reader.clone());

        let store = wiki_live::subscriptions::SubscriptionStore::open(&reader);
        for domain in ["acme.test", "vnt.test"] {
            store
                .subscribe(
                    &Subscriber::Vault,
                    wiki_proto::subscription::Subscription {
                        domain: domain.to_owned(),
                        slug: "songs".to_owned(),
                        kind: SourceKind::Assets,
                        title: "songs".to_owned(),
                        core: false,
                        declined: false,
                        selection: org_proto::Selection::All,
                    },
                )
                .expect("hold a subscription");
        }

        // The publisher's own tree, reached through a subscribed library.
        let climbing = NodeRef::new(NodeKind::Song, "../secret").in_domain("acme.test");
        assert_eq!(homes.resolve(&climbing).reach, Reach::NotFound);
        // And the copy route, where the domain is a path segment too.
        let sideways = NodeRef::new(NodeKind::Song, "secret").in_domain("..");
        assert_eq!(
            homes.resolve(&sideways).reach,
            Reach::UnknownDomain,
            "`..` is no domain, subscribed or not"
        );
        assert!(
            !is_one_safe_segment("/etc"),
            "an absolute path is not a name"
        );
    }

    /// A sub-project is subscribed to on its own terms, and holding the
    /// parent is not holding the child. That is the submodule model at
    /// the reference layer: `Depth::Surface` is the default, so a
    /// subscriber who took the album has a *reference* to the song and
    /// not its bytes.
    #[test]
    fn a_subproject_is_a_subscription_of_its_own() {
        let tmp = tempfile::tempdir().unwrap();
        let mut domains = HashMap::new();
        domains.insert("acme.test".to_owned(), "acme".to_owned());
        let reader = tmp.path().join("orgs/reader");
        std::fs::create_dir_all(&reader).unwrap();
        let homes = LocalHomes::new(tmp.path().to_path_buf(), domains, "reader", reader.clone());

        for rel in ["example-album", "example-album/track-two"] {
            let page = tmp
                .path()
                .join("orgs/acme/projects")
                .join(rel)
                .join(org_proto::PROJECT_PAGE);
            std::fs::create_dir_all(page.parent().unwrap()).unwrap();
            std::fs::write(&page, "---\ntype: project\n---\n").unwrap();
        }

        let store = wiki_live::subscriptions::SubscriptionStore::open(&reader);
        store
            .subscribe(
                &Subscriber::Vault,
                wiki_proto::subscription::Subscription {
                    domain: "acme.test".to_owned(),
                    slug: "example-album".to_owned(),
                    kind: SourceKind::Projects,
                    title: "Example Album".to_owned(),
                    core: false,
                    declined: false,
                    selection: org_proto::Selection::All,
                },
            )
            .expect("hold a subscription");

        let album = NodeRef::new(NodeKind::Project, "example-album").in_domain("acme.test");
        let song =
            NodeRef::new(NodeKind::Project, "example-album/track-two").in_domain("acme.test");
        assert_eq!(homes.resolve(&album).reach, Reach::Reachable);
        assert_eq!(
            homes.resolve(&song).reach,
            Reach::NotPermitted,
            "taking the album is not taking the song — surface-only is the default, \
             and the song stays a reference until somebody asks for it"
        );
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
    /// (`wiki_live::materialize::refresh_shelf`), so resolving and
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
