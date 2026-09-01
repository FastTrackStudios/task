//! What a vault or wiki holds when it subscribes to something.
//!
//! A subscription is the live form of a reference: citing a source
//! takes a snapshot the citer then owns, while subscribing keeps a
//! channel open to a body that goes on being corrected
//! (`features/wiki/spec/wiki.md`).
//!
//! Two things can be subscribed to and a vault is not one of them
//! ([`SourceKind`], `wiki.boundary.no-subscribe`). The subscriber, by
//! contrast, *may* be a vault: that asymmetry is the whole distinction
//! between the two, so it is in the types rather than in a check
//! somebody has to remember to write.

use facet::Facet;
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
        matches!(self, Self::Wiki)
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
}

impl Subscriber {
    /// The key this subscriber is stored under.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::Vault => "vault".to_owned(),
            Self::Wiki(slug) => format!("wiki:{slug}"),
        }
    }

    /// Parse a stored key back.
    #[must_use]
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "vault" => Some(Self::Vault),
            other => other
                .strip_prefix("wiki:")
                .filter(|s| !s.is_empty())
                .map(|s| Self::Wiki(s.to_owned())),
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
    fn a_resource_is_not_editable_and_a_wiki_is() {
        assert!(SourceKind::Wiki.is_editable());
        assert!(!SourceKind::Resource.is_editable());
    }

    #[test]
    fn subscriber_keys_round_trip() {
        for s in [Subscriber::Vault, Subscriber::Wiki("cooking".into())] {
            assert_eq!(Subscriber::from_key(&s.key()), Some(s.clone()), "{s:?}");
        }
        assert_eq!(Subscriber::from_key("wiki:"), None);
        assert_eq!(Subscriber::from_key("nonsense"), None);
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
        };
        assert!(!s.is_active());
        assert_eq!(s.qualified(), "fasttrackstudio.app/bible");
    }
}
