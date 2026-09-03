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
    /// `tags:` frontmatter (`[sermon, crossroads]`).
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
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{SermonResource, SermonSummary, SermonUpsert, TranscriptDoc, TranscriptSegment};
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
}
