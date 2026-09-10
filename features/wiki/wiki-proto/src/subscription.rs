//! What a vault or wiki holds when it subscribes to something.
//!
//! A subscription is the live form of a reference: citing a source
//! takes a snapshot the citer then owns, while subscribing keeps a
//! channel open to a body that goes on being corrected
//! (`features/wiki/spec/wiki.md`).
//!
//! Three things can be subscribed to and a vault is not one of them
//! ([`SourceKind`], `wiki.boundary.no-subscribe`). The subscriber, by
//! contrast, *may* be a vault: that asymmetry is the whole distinction
//! between the two, so it is in the types rather than in a check
//! somebody has to remember to write.
//!
//! # These two enums shadow [`org_proto::Shelf`], deliberately
//!
//! [`Subscriber`] is "which of an org's shelves holds this
//! subscription" and [`SourceKind`] is "which tier the far side
//! belongs to" — both questions [`org_proto::Tier`] also answers. They
//! are separate types anyway, and the reason is that these two are
//! **wire** types: they are `Facet`-derived, they ride vox method
//! payloads, and they are what a `Subscription` serialises to on disk.
//! A wire enum has to be able to hold a variant a future peer sends and
//! this build has never heard of; a domain trait has to be able to gain
//! a method without a schema stamp moving. Fusing them would tie a
//! protocol change to every layout change and back.
//!
//! What they do owe each other is agreement, and that is what
//! [`SourceKind::of_tier`] and [`Subscriber::of_shelf`] are for: one
//! conversion each, in one place, so "an asset shelf is subscribable"
//! is decided by [`org_proto::Tier::is_subscribable`] and never
//! re-derived here.

use facet::Facet;
pub use org_proto::Selection;
use org_proto::{Shelf, Tier};
use serde::{Deserialize, Serialize};

/// What kind of thing is being subscribed to.
///
/// There is no `Vault` member, and that is the point: a vault cannot
/// be subscribed to by any route, so the type cannot express it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum SourceKind {
    /// Authored knowledge. Editable locally, and changes go back as
    /// Edit Requests.
    Wiki,
    /// An external work — a book, a video, a scripture text. Never
    /// authored here and never edit-requestable
    /// (`wiki.resource.not-a-wiki`).
    Resource,
    /// One asset shelf of a publishing org — `<org>/assets/<kind>/`,
    /// ADR 0004's Assets tier. A song library, a chart library.
    ///
    /// Editable, like a wiki and unlike a Resource: an asset is a thing
    /// people type into, which is the whole reason ADR 0004 moved
    /// charts and songs off the resources tier. Its slug is the *kind*
    /// (`songs`, `charts`), because a kind is the unit an org publishes
    /// — see [`org_proto::AssetShelf`].
    Assets,
    /// One project of a publishing org — `<org>/projects/<name>/`, the
    /// fourth root of ADR 0004 decision 1.
    ///
    /// Its slug is the project's **tier-relative path**, not a library
    /// name, and that is the difference from every other member here.
    /// `Assets` publishes a directory holding many songs, so its slug is
    /// the kind; a project is one thing, and it is itself the unit
    /// somebody publishes — a mix engineer is given a song, not "the
    /// projects library". So `example-album` and
    /// `example-album/track-two` are two subscribable slugs, which is
    /// the submodule model in `org_proto::OrgRoot::project_shelves`.
    ///
    /// Editable, like a wiki and an asset shelf: a project tree is
    /// written by the tools that made it, and a collaborator given one
    /// is given it to work in.
    Projects,
}

impl SourceKind {
    /// Whether a subscriber may edit their local copy of this kind.
    ///
    /// t[impl wiki.subscribe.editability] — editability follows the
    /// kind rather than a flag somebody sets per subscription, so a
    /// reader is told the spine is fixed instead of finding out when a
    /// push is refused.
    #[must_use]
    pub const fn is_editable(self) -> bool {
        matches!(self, Self::Wiki | Self::Assets | Self::Projects)
    }

    /// The wire kind for a publishable tier, or `None` for a tier that
    /// is never subscribable.
    ///
    /// The `None` arm is [`Tier::Vault`] and it is the type-level form
    /// of `wiki.boundary.no-subscribe`: there is no `SourceKind` a
    /// vault could be converted into, so no caller can construct the
    /// subscription this rule forbids, however hard it tries.
    #[must_use]
    pub const fn of_tier(tier: Tier) -> Option<Self> {
        match tier {
            Tier::Vault => None,
            Tier::Wiki => Some(Self::Wiki),
            Tier::Assets => Some(Self::Assets),
            Tier::Projects => Some(Self::Projects),
        }
    }

