//! Parts and capabilities — `project.part.*`, `project.capability.*`.
//!
//! # A part is not a small project
//!
//! `project.part.unit`: a project's work divides into named parts — a
//! song, a scene, an episode — and a part "costs nothing: no directory,
//! no marker, no capabilities of its own". So a [`Part`] is two fields
//! in its project's own frontmatter, and there is deliberately nowhere
//! else to look for one. Dividing an album into ten songs creates ten
//! list entries, not ten files.
//!
//! That is the rule doing real work rather than being tidy. The archive
//! this model was drawn from has albums of fifteen tracks; making each
//! one project-shaped would mean fifteen pages, fifteen ids to keep in
//! step, and fifteen things to delete when someone renames the album.
//!
//! ## Why a part has an id from the moment it is named
//!
//! `project.part.promotion` says a part is promotable to a subproject
//! and back, and that "links, deliverables, setlist references and time
//! already attached to the part continue to resolve" across both moves.
//! Nothing can continue to resolve through a promotion unless it was
//! addressing something stable to begin with.
//!
//! An id assigned at promotion time would be an id that everything
//! referencing the part predates. So parts get one when they are
//! created, before anything can point at them, which is the only moment
//! at which it is free.
//!
//! # Capabilities are a set, and the set is closed
//!
//! `project.capability.multiple` wants a set rather than a type;
//! `project.capability.closed` wants that set drawn from a small closed
//! vocabulary. Both are about the same field, and the field that exists
//! today — `project_type`, a free string — is neither.
//!
//! ## Reading is tolerant, writing is not
//!
//! A vault is hand-editable and gets edited in Obsidian, so a closed
//! vocabulary cannot be a claim about what is on disk. It is a claim
//! about what *we* write:
//!
//! - `capabilities: [music-production]` parses as itself.
//! - `projectType: music` parses as `[MusicProduction]` — the
//!   compatibility path, since every project page in every vault
//!   predates this module.
//! - anything unrecognised parses as **no** capability and is reported
//!   through [`Capabilities::unrecognised`], never guessed at. A
//!   project whose type nobody can interpret is a project with no
//!   conventions, which is a legible state; inventing a capability for
//!   it is not.
//!
//! And serialising always emits `capabilities`, never `projectType`. A
//! page migrates the first time it is saved and a page nobody touches
//! keeps working, which is the most migration a vault we do not host
//! can be given.

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// t[impl project.part.unit] — a part is two fields in its project's
// frontmatter: "no directory, no marker, no capabilities of its own",
// and nowhere else to look for one
/// One named division of a project's work.
///
/// Deliberately two fields. Anything richer — a status, a lead, a date
/// — is the thing that makes a part want to be a project, and
/// `project.part.promotion` is how it becomes one rather than growing
/// into one field at a time.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct Part {
    /// Stable from creation — see the module docs on why not from
    /// promotion.
    pub id: Uuid,
    /// What a person calls it: "Overture", "Scene 4", "Episode 2".
    pub name: String,
}

/// A project's parts — the optional `parts:` list in frontmatter.
///
/// `Vec<Part>` newtype so architect can store it as a JSON column, the
/// same pattern as `Tags` and `StatesConfig`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Parts(pub Vec<Part>);

impl Parts {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// The part with this id.
    #[must_use]
    pub fn get(&self, id: Uuid) -> Option<&Part> {
        self.0.iter().find(|p| p.id == id)
    }

    /// Whether a part of this name already exists, case-insensitively.
    ///
    /// Case-insensitive because "Overture" and "overture" are one song
    /// with two spellings, and a project holding both is a project
    /// whose setlist references are ambiguous.
    #[must_use]
    pub fn has_name(&self, name: &str) -> bool {
        self.0.iter().any(|p| p.name.eq_ignore_ascii_case(name))
    }
}

// t[impl project.capability.closed] — a closed enum, small enough to
// enumerate, so adding one is a design act rather than a string
// t[impl project.part.listing] — the type that makes one list possible:
// a piece says what it is called and, only if asked, whether it has a page
/// One piece of a project's work, whichever side of the line it is on.
///
/// `project.part.listing`: a caller asking what an album consists of
/// gets ten songs, and learns which have pages only if it asks. The
/// alternative — two surfaces to query and merge — would make every
/// caller that starts from the project need to know exactly the thing
/// `project.part.promotion` says it should not.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct Piece {
    /// The same id on both sides. A promotion does not mint one and a
    /// demotion does not retire one — see [`Part`].
    pub id: Uuid,
    pub name: String,
    /// Whether this piece has a page of its own.
    ///
    /// Present so a surface *can* ask, not so it must: a track listing
    /// ignores it, and a "promote" button reads it.
    pub promoted: bool,
}

