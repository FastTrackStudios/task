//! Wasm-clean wire contract for the Resource Library.
//!
//! The transcript / annotation sidecars live on the server under
//! `<org>/resources/` (a native tier, not the vault), so the in-app
//! watch + reader views read them over this `#[architect::rpc]` service
//! rather than fetching files. Mirrors the `links-proto` shape.
//!
//! The write side is the **sermon sync**: a cron-driven CLI hands the
//! server a video's captions ([`SermonResource`]) and the server lays
//! the resource down (`<slug>.md` + transcript + annotation sidecars),
//! extracts the scripture references the preacher spoke, and mints the
//! `sermon:<slug>#t:<secs> → verse:<osis>` links that make the sermon
//! show up as a backlink in the scripture reader.
//!
//! The **chart lane** ([`ChartDoc`] and friends) is the second writer
//! on the same tier: Keyflow keeps a person's charts under
//! `<org>/resources/charts/` through these RPCs, and a *library* of
//! them is an ordinary `Collection` of kind `Library` over
//! `chart:<slug>` node references — ADR 0003, which builds nothing new
//! for libraries on purpose.
//!
//! The **asset lanes** that follow it are the same lane again, once per
//! kind ADR 0003's table gives a home to: [`PatchDoc`] and [`SampleDoc`]
//! for Signal (`resources/patches/`, `resources/samples/`) and
//! [`LightingDoc`] for Ignition (`resources/lighting/`). Four RPCs each,
//! one manifest each, and the identity is always the slug — because the
//! whole point of the decision is that a `patch:` or a `lighting:`
//! reference is addressable from a setlist without any of these apps
//! knowing the others exist.

use facet::Facet;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One transcript cue — spoken text and when it occurs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
pub struct TranscriptSegment {
    /// Start time in seconds.
    pub start: f32,
    /// Duration in seconds.
    #[serde(default)]
    pub dur: f32,
    pub text: String,
}

/// A resource's full transcript (matches the `<slug>.transcript.json`
/// sidecar shape, so the backend deserializes straight into it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct TranscriptDoc {
    #[serde(default)]
    pub slug: String,
    #[serde(default)]
    pub source: String,
    pub segments: Vec<TranscriptSegment>,
}

/// A sermon as the sync hands it to the server: the video, its
/// captions, and what the sync knows about the channel. The server
/// owns the slug (stable per video id), the file layout, and the
/// scripture references it extracts from the cues.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct SermonResource {
    /// Subfolder under the sermons root (`crossroads`). Kebab-case;
    /// the sync's name for the channel.
    pub folder: String,
    /// The named wiki the sermons belong to (`bible`). When set, the
    /// sermons root is `<org>/wikis/<wiki>/Resources/Sermons/` — the
    /// resources are pages of that wiki (its explorer, editor, bases and
    /// subscriptions see them). Empty: the org-wide
    /// `<org>/resources/sermons/` tier.
    #[serde(default)]
    #[facet(default)]
    pub wiki: String,
    /// YouTube video id (`YMypVgZXFIU`). The identity of the resource:
    /// the same id always maps to the same slug.
    pub video_id: String,
    /// Canonical watch URL (`https://youtu.be/<id>`).
    pub video_url: String,
    pub title: String,
    /// The channel / speaker — becomes `writers: [<channel>]`.
    pub channel: String,
    /// `tags:` frontmatter (`[sermons/crossroads]` — hierarchical, so
    /// a tag tree nests the channel under Sermons).
    pub tags: Vec<String>,
    /// `YYYY-MM-DD` upload date; empty when unknown.
    pub published: String,
    /// Video length; `0` when unknown.
    pub duration_secs: u64,
    /// `manual` (uploader captions) or `auto` (YouTube ASR).
    pub caption_kind: String,
    /// Caption track language as YouTube labels it (`en`, `en-orig`).
    pub language: String,
    /// The cues, in time order.
    pub segments: Vec<TranscriptSegment>,
}

