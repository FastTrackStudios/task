//! [`Collection`] — a generic, ordered list of [`NodeRef`] items.
//!
//! A collection is an ordered list of references to nodes that already
//! live elsewhere in the graph (songs, notes, videos, …), labelled with
//! a [`CollectionKind`] its *caller* chooses. Items are addressed by
//! their [`NodeRef`] and ordered by a fractional-index `rank` (a
//! lexorank key) so inserts and reorders stay local — no re-indexing of
//! siblings.
//!
//! This is the wasm-clean model half; `rank` is a plain `String` and the
//! lexorank generator lives in the native `collection` crate.

use facet::Facet;
use serde::{Deserialize, Deserializer, Serialize, de};

pub use links_proto::{NodeKind, NodeRef};

/// What kind of collection this is — a label the **caller** defines.
///
/// # A primitive does not enumerate its consumers
///
/// This type used to be an enum: `Library`, `Setlist`, `Show`,
/// `Playlist`, and an `Other(String)` escape hatch. Those four words are
/// not things a *store* means. They are things a *performance
/// application* means, and the moment they lived here, every application
/// built on this crate had to wait for a release of the thing it is
/// built on before it could say a word about its own domain. A sixth app
/// could not name a rehearsal pool without a pull request against Task;
/// Session could not decide that a show contains rehearsals as well as
/// setlists without changing the layer underneath it. That is exactly
/// backwards, and it is the reasoning ADR 0004 records so that the next
/// person does not helpfully add `CollectionKind::Rehearsal` back.
///
/// What Task actually contributes to a collection is what remains: an
/// ordered list of resolvable node references, a lexorank so that
/// inserts and reorders stay local, membership, and a kind you can
/// filter on. It guarantees ordering, membership and reference
/// resolution. It neither knows nor cares that `setlist` is a setlist.
/// The words `library`, `setlist`, `show` and `playlist` still exist —
/// they are now strings that Session, Keyflow and Ignition define,
/// document and validate, in their own repositories, on their own
/// release cadence.
///
/// # Why a newtype rather than a bare `String`
///
/// A bare `String` would have been one fewer type, and it would have
/// been wrong, for one reason that is not aesthetic: **a store that
/// accepts `"Setlist"` and `"setlist"` as two different kinds is a store
/// that will silently split someone's data.** Nobody would ever see an
/// error. A CLI user types `--kind Setlist`, the web UI writes
/// `setlist`, and six months later half the sets are missing from a list
/// that filters on one spelling. There is no repair for that which does
/// not involve guessing what a person meant.
///
/// So there is exactly one place a kind string can be made
/// ([`CollectionKind::new`]) and it normalises: trimmed of surrounding
/// whitespace, ASCII-lowercased. Two spellings of the same word are the
/// same kind, by construction, and no caller has to remember to do it.
/// Interior structure is left alone — `rehearsal pool` and
/// `rehearsal-pool` stay distinct, because those really are two
/// different strings and picking one for the caller would be this type
/// having an opinion about a vocabulary it just finished disclaiming.
///
/// Normalising is deliberately lossy for the one case that used to
/// preserve case: the old `Other("Rehearsal Pool")` kept the caller's
/// capitals. Display casing is a presentation concern and belongs to the
/// application that owns the word; identity is what a store owes, and
/// identity has to be stable under a shift key.
///
/// # Does an empty kind mean anything?
///
/// No, and the store refuses to create one — see `collection::Store`,
/// which answers `BadRequest`. But the *type* stays total: an empty kind
/// deserialises rather than failing, because the JSONL loader drops
/// lines it cannot parse, and losing a whole collection and its ordering
/// over a blank label would be a far worse outcome than carrying one
/// unlabelled row until somebody fixes it. Reject on the way in, tolerate
/// on the way out.
///
/// # On-disk compatibility
///
/// The [`Deserialize`] impl below reads the two legacy JSON shapes the
/// enum wrote as well as the plain string this type writes. See it for
/// why that was necessary and what ADR 0004 got wrong about it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Facet)]
#[serde(transparent)]
#[repr(transparent)]
pub struct CollectionKind(String);

impl CollectionKind {
    /// A kind from any caller-supplied label, normalised: surrounding
    /// whitespace trimmed, ASCII-lowercased.
    ///
    /// This is the only constructor, which is the point — see the type
    /// docs on why normalisation cannot be left to callers.
    #[must_use]
    pub fn new(label: impl AsRef<str>) -> Self {
        Self(label.as_ref().trim().to_ascii_lowercase())
    }

