//! Wasm-clean data types for the Resource Library + annotations.
//!
//! These carry no filesystem dependency, so a future `*-ui` / `*-proto`
//! split can lift them verbatim. They model two things:
//! - a **[`Resource`]** — a read-only primary source (its manifest is the
//!   `type: resource` markdown frontmatter), and
//! - the **annotation sidecar** ([`AnnotationFile`]) — the per-resource
//!   geometry/metadata store, keyed by the same `#anchor` string the
//!   `links_proto::NodeRef` uses (Logseq's two-layer model: the compact
//!   anchor lives in the link graph, the geometry lives here).

use serde::{Deserialize, Serialize};

/// What kind of primary source a resource is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Song,
    Sermon,
    Pdf,
    Book,
    Video,
    Audio,
    Article,
}

/// One playable / viewable representation of a resource (a song has a
/// YouTube video *and* a lyrics page; a book may have a PDF and an epub).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaRef {
    /// `video` / `audio` / `page` / `pdf`.
    pub kind: String,
    /// `youtube` / `spotify` / `worshiptogether` / `file` …
    #[serde(default)]
    pub provider: String,
    pub url: String,
    /// The provider's own id for the media (a YouTube video id). The
    /// sermon sync keys resource identity on it: one id, one slug.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
}

/// A resource manifest — the read-only Library entry. Mirrors the
/// `type: resource` frontmatter of e.g.
/// `<org>/resources/songs/<slug>.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resource {
    pub slug: String,
    #[serde(rename = "resource_kind")]
    pub kind: ResourceKind,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub writers: Vec<String>,
    #[serde(default)]
    pub media: Vec<MediaRef>,
    #[serde(default)]
    pub readonly: bool,
    /// Free tags (`[sermon, crossroads]`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// `YYYY-MM-DD` publication / upload date; empty when unknown.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub published: String,
    /// Media length in seconds; `0` when unknown.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub duration_secs: u64,
    /// Scripture the resource references, as OSIS ids in first-mention
    /// order (`1Pet.5.7`, `John.21.15-John.21.17`, `1Pet.5`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripture: Vec<String>,
    /// Where the content came from (`youtube-captions`, `manual`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
    /// For caption-sourced transcripts: `manual` or `auto`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub caption_kind: String,
    /// Caption / transcript language (`en`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub language: String,
}

#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Resource {
    /// The first media ref of a given `kind` (`"video"`, `"audio"`, …).
    #[must_use]
    pub fn media_of(&self, kind: &str) -> Option<&MediaRef> {
        self.media.iter().find(|m| m.kind == kind)
    }
}

/// A scaled rectangle in resource space (e.g. PDF page coordinates,
/// `0..page_width`/`0..page_height`) — *not* viewport pixels, so it
/// survives zoom. Matches Logseq's highlight `:bounding` / `:rects`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
    /// Page width the coords were captured against (for viewport scaling).
    pub width: f32,
    /// Page height the coords were captured against.
    pub height: f32,
}

/// The geometry behind an anchor — the part that does *not* fit in the
/// compact anchor string and lives in the sidecar instead.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Geometry {
    /// A recording moment (seconds). The anchor string is `t:<secs>`.
    Timestamp { secs: u32 },
    /// A PDF highlight region (page + scaled box + per-line rects + an
    /// optional captured-area image filename). The anchor string is
    /// `p<page>.<id>`. Mirrors Logseq's PDF highlight shape.
    PdfRegion {
        page: u32,
        bounding: Rect,
        #[serde(default)]
        rects: Vec<Rect>,
        #[serde(default)]
        image: Option<String>,
    },
}

/// One annotation on a resource — the sidecar counterpart of a
/// `links_proto::Anchor`. The links it carries live in the shared
/// `links.jsonl`; this row holds the human label, captured text, colour,
/// and (for PDF/media) the [`Geometry`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Annotation {
    /// The anchor string — the part after `#` in the `NodeRef` token
    /// (`chorus.L1`, `t:90`, `p3.h2`). Unique within a resource.
    pub anchor: String,
    /// Short human label (e.g. the lyric line, or the highlighted phrase).
    #[serde(default)]
    pub label: String,
    /// The captured source text the annotation covers.
    #[serde(default)]
    pub text: String,
    /// Highlight colour (`yellow`/`red`/…), Logseq-style. `None` = default.
    #[serde(default)]
    pub color: Option<String>,
    /// Region/timestamp geometry. `None` for a plain text/lyric span,
    /// whose location is the anchor label resolved against the resource.
    #[serde(default)]
    pub geometry: Option<Geometry>,
}

/// The per-resource annotation sidecar — serialised to
/// `<resource-path>.annotations.json` next to the resource, exactly as
/// Logseq writes `<pdf>.edn` next to the asset.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AnnotationFile {
    pub slug: String,
    #[serde(default)]
    pub annotations: Vec<Annotation>,
}

impl AnnotationFile {
    #[must_use]
    pub fn new(slug: impl Into<String>) -> Self {
        Self {
            slug: slug.into(),
            annotations: Vec::new(),
        }
    }

    /// Look up an annotation by its anchor string.
    #[must_use]
    pub fn get(&self, anchor: &str) -> Option<&Annotation> {
        self.annotations.iter().find(|a| a.anchor == anchor)
    }

    /// Insert or replace an annotation (keyed by `anchor`).
    pub fn upsert(&mut self, ann: Annotation) {
        if let Some(slot) = self.annotations.iter_mut().find(|a| a.anchor == ann.anchor) {
            *slot = ann;
        } else {
            self.annotations.push(ann);
        }
    }
}