/// What the server laid down for one sermon.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct SermonUpsert {
    pub slug: String,
    /// Resources-relative path of the manifest
    /// (`sermons/crossroads/<slug>.md`).
    pub rel_path: String,
    /// `true` when the manifest did not exist before this call.
    pub created: bool,
    /// `true` when an existing manifest body was preserved (only the
    /// sync-owned frontmatter fields were rewritten).
    pub body_kept: bool,
    /// OSIS references extracted from the captions, first-mention
    /// order, deduped (`1Pet.5.7`, `John.21.15-John.21.17`, `1Pet.5`).
    pub scripture: Vec<String>,
    /// Number of `sermon → verse` links now in the store for this slug.
    pub links: u32,
}

/// One synced sermon, as `list_sermons` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct SermonSummary {
    pub slug: String,
    pub title: String,
    pub folder: String,
    /// The named wiki holding this sermon; empty for the org-wide tier.
    #[serde(default)]
    #[facet(default)]
    pub wiki: String,
    pub channel: String,
    pub video_id: String,
    pub video_url: String,
    pub published: String,
    pub duration_secs: u64,
    pub tags: Vec<String>,
    pub scripture: Vec<String>,
    /// Manifest path: `sermons/crossroads/<slug>.md` under the org's
    /// resources tier, or `wikis/<wiki>/Resources/Sermons/<folder>/<slug>.md`
    /// for a wiki-hosted sermon. Either form resolves through
    /// [`ResourcesService::transcript`].
    pub rel_path: String,
    /// Transcript sidecar path, in the same form as `rel_path`.
    pub transcript_rel_path: String,
}

/// A Keyflow chart as Keyflow hands it to the server, and as the server
/// hands it back.
///
/// The identity is the `slug`: pass one to update a chart, leave it
/// empty and the server derives it from the title the same way sermon
/// slugs are derived (`resources::sermon::slugify`), suffixing a
/// collision so two charts titled the same never overwrite each other.
///
/// The `source` is the chart text, stored verbatim in
/// `<org>/resources/charts/<slug>.kf` — an outside editor sees a plain
/// chart file, not an encoding of one.
///
/// # One chart is one arrangement
///
/// A song is played more than one way — the original, a condensed live
/// cut, an acoustic reading a tone down — and each of those is a
/// *different chart of the same song*, not a revision of one chart.
/// Three fields carry that, and they are the reason a second chart of a
/// song is no longer merely a slug collision:
///
/// - [`ChartDoc::song`] says which song this arranges,
/// - [`ChartDoc::arrangement`] says which reading of it this is,
/// - [`ChartDoc::is_default`] says which one a caller that asked for
///   "the chart" should get.
///
/// All three are defaulted, so every chart written before they existed
/// still parses — as an unattached chart, which stays a valid and
/// ordinary state: Keyflow saves what a person typed before they have
/// said what song it is.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct ChartDoc {
    /// Chart slug — the `chart:<slug>` node id. Empty on a create:
    /// the server derives it from the title.
    #[serde(default)]
    #[facet(default)]
    pub slug: String,
    pub title: String,
    /// The Keyflow source, verbatim.
    #[serde(default)]
    #[facet(default)]
    pub source: String,
    /// Musical key as written (`A`, `Bb`, `f#m`); empty when unset.
    #[serde(default)]
    #[facet(default)]
    pub key: String,
    /// Notation dialect the source is written in (`keyflow`,
    /// `chordpro`, `nashville`); empty means Keyflow's own.
    #[serde(default)]
    #[facet(default)]
    pub notation: String,
    /// Section names in chart order (`verse-1`, `chorus`) — the
    /// anchors a `chart:<slug>#chorus` reference addresses. Supplied by
    /// the caller: the server does not parse Keyflow source (see
    /// [`ResourcesService::upsert_chart`]).
    #[serde(default)]
    #[facet(default)]
    pub sections: Vec<String>,
    /// The song this chart is an arrangement of, as a
    /// `links_proto::NodeRef` token: `song:doxology` locally, or
    /// `guest.example/song:hosanna` for another org's song (ADR 0003 —
    /// a qualified reference is an address, not a grant, and this field
    /// accepts one so that path stays open even though *writing* into
    /// another org still needs a membership row).
    ///
    /// **Empty is valid and ordinary.** Keyflow saves a chart before
    /// the person has said what song it is, and an unattached chart is
    /// independent: the default invariant below does not touch it.
    ///
    /// The server normalises what it is given rather than refusing it:
    /// a bare slug (`doxology`) is read as the local `song:doxology`,
    /// because ADR 0003 keeps reference parsing total. A token naming
    /// some *other* kind (`chart:doxology`) is a `BadRequest` — that is
    /// not a lenient reading of a song, it is a different thing.
    #[serde(default)]
    #[facet(default)]
    pub song: String,
    /// Which reading of the song this is, in a person's own words:
    /// `original`, `condensed live`, `acoustic in G`. Free text, and
    /// empty is fine — the first chart of a song usually needs no label
    /// because there is nothing yet to tell it apart from.
    ///
    /// It is not an identifier. It does feed the derived slug
    /// (`doxology-condensed-live`), so that a song's second chart is
    /// named for what it is rather than landing on `doxology-2`.
    #[serde(default)]
    #[facet(default)]
    pub arrangement: String,
    /// Whether this is the song's main chart — the one to open when
    /// somebody asks for "the chart" and names no arrangement.
    ///
    /// The server owns this flag rather than storing what it is told;
    /// see [`ResourcesService::upsert_chart`] for the invariant and why
    /// it is enforced there rather than left to callers.
    #[serde(default)]
    #[facet(default)]
    pub is_default: bool,
    /// When the caller last changed the chart (`RFC 3339`); empty when
    /// the caller does not track it. Caller-owned — the server stores
    /// what it is given and stamps nothing.
    #[serde(default)]
    #[facet(default)]
    pub updated_at: String,
}