    /// The stable string form, for storage, filtering and display. What
    /// is stored is exactly what [`Self::new`] normalised.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// True for the label no caller should have supplied. The store
    /// rejects it on create; readers use this to notice a legacy or
    /// hand-edited row rather than to panic on one.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Consume into the owned label.
    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl From<&str> for CollectionKind {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for CollectionKind {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl std::fmt::Display for CollectionKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Reads the string this type writes **and** both shapes the old enum
/// wrote.
///
/// ADR 0004 asserted that this migration was free, on the grounds that
/// "existing rows carry their variant's `as_str()` value, which is
/// already what is persisted". That is not true, and a look at a real
/// `collections.jsonl` says so in one line:
///
/// ```jsonl
/// {"id":"24aa…","org":"acme-audio","title":"Chart Library","kind":"Library",…}
/// ```
///
/// `Library`, not `library`. `as_str()` was the *display and filtering*
/// spelling; serde never called it. An externally-tagged unit variant
/// serialises as its **Rust variant name**, so the four named kinds were
/// on disk capitalised, and `Other(label)` was on disk not a string at
/// all but the map `{"Other":"<label>"}`. Deserialising either of those
/// into a plain `String` fails: the capitalised form would round-trip as
/// a *different kind* from anything the new code writes, and the map
/// form would not parse — and the store's loader drops unparseable lines
/// silently, so the visible symptom would have been collections quietly
/// vanishing.
///
/// Hence this impl, and hence `legacy_rows_load_with_their_kinds_folded`
/// in `collection::store`, which pins it against bytes copied out of a
/// real planted vault rather than against bytes this code wrote.
///
/// The folding falls out of normalisation for free: `"Library"` is a
/// string, [`CollectionKind::new`] lowercases it, and it *becomes*
/// `library` — the same value the new writer produces for the same
/// collection. Normalisation is the migration. The only thing that
/// needed writing by hand is the `{"Other": …}` map arm.
impl<'de> Deserialize<'de> for CollectionKind {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;

        impl<'de> de::Visitor<'de> for V {
            type Value = CollectionKind;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a collection kind label, or a legacy `{\"Other\": label}`")
            }

            fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
                Ok(CollectionKind::new(s))
            }

            /// The legacy `Other(String)` newtype variant, externally
            /// tagged: a one-entry map whose key is the variant name.
            /// Any other key is a row this code has no reading for, and
            /// saying so beats inventing a kind out of it.
            fn visit_map<M: de::MapAccess<'de>>(self, mut m: M) -> Result<Self::Value, M::Error> {
                let Some((tag, label)) = m.next_entry::<String, String>()? else {
                    return Err(de::Error::custom("empty collection-kind map"));
                };
                if tag != "Other" {
                    return Err(de::Error::custom(format!(
                        "unknown legacy collection-kind variant `{tag}`"
                    )));
                }
                Ok(CollectionKind::new(label))
            }
        }

        d.deserialize_any(V)
    }
}

/// One entry in a [`Collection`]: a reference to a node plus its sort key.
///
/// `rank` is a lexorank fractional-index string; items in a collection are
/// kept sorted by it. Two items never share a rank.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct CollectionItem {
    /// The node this entry points at (a song, note, video, …).
    pub node: NodeRef,
    /// Lexorank sort key. Items are ordered ascending by this string.
    pub rank: String,
}

impl CollectionItem {
    #[must_use]
    pub fn new(node: NodeRef, rank: impl Into<String>) -> Self {
        Self {
            node,
            rank: rank.into(),
        }
    }
}

/// An ordered collection of node references.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Facet)]
pub struct Collection {
    /// Stable id (assigned by the backend on create if empty).
    pub id: String,
    /// Owning org.
    pub org: String,
    /// Human title.
    pub title: String,
    /// What kind of collection this is.
    pub kind: CollectionKind,
    /// The items, kept sorted ascending by [`CollectionItem::rank`].
    #[serde(default)]
    pub items: Vec<CollectionItem>,
}

impl Collection {
    /// A fresh, empty collection (no id yet — the backend assigns one).
    #[must_use]
    pub fn new(org: impl Into<String>, title: impl Into<String>, kind: CollectionKind) -> Self {
        Self {
            id: String::new(),
            org: org.into(),
            title: title.into(),
            kind,
            items: Vec::new(),
        }
    }

    /// Sort items ascending by rank (the canonical order). Called by the
    /// store after every mutation so readers see a sorted list.
    pub fn sort_items(&mut self) {
        self.items.sort_by(|a, b| a.rank.cmp(&b.rank));
    }
}