/// What a project does, and therefore what conventions apply to it.
///
/// Closed and small on purpose (`project.capability.closed`): each
/// variant carries a whole convention set — which tool directories are
/// recognised, which facets exist, which surfaces appear — so adding one
/// is a design act rather than a string.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Facet, Serialize, Deserialize,
)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum Capability {
    /// Sessions, stems, mixes. Reaper and Pro Tools layouts.
    MusicProduction,
    /// Footage, proxies, cuts, renders. Resolve and Premiere layouts.
    VideoProduction,
}

impl Capability {
    /// The wire and frontmatter spelling.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::MusicProduction => "music-production",
            Self::VideoProduction => "video-production",
        }
    }

    /// Parse a capability name. `None` for anything outside the
    /// vocabulary — the caller reports it rather than guessing.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "music-production" | "music_production" => Some(Self::MusicProduction),
            "video-production" | "video_production" => Some(Self::VideoProduction),
            _ => None,
        }
    }

    /// Interpret a legacy `projectType` value.
    ///
    /// Separate from [`Self::parse`] because these are not capability
    /// names — they are the free-string type field this replaces, and
    /// keeping the two readings apart is what stops `projectType`'s
    /// vocabulary quietly becoming the capability vocabulary.
    #[must_use]
    pub fn from_project_type(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "music" | "audio" | "album" | "song" | "music-production" => {
                Some(Self::MusicProduction)
            }
            "video" | "film" | "documentary" | "video-production" => Some(Self::VideoProduction),
            // `general` and everything else: a project with no declared
            // conventions, which is a legible state and the honest
            // reading of a field that meant nothing in particular.
            _ => None,
        }
    }

    /// Every capability there is. Small enough to enumerate, which is
    /// the point of the vocabulary being closed.
    #[must_use]
    pub fn all() -> &'static [Self] {
        &[Self::MusicProduction, Self::VideoProduction]
    }
}

impl std::fmt::Display for Capability {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

// t[impl project.capability.multiple] — a set, not a type: `held` is a
// collection and every reader asks `has()` rather than comparing one value
/// A project's capabilities — the `capabilities:` list in frontmatter.
///
/// Holds what it recognised *and* what it did not, because
/// "unrecognised" is a thing a surface should be able to say. Dropping
/// the unknown values silently would make a typo indistinguishable from
/// an intentionally plain project.
/// On disk this is a **list of names**, not this struct:
///
/// ```yaml
/// capabilities: [music-production, video-production]
/// ```
///
/// The struct is the parsed form, and it carries a second field
/// (`unrecognised`) that has no business in frontmatter — it is what
/// *this* reader made of what was written, which the next reader must
/// work out for itself rather than inherit. So the serde impls below are
/// hand-written: a sequence out, a sequence in, and the sorting happens
/// on the way in.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct Capabilities {
    /// In the vocabulary, deduplicated, in declaration order.
    pub held: Vec<Capability>,
    /// Names that are not in the vocabulary, as written. Reported, not
    /// guessed at, and never written back — see the type's docs.
    pub unrecognised: Vec<String>,
}

impl Capabilities {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty() && self.unrecognised.is_empty()
    }

    #[must_use]
    pub fn has(&self, capability: Capability) -> bool {
        self.held.contains(&capability)
    }

    /// Build from written names, sorting the recognised from the not.
    ///
    /// Deduplicates while keeping declaration order: a project that
    /// lists a capability twice holds it once, and the order a human
    /// wrote is the order they read back.
    pub fn from_names<I, S>(names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut out = Self::default();
        for name in names {
            let name = name.as_ref();
            match Capability::parse(name) {
                Some(c) if !out.held.contains(&c) => out.held.push(c),
                Some(_) => {}
                None if name.trim().is_empty() => {}
                None => out.unrecognised.push(name.trim().to_owned()),
            }
        }
        out
    }

    /// The single legacy `projectType`, read as a capability set.
    #[must_use]
    pub fn from_project_type(project_type: &str) -> Self {
        Capability::from_project_type(project_type).map_or_else(Self::default, |c| Self {
            held: vec![c],
            unrecognised: Vec::new(),
        })
    }

    /// What to write: recognised names only.
    ///
    /// Unrecognised values are not echoed back. Writing them would make
    /// this the field that accumulates every typo any editor ever made,
    /// which is the opposite of a closed vocabulary.
    #[must_use]
    pub fn to_names(&self) -> Vec<String> {
        self.held.iter().map(|c| c.as_str().to_owned()).collect()
    }
}

impl Serialize for Capabilities {
    /// Recognised names, as a plain sequence. See the type's docs on why
    /// `unrecognised` does not go out.
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let names = self.to_names();
        let mut seq = s.serialize_seq(Some(names.len()))?;
        for name in &names {
            seq.serialize_element(name)?;
        }
        seq.end()
    }
}

impl<'de> Deserialize<'de> for Capabilities {
    /// A sequence of names, or a single name — `capabilities:
    /// music-production` is what a person writes when there is one.
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            One(String),
            Many(Vec<String>),
        }
        Ok(match Written::deserialize(d)? {
            Written::One(one) => Self::from_names([one]),
            Written::Many(many) => Self::from_names(many),
        })
    }
}