/// What the server laid down for one chart.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct ChartUpsert {
    pub slug: String,
    /// Resources-relative path of the manifest (`charts/<slug>.md`).
    pub rel_path: String,
    /// `true` when the chart did not exist before this call.
    pub created: bool,
}

/// One chart, as `list_charts` reports it — everything but the source,
/// which `chart` fetches.
///
/// It carries [`ChartSummary::song`], [`ChartSummary::arrangement`] and
/// [`ChartSummary::is_default`] so that *one* call renders a song with
/// its arrangements: `list_charts(song)` returns the whole set, the
/// labels that tell them apart, and which one is the main one, with no
/// read per row. That is the call a Keyflow song screen makes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct ChartSummary {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    #[facet(default)]
    pub key: String,
    #[serde(default)]
    #[facet(default)]
    pub notation: String,
    #[serde(default)]
    #[facet(default)]
    pub sections: Vec<String>,
    /// The song this chart arranges, as a `song:<slug>` token (or a
    /// qualified `domain/song:<slug>`); empty for an unattached chart.
    #[serde(default)]
    #[facet(default)]
    pub song: String,
    /// The label that tells this arrangement from the song's others.
    #[serde(default)]
    #[facet(default)]
    pub arrangement: String,
    /// Whether this is the song's main chart. Exactly one chart of a
    /// song carries it; an unattached chart never does.
    #[serde(default)]
    #[facet(default)]
    pub is_default: bool,
    /// Manifest path (`charts/<slug>.md`) under the org's resources
    /// tier. The source sits beside it as `charts/<slug>.kf`.
    pub rel_path: String,
    #[serde(default)]
    #[facet(default)]
    pub updated_at: String,
}

/// Where an asset's *bytes* live — a path inside a File Root, not a
/// path inside `resources/`.
///
/// The split is deliberate, and it is the one [`project_proto`]'s
/// `Deliverable` already makes: the manifest under
/// `<org>/resources/<kind>/<slug>/` declares *what this is* — the
/// identity a `patch:` or `sample:` reference addresses, small enough
/// that a Resource subscription can carry every one of them across an
/// org boundary — and this reference says *where its content sits*,
/// which is the Files layer's business.
///
/// Content belongs there because `resources/` has none of what content
/// needs and Files has all of it: large-content versioning, selective
/// sync by facet with dehydrated stubs that hydrate on access,
/// renditions (Peaks waveforms, audio proxies) and flow-controlled
/// chunked streaming over the same transport. `files.sync.selective`
/// makes the case for these lanes exactly — "Atomic facets bring their
/// dependencies — a session arrives with the media it references,
/// because one that streams in on first play will glitch" — which is a
/// patch and the samples it plays. A multi-gigabyte sample library in
/// `resources/` would make every subscriber pull all of it to read a
/// list of names.
///
/// Empty is the ordinary state of a freshly declared asset: the
/// manifest exists, no bytes are bound yet. Nothing in these lanes
/// transfers a byte — they carry the reference and the Files lane does
/// the work.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct ContentRef {
    /// File Root the content lives in, as the Files lane ids it. Empty
    /// means nothing is bound.
    #[serde(default)]
    #[facet(default)]
    pub root_id: String,
    /// Root-relative path of the content (`Samples/Kicks/808.wav`).
    #[serde(default)]
    #[facet(default)]
    pub path: String,
}