    /// The tier directory this kind's files live in, under the
    /// publishing org's root: `wikis/`, `resources/`, `assets/`.
    ///
    /// One place, so the resolver that joins a stranger-supplied slug
    /// onto a path and the discovery that walks the same directory can
    /// never disagree about which tree is being opened.
    #[must_use]
    pub const fn tier_dir(self) -> &'static str {
        match self {
            Self::Wiki => "wikis",
            Self::Resource => "resources",
            Self::Assets => "assets",
            Self::Projects => "projects",
        }
    }

    /// The noun a refusal uses: "acme.test has no *song shelf* `songs`".
    #[must_use]
    pub const fn noun(self) -> &'static str {
        match self {
            Self::Wiki => "wiki",
            Self::Resource => "resource",
            Self::Assets => "asset shelf",
            Self::Projects => "project",
        }
    }
}

/// Who holds a subscription.
///
/// A vault may subscribe; a vault may not *be* subscribed to. Both
/// halves of that live here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[serde(rename_all = "snake_case", tag = "kind", content = "slug")]
#[repr(u8)]
pub enum Subscriber {
    /// The org's own vault.
    Vault,
    /// One of the org's wikis, by slug.
    Wiki(String),
    /// One of the org's asset shelves, by kind.
    ///
    /// A shelf subscribes for the same reason a wiki does: a chart
    /// library that cites another org's song library wants those songs
    /// to go on being corrected, not to hold a snapshot of them.
    Assets(String),
    /// One of the org's projects, by its tier-relative path. See
    /// [`SourceKind::Projects`].
    ///
    /// A project subscribes for the same reason a wiki does: an album
    /// citing another org's song library wants those songs to go on
    /// being corrected rather than to hold a snapshot. A *sub*-project
    /// subscribes on its own terms too — `example-album/track-two` is
    /// a subscriber key of its own, because a sub-project is a shelf
    /// and not a subtree.
    Projects(String),
}

impl Subscriber {
    /// The key this subscriber is stored under.
    ///
    /// Identical to [`org_proto::Shelf::subscriber_key`] by
    /// construction — [`Self::of_shelf`] is the only way the two are
    /// ever produced from one shelf, and the round trip is tested.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Vault => "vault".to_owned(),
            Self::Wiki(slug) => format!("wiki:{slug}"),
            Self::Assets(kind) => format!("assets:{kind}"),
            Self::Projects(name) => format!("project:{name}"),
        }
    }

    /// Parse a stored key back.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "vault" => Some(Self::Vault),
            other => {
                if let Some(slug) = other.strip_prefix("wiki:").filter(|s| !s.is_empty()) {
                    return Some(Self::Wiki(slug.to_owned()));
                }
                if let Some(kind) = other.strip_prefix("assets:").filter(|s| !s.is_empty()) {
                    return Some(Self::Assets(kind.to_owned()));
                }
                other
                    .strip_prefix("project:")
                    .filter(|s| !s.is_empty())
                    .map(|s| Self::Projects(s.to_owned()))
            }
        }
    }

    /// Who a shelf is, as a subscriber.
    ///
    /// Every shelf may subscribe, the vault included — that is the
    /// half of the asymmetry this type carries, and it is why there is
    /// no `Option` here while [`SourceKind::of_tier`] has one.
    #[must_use]
    pub fn of_shelf(shelf: &dyn Shelf) -> Self {
        match shelf.tier() {
            Tier::Vault => Self::Vault,
            Tier::Wiki => Self::Wiki(shelf.name().to_owned()),
            Tier::Assets => Self::Assets(shelf.name().to_owned()),
            Tier::Projects => Self::Projects(shelf.name().to_owned()),
        }
    }
}

/// One held subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Subscription {
    /// The publishing org's federation domain. This is a *name*, not
    /// a live address: it survives the org going away and another
    /// adopting the source (`wiki.ref.redirect`).
    pub domain: String,
    /// The source's slug, as it appears in a reference.
    pub slug: String,
    /// Wiki or Resource.
    pub kind: SourceKind,
    /// Display title, for a list a person reads.
    #[serde(default)]
    pub title: String,
    /// Whether this came from the deployment's core set rather than
    /// from someone choosing it (`wiki.core.default`).
    #[serde(default)]
    pub core: bool,
    /// Set when the subscriber turned a core subscription off. Kept
    /// rather than deleted, so a resubscription sweep re-offers it
    /// instead of silently restoring it (`wiki.core.optional`).
    #[serde(default)]
    pub declined: bool,
    /// **Which part** of the source this subscription takes
    /// (ADR 0004 decision 1a).
    ///
    /// Defaults to [`Selection::All`], which is what every subscription
    /// written before this field existed meant — so an old row on disk
    /// deserialises to the whole shelf, which is what it was.
    ///
    /// It is on the type from the first commit that had a subscribable
    /// asset shelf, and that ordering is the point. Whole-shelf-only is
    /// not a smaller version of granular subscription; it is a
    /// different contract, and every caller written against it would
    /// have to be revisited when the second contract arrived. The field
    /// costs one enum today and removes that migration entirely.
    #[serde(default)]
    pub selection: Selection,
}

