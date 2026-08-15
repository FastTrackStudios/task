//! [`NodeRef`] — a universal reference to anything a link can touch.
//!
//! A node is a verse, a vault note, a wiki page, a topic, an entity
//! (person/place), a block, or an external URL. The `id` is interpreted
//! per [`NodeKind`] (an OSIS verse id, a vault path, a topic slug, …).
//! The canonical string form is `kind:id` (e.g. `verse:John.3.16`,
//! `note:Journal/2026-06-16.md`, `topic:money`).
//!
//! ## Anchors — sub-node addressing
//!
//! A bare `NodeRef` points at a whole node. An optional `anchor` narrows
//! it to a span *inside* that node — generalizing Obsidian's
//! `[[page#^block]]` to verses, words, and blocks uniformly:
//! - `verse:John.3.16#word:5` — the 5th original-language word of a verse.
//! - `note:Journal/2026.md#^abc123` — a Logseq/Obsidian block in a note.
//! - `verse:John.3.16-18` is a *range id*; `#` is for sub-node spans, not
//!   verse ranges (those live in the `id`).
//!
//! The token form is `kind:id#anchor`; the `#anchor` is omitted when
//! empty, so legacy `kind:id` tokens still round-trip.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// What kind of thing a [`NodeRef`] points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum NodeKind {
    /// A Bible verse — `id` is the OSIS reference (`John.3.16`).
    Verse,
    /// A vault note — `id` is the vault-relative path.
    Note,
    /// A wiki page — `id` is the wiki-relative path.
    Wiki,
    /// A topic / tag — `id` is the topic slug (`money`).
    Topic,
    /// An entity (person, place, thing) — `id` is its stable id.
    Entity,
    /// A block within a note — `id` is the block uuid.
    Block,
    /// A song / recorded work — `id` is the song slug
    /// (`keep-on-finding-more`). Its YouTube, audio, and lyric
    /// representations are all the *same* node; the `anchor` addresses
    /// into it (a timestamp `t:1:30` for the recording, or a lyric span
    /// `chorus.L1`). The lyrics/chart live as a read-only resource under
    /// `<org>/resources/songs/`.
    Song,
    /// A sermon / talk — `id` is the sermon slug. A video resource with a
    /// timestamped transcript; annotations anchor to a moment (`#t:1830`)
    /// and carry what was said. Lives under `<org>/resources/sermons/`.
    Sermon,
    /// A video — `id` is the video slug; the resource note holds the
    /// YouTube/URL. Annotate by timestamp (`#t:90`) or clip
    /// (`#t:263-983` = 4:23–16:23). The generic case; Song/Sermon are
    /// specializations.
    Video,
    /// An external resource — `id` is the URL.
    External,
}

impl NodeKind {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Verse => "verse",
            Self::Note => "note",
            Self::Wiki => "wiki",
            Self::Topic => "topic",
            Self::Entity => "entity",
            Self::Block => "block",
            Self::Song => "song",
            Self::Sermon => "sermon",
            Self::Video => "video",
            Self::External => "external",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "verse" => Self::Verse,
            "note" => Self::Note,
            "wiki" => Self::Wiki,
            "topic" => Self::Topic,
            "entity" => Self::Entity,
            "block" => Self::Block,
            "song" => Self::Song,
            "sermon" => Self::Sermon,
            "video" => Self::Video,
            "external" => Self::External,
            _ => return None,
        })
    }
}

/// A reference to one node — or a span inside one — in the graph.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
pub struct NodeRef {
    pub kind: NodeKind,
    pub id: String,
    /// Optional sub-node span (`word:5`, `^abc123`). Empty = the whole
    /// node. `#[serde(default)]` keeps legacy two-field tokens readable.
    #[serde(default)]
    pub anchor: String,
}

impl NodeRef {
    #[must_use]
    pub fn new(kind: NodeKind, id: impl Into<String>) -> Self {
        Self {
            kind,
            id: id.into(),
            anchor: String::new(),
        }
    }

    /// A verse node (`id` = OSIS).
    #[must_use]
    pub fn verse(osis: impl Into<String>) -> Self {
        Self::new(NodeKind::Verse, osis)
    }

    /// A block node (`id` = block uuid), resolvable via the vault's
    /// `BlockIndex`.
    #[must_use]
    pub fn block(uuid: impl Into<String>) -> Self {
        Self::new(NodeKind::Block, uuid)
    }