impl ContentRef {
    /// Whether any bytes are bound. A half-filled reference — a root
    /// with no path, or the reverse — names nothing, so it is unbound.
    #[must_use]
    pub fn is_bound(&self) -> bool {
        !self.root_id.is_empty() && !self.path.is_empty()
    }
}

/// A Signal patch as Signal hands it to the server, and as the server
/// hands it back.
///
/// The identity is the `slug` (`patch:<slug>` in the link graph and in
/// a `Library` collection); leave it empty on a create and the server
/// derives it from the title, suffixing a collision so two patches
/// titled the same never overwrite each other.
///
/// The `body` is the patch definition text, stored verbatim in
/// `<org>/resources/patches/<slug>/patch.json`. A *directory* rather
/// than a flat file because a patch grows sidecars — impulse responses,
/// captures, a photo of the pedalboard — and ADR 0003's `locate()`
/// already looks for the directory.
///
/// Anything large the patch plays lives in a File Root and is named by
/// [`PatchDoc::content`]; see [`ContentRef`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct PatchDoc {
    /// Patch slug — the `patch:<slug>` node id. Empty on a create.
    #[serde(default)]
    #[facet(default)]
    pub slug: String,
    pub title: String,
    /// The rig the patch is for (`helix`, `kemper`, `serum`); empty
    /// when the app does not say.
    #[serde(default)]
    #[facet(default)]
    pub rig: String,
    /// Free tags (`lead`, `ambient`) — how a person finds a patch when
    /// they have four hundred of them.
    #[serde(default)]
    #[facet(default)]
    pub tags: Vec<String>,
    /// The patch definition, verbatim.
    #[serde(default)]
    #[facet(default)]
    pub body: String,
    /// Where the patch's bytes live, when any are bound.
    #[serde(default)]
    #[facet(default)]
    pub content: ContentRef,
    /// When the caller last changed the patch (`RFC 3339`); empty when
    /// the caller does not track it. Caller-owned — the server stores
    /// what it is given and stamps nothing.
    #[serde(default)]
    #[facet(default)]
    pub updated_at: String,
}

/// What the server laid down for one patch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct PatchUpsert {
    pub slug: String,
    /// Resources-relative path of the manifest
    /// (`patches/<slug>/patch.md`).
    pub rel_path: String,
    /// `true` when the patch did not exist before this call.
    pub created: bool,
}

/// One patch, as `list_patches` reports it — everything but the body,
/// which `patch` fetches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct PatchSummary {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    #[facet(default)]
    pub rig: String,
    #[serde(default)]
    #[facet(default)]
    pub tags: Vec<String>,
    /// Manifest path (`patches/<slug>/patch.md`) under the org's
    /// resources tier. The definition sits beside it as `patch.json`.
    pub rel_path: String,
    #[serde(default)]
    #[facet(default)]
    pub content: ContentRef,
    #[serde(default)]
    #[facet(default)]
    pub updated_at: String,
}