// ── Deliverables ─────────────────────────────────────────────────────

/// What a deliverable is made of.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Medium {
    Audio,
    Video,
    Image,
    Document,
}

/// Who a deliverable is for.
///
/// The ordering is deliberate and load-bearing: `Internal < Client <
/// Public`, so "reachable by at most this audience" is a comparison
/// rather than a match somebody has to keep exhaustive.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Facet, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum Audience {
    /// The org's own. Never reachable from a client view.
    Internal,
    /// The people who commissioned the work.
    Client,
    /// Anyone.
    Public,
}

/// How much of the project one deliverable covers.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
#[repr(u8)]
pub enum Scope {
    /// One, for the project as a whole — the album master.
    WholeProject,
    /// One per piece, and it stays in step as pieces come and go.
    PerPart,
    /// A chosen extract. Does not expand on its own: an excerpt is
    /// picked rather than derived, so it exists once something is bound
    /// to it.
    Excerpt,
}

// t[impl project.deliverable.kind] — named, with a medium, a scope and
// an audience, and *declared*: there is no path here for anything to be
// discovered by looking at which renders seem final
/// A declaration of output the project owes someone.
///
/// `project.deliverable.scope`: five declarations, not twenty-one files
/// named individually. A `PerPart` audio deliverable of a ten-song album
/// is **one** of these.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct Deliverable {
    pub id: Uuid,
    /// What it is called: "Album master", "Per-song video".
    pub name: String,
    pub medium: Medium,
    pub scope: Scope,
    pub audience: Audience,
}

/// A project's deliverable declarations.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct Deliverables(pub Vec<Deliverable>);

impl Deliverables {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.0.len()
    }

    #[must_use]
    pub fn get(&self, id: Uuid) -> Option<&Deliverable> {
        self.0.iter().find(|d| d.id == id)
    }

    #[must_use]
    pub fn has_name(&self, name: &str) -> bool {
        self.0.iter().any(|d| d.name.eq_ignore_ascii_case(name))
    }
}

// t[impl project.deliverable.binding] — an item exists as soon as it is
// declared, with nothing attached. Declared-and-unbound is the state a
// project is in at the start of a job, and the client view shows it as
// outstanding rather than hiding it
/// One concrete thing a declaration resolves to.
///
/// The expansion of a declaration against the project's pieces. A
/// `PerPart` audio declaration over ten songs is ten of these, and
/// eleven the moment an eleventh song is named — which is what
/// `project.deliverable.scope` means by "stay in step".
///
/// Derived on every read rather than stored. Storing it would make
/// "stays in step" a job somebody has to remember to run, and the
/// failure would be an album quietly owing ten deliverables after
/// growing an eleventh song.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct DeliverableItem {
    /// The declaration this came from.
    pub deliverable: Uuid,
    pub name: String,
    pub medium: Medium,
    pub audience: Audience,
    /// The piece it covers, for a per-part item. `None` for a
    /// whole-project one.
    pub part: Option<Uuid>,
    /// What to call this item — the declaration's name for a
    /// whole-project item, the piece's for a per-part one.
    pub title: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_vocabulary_round_trips_through_its_own_spelling() {
        for c in Capability::all() {
            assert_eq!(Capability::parse(c.as_str()), Some(*c));
        }
    }

    /// An unrecognised name is kept as text, not turned into a
    /// capability and not dropped.
    #[test]
    fn an_unrecognised_capability_is_reported_rather_than_guessed() {
        let caps = Capabilities::from_names(["music-production", "wedding-videography"]);
        assert_eq!(caps.held, vec![Capability::MusicProduction]);
        assert_eq!(caps.unrecognised, vec!["wedding-videography".to_owned()]);
        // And it is never written back.
        assert_eq!(caps.to_names(), vec!["music-production".to_owned()]);
    }

    #[test]
    fn a_capability_listed_twice_is_held_once() {
        let caps = Capabilities::from_names(["music-production", "music_production"]);
        assert_eq!(caps.held, vec![Capability::MusicProduction]);
    }

    /// The compatibility path: the free string every existing page
    /// carries.
    #[test]
    fn a_legacy_project_type_reads_as_a_capability() {
        assert_eq!(
            Capabilities::from_project_type("video").held,
            vec![Capability::VideoProduction]
        );
        assert_eq!(
            Capabilities::from_project_type("album").held,
            vec![Capability::MusicProduction]
        );
        // `general` meant nothing in particular, and still does.
        assert!(Capabilities::from_project_type("general").is_empty());
        assert!(Capabilities::from_project_type("").is_empty());
    }

    #[test]
    fn parts_are_named_case_insensitively() {
        let parts = Parts(vec![Part {
            id: Uuid::nil(),
            name: "Overture".into(),
        }]);
        assert!(parts.has_name("overture"));
        assert!(!parts.has_name("Daybreak"));
    }
}