    /// Narrow this reference to a sub-node span (builder). An empty
    /// `anchor` clears it back to the whole node.
    #[must_use]
    pub fn with_anchor(mut self, anchor: impl Into<String>) -> Self {
        self.anchor = anchor.into();
        self
    }

    /// The Nth original-language word of a verse (`verse:John.3.16#word:5`).
    #[must_use]
    pub fn word(osis: impl Into<String>, index: usize) -> Self {
        Self::verse(osis).with_anchor(format!("word:{index}"))
    }

    /// A song node (`id` = slug). Bare = the whole work; add a
    /// [`Self::with_anchor`] for a lyric span or [`Self::at`] for a
    /// recording timestamp.
    #[must_use]
    pub fn song(slug: impl Into<String>) -> Self {
        Self::new(NodeKind::Song, slug)
    }

    /// A sermon node (`id` = slug). Pair with [`Self::at`] for a moment
    /// in the talk (`sermon:god-restores-broken-people#t:1830`).
    #[must_use]
    pub fn sermon(slug: impl Into<String>) -> Self {
        Self::new(NodeKind::Sermon, slug)
    }

    /// A video node (`id` = slug). The resource note holds the YouTube
    /// URL; anchor with [`Self::at`] for a moment or [`Self::clip`] for a
    /// range.
    #[must_use]
    pub fn video(slug: impl Into<String>) -> Self {
        Self::new(NodeKind::Video, slug)
    }

    /// Anchor a media node to a recording timestamp in seconds
    /// (`song:keep-on-finding-more#t:90`). Renders/parses as `t:<secs>`;
    /// players format it back to `m:ss` for display.
    #[must_use]
    pub fn at(self, secs: u32) -> Self {
        self.with_anchor(format!("t:{secs}"))
    }

    /// Anchor a media node to a clip — a timestamp *range* in seconds
    /// (`video:my-talk#t:263-983`). Players scrub from `start`, stopping
    /// at `end`.
    #[must_use]
    pub fn clip(self, start_secs: u32, end_secs: u32) -> Self {
        self.with_anchor(format!("t:{start_secs}-{end_secs}"))
    }

    /// Anchor a clip from human timecodes (`"4:23"`, `"16:23"` →
    /// `t:263-983`). Returns the node unanchored if either timecode is
    /// unparseable.
    #[must_use]
    pub fn clip_from_timecode(self, start: &str, end: &str) -> Self {
        match (parse_timecode(start), parse_timecode(end)) {
            (Some(a), Some(b)) => self.clip(a, b),
            _ => self,
        }
    }

    /// True when this points at a span inside a node, not the whole node.
    #[must_use]
    pub fn has_anchor(&self) -> bool {
        !self.anchor.is_empty()
    }

    /// Classify the anchor for display + navigation. The string stays the
    /// canonical key (it's what's stored in `links.jsonl`); this just
    /// tells the renderer whether to show a "📍 2:05" seek chip, a word
    /// pill, a PDF region jump, etc. Rich geometry (PDF rects, captured
    /// image, color) lives in the resource's annotation sidecar keyed by
    /// this same anchor — never in the wire token (Logseq's two-layer
    /// model: compact ref in the graph, geometry in the `.edn` sidecar).
    #[must_use]
    pub fn anchor_kind(&self) -> Anchor {
        Anchor::parse(&self.anchor)
    }

    /// Canonical `kind:id` (or `kind:id#anchor`) string.
    #[must_use]
    pub fn to_token(&self) -> String {
        if self.anchor.is_empty() {
            format!("{}:{}", self.kind.as_str(), self.id)
        } else {
            format!("{}:{}#{}", self.kind.as_str(), self.id, self.anchor)
        }
    }

    /// Parse a `kind:id` or `kind:id#anchor` token. The first `#` splits
    /// the anchor off, so verse-range ids (`John.3.16-18`) stay intact.
    #[must_use]
    pub fn parse(token: &str) -> Option<Self> {
        let (head, anchor) = match token.split_once('#') {
            Some((h, a)) => (h, a),
            None => (token, ""),
        };
        let (kind, id) = head.split_once(':')?;
        Some(Self::new(NodeKind::parse(kind)?, id).with_anchor(anchor))
    }
}

impl std::fmt::Display for NodeRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.to_token())
    }
}