/// A Signal sample as Signal hands it to the server.
///
/// **The audio is not here.** This lane carries the sample's identity
/// and its metadata — what it is, how long, at what rate — and
/// [`SampleDoc::content`] names the File Root path the actual audio
/// sits at. Bytes go through the Files lane for the reasons
/// [`ContentRef`] states: versioning, selective sync, Peaks renditions
/// and chunked streaming, none of which the resources tier has. A
/// sample *library* is then an ordinary `Collection` of kind `Library`
/// over `sample:<slug>` references, and it stays cheap to subscribe to
/// because subscribing to it moves manifests, not gigabytes.
///
/// The manifest is `<org>/resources/samples/<slug>/sample.md`, with the
/// `body` beside it as `sample.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct SampleDoc {
    /// Sample slug — the `sample:<slug>` node id. Empty on a create.
    #[serde(default)]
    #[facet(default)]
    pub slug: String,
    pub title: String,
    /// Free tags (`kick`, `vintage`, `808`).
    #[serde(default)]
    #[facet(default)]
    pub tags: Vec<String>,
    /// Length in seconds; `0` when unknown. What a
    /// `sample:<slug>#t:0-2:400` region anchor is measured against.
    #[serde(default)]
    #[facet(default)]
    pub duration_secs: u64,
    /// Sample rate in Hz (`48000`); `0` when unknown.
    #[serde(default)]
    #[facet(default)]
    pub sample_rate: u32,
    /// Metadata notes about the sample — how it was captured, what it
    /// is for. **Not the audio**: see [`SampleDoc::content`].
    #[serde(default)]
    #[facet(default)]
    pub body: String,
    /// Where the audio lives, in a File Root. Empty until bytes are
    /// bound, which is the ordinary state of a declared sample.
    #[serde(default)]
    #[facet(default)]
    pub content: ContentRef,
    /// When the caller last changed the sample (`RFC 3339`).
    /// Caller-owned.
    #[serde(default)]
    #[facet(default)]
    pub updated_at: String,
}

/// What the server laid down for one sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct SampleUpsert {
    pub slug: String,
    /// Resources-relative path of the manifest
    /// (`samples/<slug>/sample.md`).
    pub rel_path: String,
    /// `true` when the sample did not exist before this call.
    pub created: bool,
}

/// One sample, as `list_samples` reports it — everything but the body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct SampleSummary {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    #[facet(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    #[facet(default)]
    pub duration_secs: u64,
    #[serde(default)]
    #[facet(default)]
    pub sample_rate: u32,
    /// Manifest path (`samples/<slug>/sample.md`).
    pub rel_path: String,
    /// Where the audio is, when it is bound. A summary carries it so a
    /// library list can say which entries are playable without a read
    /// per row.
    #[serde(default)]
    #[facet(default)]
    pub content: ContentRef,
    #[serde(default)]
    #[facet(default)]
    pub updated_at: String,
}

/// An Ignition lighting document — the cues for a song, a setlist or a
/// whole show.
///
/// The `scope` says which of those it is, and is one of `song`,
/// `setlist` or `show`; anything else is a `BadRequest`, because the
/// scope is what tells a caller whether `lighting:sunday-set` belongs
/// beside one song or over an evening.
///
/// The `cues` are the cue labels an anchor may address:
/// `lighting:sunday-set#cue:12` resolves only if `12` is listed here,
/// the same way a chart's sections are declared rather than parsed.
///
/// The manifest is `<org>/resources/lighting/<slug>/show.md`, with the
/// `body` beside it as `show.json`. Any rendered media the show needs
/// lives in a File Root, named by [`LightingDoc::content`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct LightingDoc {
    /// Lighting slug — the `lighting:<slug>` node id. Empty on a
    /// create.
    #[serde(default)]
    #[facet(default)]
    pub slug: String,
    pub title: String,
    /// `song`, `setlist` or `show`. Validated on upsert.
    #[serde(default)]
    #[facet(default)]
    pub scope: String,
    /// Cue labels in show order — the anchors a
    /// `lighting:<slug>#cue:<label>` reference addresses.
    #[serde(default)]
    #[facet(default)]
    pub cues: Vec<String>,
    /// The lighting definition, verbatim.
    #[serde(default)]
    #[facet(default)]
    pub body: String,
    /// Where the show's bytes live, when any are bound.
    #[serde(default)]
    #[facet(default)]
    pub content: ContentRef,
    /// When the caller last changed the show (`RFC 3339`).
    /// Caller-owned.
    #[serde(default)]
    #[facet(default)]
    pub updated_at: String,
}

/// What the server laid down for one lighting document.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct LightingUpsert {
    pub slug: String,
    /// Resources-relative path of the manifest
    /// (`lighting/<slug>/show.md`).
    pub rel_path: String,
    /// `true` when it did not exist before this call.
    pub created: bool,
}