impl Subscription {
    /// The qualified id a reference carries: `domain/slug`.
    #[must_use]
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.domain, self.slug)
    }

    /// Whether this subscription is currently in force.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        !self.declined
    }
}

/// Why a reference did not resolve, when it did not.
///
/// Each variant is a different thing to tell a reader, which is why
/// they are not one "unresolved". `wiki.subscribe.resolution` requires
/// an unknown *source* never be reported as a missing page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unresolved {
    /// The reference named a source this reader does not hold. Carries
    /// the qualified id so the reader can be offered a subscription.
    NoSubscription(String),
    /// A short-form reference matched more than one subscription.
    /// Ambiguity is reported with its candidates, never guessed.
    Ambiguous {
        slug: String,
        candidates: Vec<String>,
    },
    /// The source is held, but has no such page.
    NoSuchTarget { source: String, target: String },
}

impl std::fmt::Display for Unresolved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSubscription(id) => {
                write!(f, "not subscribed to `{id}`")
            }
            Self::Ambiguous { slug, candidates } => write!(
                f,
                "`{slug}` matches {} subscriptions: {}",
                candidates.len(),
                candidates.join(", ")
            ),
            Self::NoSuchTarget { source, target } => {
                write!(f, "`{source}` has no `{target}`")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_resource_is_not_editable_and_a_wiki_or_an_asset_is() {
        assert!(SourceKind::Wiki.is_editable());
        assert!(!SourceKind::Resource.is_editable());
        assert!(
            SourceKind::Assets.is_editable(),
            "an asset is a thing people type into; that is why it left the resources tier"
        );
    }

    #[test]
    fn subscriber_keys_round_trip() {
        for s in [
            Subscriber::Vault,
            Subscriber::Wiki("cooking".into()),
            Subscriber::Assets("songs".into()),
        ] {
            assert_eq!(Subscriber::from_key(&s.key()), Some(s.clone()), "{s:?}");
        }
        assert_eq!(Subscriber::from_key("wiki:"), None);
        assert_eq!(Subscriber::from_key("assets:"), None);
        assert_eq!(Subscriber::from_key("nonsense"), None);
    }

    /// The wire spelling and the layout spelling are one string, and
    /// this is where that is checked. If they ever drift, a
    /// subscription written by the boot sweep stops being found by the
    /// service that reads it — a bug with no error message.
    #[test]
    fn a_shelfs_subscriber_key_is_the_wire_subscriber_key() {
        let shelves: Vec<Box<dyn Shelf>> = vec![
            Box::new(org_proto::VaultShelf::new("/tmp/v".into())),
            Box::new(org_proto::WikiShelf::new("cooking".into(), "/tmp/w".into())),
            Box::new(org_proto::AssetShelf::new("songs".into(), "/tmp/a".into())),
        ];
        for shelf in &shelves {
            assert_eq!(
                Subscriber::of_shelf(shelf.as_ref()).key(),
                shelf.subscriber_key(),
                "{}",
                shelf.vault_id()
            );
        }
    }

    /// t[verify wiki.boundary.no-subscribe] — there is no `SourceKind`
    /// a vault converts to, so the forbidden subscription cannot be
    /// constructed rather than being rejected somewhere downstream.
    #[test]
    fn a_vault_has_no_source_kind_at_all() {
        assert_eq!(SourceKind::of_tier(Tier::Vault), None);
        assert_eq!(SourceKind::of_tier(Tier::Wiki), Some(SourceKind::Wiki));
        assert_eq!(SourceKind::of_tier(Tier::Assets), Some(SourceKind::Assets));
    }

    /// The tier directory is what a stranger's slug gets joined onto.
    #[test]
    fn each_kind_names_its_own_tier_directory() {
        assert_eq!(SourceKind::Wiki.tier_dir(), "wikis");
        assert_eq!(SourceKind::Resource.tier_dir(), "resources");
        assert_eq!(SourceKind::Assets.tier_dir(), "assets");
    }

    #[test]
    fn a_declined_core_subscription_is_kept_but_inactive() {
        let s = Subscription {
            domain: "fasttrackstudio.app".into(),
            slug: "bible".into(),
            kind: SourceKind::Resource,
            title: "Bible".into(),
            core: true,
            declined: true,
            selection: Default::default(),
        };
        assert!(!s.is_active());
        assert_eq!(s.qualified(), "fasttrackstudio.app/bible");
    }
}