/// A classified [`NodeRef`] anchor — the *grammar* of the `#…` span, for
/// display and navigation. The geometry behind a [`Anchor::Region`] (PDF
/// bounding box / rects / captured image / color) is resolved separately
/// from the resource's annotation sidecar; only the key lives here.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Anchor {
    /// No anchor — the whole node.
    Whole,
    /// A recording timestamp in whole seconds (`t:90`). Players seek here.
    Timestamp(u32),
    /// A clip — a timestamp *range* in seconds (`t:263-983` = 4:23–16:23).
    /// Players scrub from `start` and can stop at `end`. The human form is
    /// written `mm:ss-mm:ss` (`[[my-talk#4:23-16:23]]`).
    Clip { start: u32, end: u32 },
    /// The Nth original-language word of a verse (`word:5`).
    Word(usize),
    /// A Logseq/Obsidian block id (`^abc123`).
    Block(String),
    /// A region in a paged resource — a PDF highlight key (`p3.h2` =
    /// page 3, highlight 2). The page is parsed out for the jump; the
    /// full id keys the geometry sidecar.
    Region { page: u32, id: String },
    /// A named text span — a lyric line/section (`chorus.L1`) or any
    /// other opaque label.
    Span(String),
}

impl Anchor {
    /// Classify an anchor string (the part after `#`).
    #[must_use]
    pub fn parse(anchor: &str) -> Self {
        if anchor.is_empty() {
            return Self::Whole;
        }
        if let Some(rest) = anchor.strip_prefix("t:") {
            // A clip is a `start-end` range; a bare time is a point. Both
            // accept seconds (`263`) or `mm:ss` / `h:mm:ss` (`4:23`).
            if let Some((a, b)) = rest.split_once('-') {
                if let (Some(start), Some(end)) = (parse_timecode(a), parse_timecode(b)) {
                    return Self::Clip { start, end };
                }
            } else if let Some(secs) = parse_timecode(rest) {
                return Self::Timestamp(secs);
            }
        }
        if let Some(rest) = anchor.strip_prefix("word:") {
            if let Ok(n) = rest.parse::<usize>() {
                return Self::Word(n);
            }
        }
        if let Some(rest) = anchor.strip_prefix('^') {
            return Self::Block(rest.to_string());
        }
        // `p<page>.<id>` — a PDF region key.
        if let Some(rest) = anchor.strip_prefix('p') {
            if let Some((page, id)) = rest.split_once('.') {
                if let Ok(page) = page.parse::<u32>() {
                    return Self::Region {
                        page,
                        id: id.to_string(),
                    };
                }
            }
        }
        Self::Span(anchor.to_string())
    }
}

/// Parse a timecode into whole seconds. Accepts bare seconds (`263`),
/// `mm:ss` (`4:23` → 263), or `h:mm:ss` (`1:04:23` → 3863). `None` on
/// garbage. The inverse of [`format_timecode`].
#[must_use]
pub fn parse_timecode(s: &str) -> Option<u32> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    if !s.contains(':') {
        return s.parse::<u32>().ok();
    }
    // Colon-separated, biggest unit first: fold into seconds base-60.
    let mut total: u32 = 0;
    for part in s.split(':') {
        let n: u32 = part.trim().parse().ok()?;
        total = total.checked_mul(60)?.checked_add(n)?;
    }
    Some(total)
}