/// One lighting document, as `list_lighting` reports it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Default)]
pub struct LightingSummary {
    pub slug: String,
    pub title: String,
    #[serde(default)]
    #[facet(default)]
    pub scope: String,
    #[serde(default)]
    #[facet(default)]
    pub cues: Vec<String>,
    /// Manifest path (`lighting/<slug>/show.md`).
    pub rel_path: String,
    #[serde(default)]
    #[facet(default)]
    pub content: ContentRef,
    #[serde(default)]
    #[facet(default)]
    pub updated_at: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet, Error)]
#[repr(u8)]
pub enum ResourcesError {
    #[error("not found: {0}")]
    NotFound(String),
    #[error("io: {0}")]
    Io(String),
    #[error("bad request: {0}")]
    BadRequest(String),
}

#[architect::rpc]
pub trait ResourcesService {
    /// Read a transcript sidecar at `<org>/resources/<rel_path>`
    /// (e.g. `sermons/god-restores-broken-people.transcript.json`).
    /// A path whose file is not there is retried one directory down
    /// (`sermons/*/<name>`), so a sermon synced into a channel folder
    /// still resolves from its `sermon:<slug>` node.
    /// Path traversal (`..`) is rejected.
    fn transcript(&self, rel_path: &str) -> Result<TranscriptDoc, ResourcesError>;

    /// Create or refresh a sermon resource from its captions. Writes
    /// `<slug>.md`, `<slug>.transcript.json` and — only when absent —
    /// an empty `<slug>.annotations.json`. An existing manifest keeps
    /// its hand-edited body; only the sync-owned frontmatter is
    /// rewritten. Replaces this sermon's `sermon-sync` links.
    fn upsert_sermon(&self, sermon: SermonResource) -> Result<SermonUpsert, ResourcesError>;

    /// Every sermon manifest under `resources/sermons/**`, by slug.
    fn list_sermons(&self) -> Result<Vec<SermonSummary>, ResourcesError>;

    /// One sermon by slug.
    fn sermon(&self, slug: &str) -> Result<SermonSummary, ResourcesError>;

    /// Move every file of `resources/sermons/<folder>/` into
    /// `wikis/<wiki>/Resources/Sermons/<folder>/`, so sermons synced
    /// into the org-wide tier become pages of that wiki. Slugs, links
    /// and sidecars are untouched (links are keyed by slug). Returns
    /// how many manifests moved.
    fn relocate_sermons(&self, folder: &str, wiki: &str) -> Result<u32, ResourcesError>;

    /// Create or replace a chart under `<org>/resources/charts/`:
    /// `<slug>.kf` holds [`ChartDoc::source`] verbatim and `<slug>.md`
    /// is the `type: resource`, `resource_kind: chart` manifest. An
    /// existing manifest keeps its hand-edited body — only the
    /// chart-owned frontmatter is rewritten, exactly as a sermon
    /// re-sync keeps its body.
    ///
    /// `sections` is taken from the caller, not parsed: the Keyflow
    /// parser is a UI-side git dependency, and the resources tier is not
    /// the place to pull it into the server.
    ///
    /// [`ChartDoc::song`] is normalised to a canonical `song:<slug>`
    /// token — a bare slug is read as local, a token of another kind is
    /// a `BadRequest`, and empty stays empty.
    ///
    /// An empty title is a `BadRequest`.
    ///
    /// # The default invariant, and why the server owns it
    ///
    /// **At most one default chart per song, and never zero while the
    /// song has a chart at all.** The server enforces it inside this
    /// call; [`ChartDoc::is_default`] as the caller passes it is a
    /// *request*, not a stored value:
    ///
    /// - Writing a chart with `is_default: true` clears the flag on
    ///   that song's other charts, in the same operation.
    /// - The **first** chart saved for a song becomes the default
    ///   whatever the caller passed, because a song with one chart and
    ///   no main one is a state nothing can render sensibly.
    /// - `is_default: false` is **no opinion**, not "demote me": a
    ///   chart that is already its song's default stays it. Otherwise
    ///   every save of an edited source by a client that does not track
    ///   the flag would hand the default to some other chart. The way
    ///   to move a default is to ask for it on the chart that should
    ///   have it.
    /// - Re-attaching a chart to a *different* song drops the flag: it
    ///   was the old song's main chart and has no claim on the new
    ///   song's — where, being that song's first chart, it may well
    ///   become the default anyway.
    /// - A chart with an empty `song` is independent: it is never a
    ///   default, and it never clears anybody else's flag.
    ///
    /// It lives here rather than in each client because it is a
    /// statement about a *set* of charts, and a client only ever holds
    /// the one it is saving. Two Keyflow tabs both ticking "make this
    /// the main chart" would otherwise leave a song with two defaults
    /// and no way to tell which write was last; enforced here, the
    /// second write clears the first, and the song has exactly one
    /// default either way round.
    fn upsert_chart(&self, chart: ChartDoc) -> Result<ChartUpsert, ResourcesError>;

    /// One chart by slug, source included.
    fn chart(&self, slug: &str) -> Result<ChartDoc, ResourcesError>;

    /// Every chart under `resources/charts/`, by slug.
    ///
    /// `song` filters to one song's arrangements: pass a `song:<slug>`
    /// token (a bare slug is accepted and read as local, as everywhere
    /// in this lane), or the empty string for every chart the org
    /// holds. The result carries each chart's arrangement label and
    /// default flag, so a song and its arrangements render from this
    /// one call.
    fn list_charts(&self, song: &str) -> Result<Vec<ChartSummary>, ResourcesError>;

    /// Delete a chart's manifest and its `.kf` source. `false` when
    /// there was nothing there. A `chart:<slug>` reference held by a
    /// collection is left alone: a dangling reference is a legible
    /// state, not an error (ADR 0003).
    ///
    /// Deleting a song's **default** promotes another of that song's
    /// charts, so the invariant `upsert_chart` states survives a
    /// delete: the promoted one is the *oldest remaining* — the
    /// earliest `updated_at`, ties broken by slug so the choice is
    /// deterministic on charts that track no timestamp. Oldest rather
    /// than newest because the oldest chart of a song is, in practice,
    /// the one the arrangements were derived from: deleting a
    /// condensed live cut that had been made the main one should fall
    /// back to the original, not to whichever alternate was edited
    /// most recently.
    fn delete_chart(&self, slug: &str) -> Result<bool, ResourcesError>;

    /// Create or replace a patch under
    /// `<org>/resources/patches/<slug>/`: `patch.json` holds
    /// [`PatchDoc::body`] verbatim and `patch.md` is the
    /// `type: resource`, `resource_kind: patch` manifest. An existing
    /// manifest keeps its hand-edited body — only the patch-owned
    /// frontmatter is rewritten, exactly as a chart re-save does.
    ///
    /// [`PatchDoc::content`] is stored as written and never followed:
    /// binding bytes is the Files lane's job, and this lane only
    /// records where they went.
    ///
    /// An empty title, or one that kebabs to nothing, is a
    /// `BadRequest`.
    fn upsert_patch(&self, patch: PatchDoc) -> Result<PatchUpsert, ResourcesError>;

    /// One patch by slug, body included.
    fn patch(&self, slug: &str) -> Result<PatchDoc, ResourcesError>;

    /// Every patch under `resources/patches/`, by slug.
    fn list_patches(&self) -> Result<Vec<PatchSummary>, ResourcesError>;

    /// Delete a patch's directory. `false` when there was nothing
    /// there. A `patch:<slug>` reference held by a collection is left
    /// alone: a dangling reference is a legible state, not an error
    /// (ADR 0003). Content in a File Root is likewise untouched — this
    /// lane never owned it.
    fn delete_patch(&self, slug: &str) -> Result<bool, ResourcesError>;

    /// Create or replace a sample under
    /// `<org>/resources/samples/<slug>/`: `sample.json` holds
    /// [`SampleDoc::body`] verbatim and `sample.md` is the
    /// `type: resource`, `resource_kind: sample` manifest.
    ///
    /// **No audio moves through here.** The bytes belong in a File
    /// Root, and [`SampleDoc::content`] is how this manifest names
    /// them; see [`ContentRef`] for why.
    ///
    /// An empty title, or one that kebabs to nothing, is a
    /// `BadRequest`.
    fn upsert_sample(&self, sample: SampleDoc) -> Result<SampleUpsert, ResourcesError>;

    /// One sample by slug, body included. Still no audio: the caller
    /// takes [`SampleDoc::content`] to the Files lane for that.
    fn sample(&self, slug: &str) -> Result<SampleDoc, ResourcesError>;