/// Render whole seconds as a timecode — `mm:ss`, or `h:mm:ss` past an
/// hour (`263` → `4:23`, `3863` → `1:04:23`). The inverse of
/// [`parse_timecode`].
#[must_use]
pub fn format_timecode(secs: u32) -> String {
    let (h, m, s) = (secs / 3600, (secs % 3600) / 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_round_trips() {
        let n = NodeRef::verse("John.3.16");
        assert_eq!(n.to_token(), "verse:John.3.16");
        assert!(!n.has_anchor());
        assert_eq!(NodeRef::parse("verse:John.3.16"), Some(n));
        assert_eq!(
            NodeRef::parse("note:Journal/2026.md"),
            Some(NodeRef::new(NodeKind::Note, "Journal/2026.md"))
        );
        assert_eq!(NodeRef::parse("bogus"), None);
        assert_eq!(NodeRef::parse("nope:x"), None);
    }

    #[test]
    fn anchor_round_trips() {
        let w = NodeRef::word("John.3.16", 5);
        assert_eq!(w.to_token(), "verse:John.3.16#word:5");
        assert!(w.has_anchor());
        assert_eq!(NodeRef::parse("verse:John.3.16#word:5"), Some(w));

        // Block anchor (Logseq `^uuid` style).
        let b = NodeRef::new(NodeKind::Note, "Journal/2026.md").with_anchor("^abc123");
        assert_eq!(b.to_token(), "note:Journal/2026.md#^abc123");
        assert_eq!(NodeRef::parse("note:Journal/2026.md#^abc123"), Some(b));
    }

    #[test]
    fn song_anchors_for_lyrics_and_timestamps() {
        // Whole work, a lyric span, and a recording timestamp.
        let work = NodeRef::song("keep-on-finding-more");
        assert_eq!(work.to_token(), "song:keep-on-finding-more");
        let lyric = NodeRef::song("keep-on-finding-more").with_anchor("chorus.L1");
        assert_eq!(lyric.to_token(), "song:keep-on-finding-more#chorus.L1");
        let moment = NodeRef::song("keep-on-finding-more").at(90);
        assert_eq!(moment.to_token(), "song:keep-on-finding-more#t:90");
        assert_eq!(
            NodeRef::parse("song:keep-on-finding-more#t:90"),
            Some(moment)
        );
    }

    #[test]
    fn anchor_kinds_classify() {
        use NodeKind::{Note, Song, Verse};
        assert_eq!(NodeRef::song("x").anchor_kind(), Anchor::Whole);
        assert_eq!(
            NodeRef::song("x").at(125).anchor_kind(),
            Anchor::Timestamp(125)
        );
        assert_eq!(NodeRef::word("John.3.16", 5).anchor_kind(), Anchor::Word(5));
        assert_eq!(
            NodeRef::new(Note, "j.md").with_anchor("^abc").anchor_kind(),
            Anchor::Block("abc".into())
        );
        assert_eq!(
            NodeRef::new(NodeKind::External, "book.pdf")
                .with_anchor("p3.h2")
                .anchor_kind(),
            Anchor::Region {
                page: 3,
                id: "h2".into()
            }
        );
        assert_eq!(
            NodeRef::new(Song, "x")
                .with_anchor("chorus.L1")
                .anchor_kind(),
            Anchor::Span("chorus.L1".into())
        );
        // A bare verse word-less anchor that isn't a known scheme stays a Span.
        assert_eq!(
            NodeRef::new(Verse, "John.3.16")
                .with_anchor("phrase")
                .anchor_kind(),
            Anchor::Span("phrase".into())
        );
    }

    #[test]
    fn video_clip_anchors_and_timecodes() {
        // Point + clip via the builders (seconds).
        let moment = NodeRef::video("my-talk").at(263);
        assert_eq!(moment.to_token(), "video:my-talk#t:263");
        assert_eq!(moment.anchor_kind(), Anchor::Timestamp(263));

        let clip = NodeRef::video("my-talk").clip(263, 983);
        assert_eq!(clip.to_token(), "video:my-talk#t:263-983");
        assert_eq!(
            clip.anchor_kind(),
            Anchor::Clip {
                start: 263,
                end: 983
            }
        );
        assert_eq!(NodeRef::parse("video:my-talk#t:263-983"), Some(clip));

        // Human mm:ss clip → seconds.
        let from_tc = NodeRef::video("my-talk").clip_from_timecode("4:23", "16:23");
        assert_eq!(from_tc.to_token(), "video:my-talk#t:263-983");

        // The anchor can also be written in mm:ss directly and still classify.
        assert_eq!(
            NodeRef::video("x")
                .with_anchor("t:4:23-16:23")
                .anchor_kind(),
            Anchor::Clip {
                start: 263,
                end: 983
            }
        );

        // Timecode round-trips.
        assert_eq!(parse_timecode("4:23"), Some(263));
        assert_eq!(parse_timecode("1:04:23"), Some(3863));
        assert_eq!(parse_timecode("90"), Some(90));
        assert_eq!(parse_timecode("nope"), None);
        assert_eq!(format_timecode(263), "4:23");
        assert_eq!(format_timecode(3863), "1:04:23");
    }

    #[test]
    fn verse_range_id_survives_anchor_split() {
        // A cross-verse range lives in the id; only the first `#` (none
        // here) would split an anchor, so the range stays whole.
        let r = NodeRef::verse("John.3.16-18");
        assert_eq!(r.to_token(), "verse:John.3.16-18");
        assert_eq!(NodeRef::parse("verse:John.3.16-18"), Some(r));
    }

    #[test]
    fn legacy_token_deserializes_without_anchor() {
        // Pre-anchor JSONL had no `anchor` field — `serde(default)` fills it.
        let n: NodeRef = serde_json::from_str(r#"{"kind":"Verse","id":"John.3.16"}"#).unwrap();
        assert_eq!(n, NodeRef::verse("John.3.16"));
        assert!(!n.has_anchor());
    }
}