    /// Every sample under `resources/samples/`, by slug.
    fn list_samples(&self) -> Result<Vec<SampleSummary>, ResourcesError>;

    /// Delete a sample's directory. `false` when there was nothing
    /// there. The audio in its File Root is not touched — deleting the
    /// manifest un-declares the sample, it does not destroy content
    /// this lane never held.
    fn delete_sample(&self, slug: &str) -> Result<bool, ResourcesError>;

    /// Create or replace a lighting document under
    /// `<org>/resources/lighting/<slug>/`: `show.json` holds
    /// [`LightingDoc::body`] verbatim and `show.md` is the
    /// `type: resource`, `resource_kind: lighting` manifest.
    ///
    /// [`LightingDoc::scope`] must be `song`, `setlist` or `show`;
    /// anything else is a `BadRequest` rather than a silently stored
    /// word nobody can act on. `cues` is taken from the caller, not
    /// parsed — an unlisted cue is not addressable as
    /// `lighting:<slug>#cue:<label>`.
    ///
    /// An empty title, or one that kebabs to nothing, is a
    /// `BadRequest`.
    fn upsert_lighting(&self, lighting: LightingDoc) -> Result<LightingUpsert, ResourcesError>;

    /// One lighting document by slug, body included.
    fn lighting(&self, slug: &str) -> Result<LightingDoc, ResourcesError>;

    /// Every lighting document under `resources/lighting/`, by slug.
    fn list_lighting(&self) -> Result<Vec<LightingSummary>, ResourcesError>;

    /// Delete a lighting document's directory. `false` when there was
    /// nothing there; references to it are left dangling and legible.
    fn delete_lighting(&self, slug: &str) -> Result<bool, ResourcesError>;
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{
        ChartDoc, ChartSummary, ChartUpsert, ContentRef, LightingDoc, LightingSummary,
        LightingUpsert, PatchDoc, PatchSummary, PatchUpsert, SampleDoc, SampleSummary,
        SampleUpsert, SermonResource, SermonSummary, SermonUpsert, TranscriptDoc,
        TranscriptSegment,
    };
    unsafe impl vox_types::Reborrow for TranscriptSegment {
        type Ref<'a> = TranscriptSegment;
    }
    unsafe impl vox_types::Reborrow for TranscriptDoc {
        type Ref<'a> = TranscriptDoc;
    }
    unsafe impl vox_types::Reborrow for SermonResource {
        type Ref<'a> = SermonResource;
    }
    unsafe impl vox_types::Reborrow for SermonUpsert {
        type Ref<'a> = SermonUpsert;
    }
    unsafe impl vox_types::Reborrow for SermonSummary {
        type Ref<'a> = SermonSummary;
    }
    unsafe impl vox_types::Reborrow for ChartDoc {
        type Ref<'a> = ChartDoc;
    }
    unsafe impl vox_types::Reborrow for ChartUpsert {
        type Ref<'a> = ChartUpsert;
    }
    unsafe impl vox_types::Reborrow for ChartSummary {
        type Ref<'a> = ChartSummary;
    }
    unsafe impl vox_types::Reborrow for ContentRef {
        type Ref<'a> = ContentRef;
    }
    unsafe impl vox_types::Reborrow for PatchDoc {
        type Ref<'a> = PatchDoc;
    }
    unsafe impl vox_types::Reborrow for PatchUpsert {
        type Ref<'a> = PatchUpsert;
    }
    unsafe impl vox_types::Reborrow for PatchSummary {
        type Ref<'a> = PatchSummary;
    }
    unsafe impl vox_types::Reborrow for SampleDoc {
        type Ref<'a> = SampleDoc;
    }
    unsafe impl vox_types::Reborrow for SampleUpsert {
        type Ref<'a> = SampleUpsert;
    }
    unsafe impl vox_types::Reborrow for SampleSummary {
        type Ref<'a> = SampleSummary;
    }
    unsafe impl vox_types::Reborrow for LightingDoc {
        type Ref<'a> = LightingDoc;
    }
    unsafe impl vox_types::Reborrow for LightingUpsert {
        type Ref<'a> = LightingUpsert;
    }
    unsafe impl vox_types::Reborrow for LightingSummary {
        type Ref<'a> = LightingSummary;
    }
}
