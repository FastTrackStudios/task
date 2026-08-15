//! Core graph data model.
//!
//! Mirrors the TS interfaces in `wiki-graph.ts`: [`GraphNode`],
//! [`GraphEdge`], [`CommunityInfo`]. Node `kind` is the wiki page
//! `type:` frontmatter value; we keep it as a free string (wikis grow
//! custom types) but map the known ones to a stable Tailwind palette
//! stem via [`NodeKind`] so the renderer never hardcodes hex.

use serde::{Deserialize, Serialize};

/// A wiki page as a graph node.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphNode {
    /// Stable id — the file name without `.md`.
    pub id: String,
    /// Display title (frontmatter `title:`, else first `# heading`,
    /// else the de-slugged file name).
    pub label: String,
    /// Frontmatter `type:`, lowercased; `"other"` when absent.
    pub kind: String,
    /// Absolute path to the source markdown file.
    pub path: String,
    /// Inbound + outbound link count (node degree).
    pub link_count: u32,
    /// Community id from detection ([`crate::community`]).
    pub community: u32,
}

/// A resolved `[[wikilink]]` between two pages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GraphEdge {
    pub source: String,
    pub target: String,
    /// Relevance score (see [`crate::relevance`]); defaults to `1.0`.
    pub weight: f32,
    /// Typed relation (`cross-ref`, `supports`, `mentions`, …). Empty for
    /// a plain wikilink.
    #[serde(default)]
    pub relation: String,
    /// Ordinal confidence `0..=4` (Speculative→Certain) for typed links.
    /// `None` = an unrated structural wikilink, always shown.
    #[serde(default)]
    pub confidence: Option<u8>,
}

impl GraphEdge {
    /// A plain (unrated) wikilink edge.
    #[must_use]
    pub fn wikilink(source: String, target: String, weight: f32) -> Self {
        Self {
            source,
            target,
            weight,
            relation: String::new(),
            confidence: None,
        }
    }
}

/// Summary of one detected community.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommunityInfo {
    pub id: u32,
    pub node_count: usize,
    /// Intra-community edge density, `0.0..=1.0`.
    pub cohesion: f32,
    /// Up to 5 member labels, highest `link_count` first.
    pub top_nodes: Vec<String>,
}

/// The fully built graph: nodes, edges, and community summaries.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct WikiGraph {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub communities: Vec<CommunityInfo>,
}

/// How node fill colors are chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ColorMode {
    /// Color by page `kind` (the [`NodeKind`] palette).
    #[default]
    Kind,
    /// Color by community id (cycled palette).
    Community,
}

/// Known wiki page kinds, each mapped to a fixed Tailwind palette stem.
///
/// Unknown kinds fall back to a deterministic stem hash so custom types
/// stay visually stable across renders. We expose *stems* (e.g.
/// `"blue"`) rather than hex so SVG fills use `fill-blue-400` utilities
/// and respect the theme system (per AGENTS.md: no raw hex in the UI).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Entity,
    Concept,
    Source,
    Query,
    Synthesis,
    Overview,
    Comparison,
    Finding,
    Thesis,
    Methodology,
    Other,
}

impl NodeKind {
    /// Parse a frontmatter `type:` string (already lowercased upstream,
    /// but we lowercase defensively).
    pub fn from_kind(kind: &str) -> Self {
        match kind.trim().to_ascii_lowercase().as_str() {
            "entity" => NodeKind::Entity,
            "concept" => NodeKind::Concept,
            "source" => NodeKind::Source,
            "query" => NodeKind::Query,
            "synthesis" => NodeKind::Synthesis,
            "overview" => NodeKind::Overview,
            "comparison" => NodeKind::Comparison,
            "finding" => NodeKind::Finding,
            "thesis" => NodeKind::Thesis,
            "methodology" => NodeKind::Methodology,
            _ => NodeKind::Other,
        }
    }

    /// Tailwind color stem for this kind, matching the llm_wiki palette.
    pub fn stem(self) -> &'static str {
        match self {
            NodeKind::Entity => "blue",
            NodeKind::Concept => "purple",
            NodeKind::Source => "orange",
            NodeKind::Query => "green",
            NodeKind::Synthesis => "red",
            NodeKind::Overview => "yellow",
            NodeKind::Comparison => "teal",
            NodeKind::Finding => "fuchsia",
            NodeKind::Thesis => "rose",
            NodeKind::Methodology => "emerald",
            NodeKind::Other => "slate",
        }
    }
}

/// Palette stem for an arbitrary `kind` string — known kinds use their
/// fixed stem, unknown kinds hash deterministically into a fallback set.
pub fn stem_for_kind(kind: &str) -> &'static str {
    let parsed = NodeKind::from_kind(kind);
    if parsed != NodeKind::Other || kind.eq_ignore_ascii_case("other") {
        return parsed.stem();
    }
    const FALLBACK: [&str; 8] = [
        "sky", "emerald", "amber", "rose", "violet", "cyan", "orange", "lime",
    ];
    let mut hash: u32 = 0;
    for ch in kind.chars() {
        hash = hash.wrapping_mul(31).wrapping_add(ch as u32);
    }
    FALLBACK[(hash as usize) % FALLBACK.len()]
}

/// Tailwind stems cycled for community coloring (12-wide, matches the
/// llm_wiki `COMMUNITY_COLORS` ordering).
pub const COMMUNITY_STEMS: [&str; 12] = [
    "blue", "green", "orange", "purple", "red", "teal", "yellow", "pink", "violet", "sky",
    "emerald", "amber",
];

/// Stem for a community id (cycled).
pub fn community_stem(community: u32) -> &'static str {
    COMMUNITY_STEMS[(community as usize) % COMMUNITY_STEMS.len()]
}

/// Tailwind text-color class (`text-<stem>-400`) for a palette stem,
/// returned as a **literal** so Tailwind v4's source scanner generates
/// it (interpolated `text-{stem}-400` strings are invisible to it).
/// The renderer pairs this with `fill-current` to color SVG nodes.
pub fn stem_text_class(stem: &str) -> &'static str {
    match stem {
        "blue" => "text-blue-400",
        "purple" => "text-purple-400",
        "orange" => "text-orange-400",
        "green" => "text-green-400",
        "red" => "text-red-400",
        "yellow" => "text-yellow-400",
        "teal" => "text-teal-400",
        "fuchsia" => "text-fuchsia-400",
        "rose" => "text-rose-400",
        "emerald" => "text-emerald-400",
        "pink" => "text-pink-400",
        "violet" => "text-violet-400",
        "sky" => "text-sky-400",
        "amber" => "text-amber-400",
        "cyan" => "text-cyan-400",
        "lime" => "text-lime-400",
        // "slate" and any unknown stem fall through to the neutral.
        _ => "text-slate-400",
    }
}

/// Node text-color class for a wiki `kind`.
pub fn kind_text_class(kind: &str) -> &'static str {
    stem_text_class(stem_for_kind(kind))
}

/// Node text-color class for a community id.
pub fn community_text_class(community: u32) -> &'static str {
    stem_text_class(community_stem(community))
}

/// Concrete hex color (Tailwind 400-stop) for a palette stem — for SVG
/// fills that must render identically in every host app. The
/// class-based variants above depend on the host's Tailwind scanner
/// having seen this crate's sources; a consumer that doesn't scan us
/// (e.g. another app's tailwind pipeline) would silently render black.
/// These hues sit at the 400 stop, muted enough for a dark background
/// and saturated enough for a light one.
pub fn stem_color(stem: &str) -> &'static str {
    match stem {
        "blue" => "#60a5fa",
        "purple" => "#c084fc",
        "orange" => "#fb923c",
        "green" => "#4ade80",
        "red" => "#f87171",
        "yellow" => "#facc15",
        "teal" => "#2dd4bf",
        "fuchsia" => "#e879f9",
        "rose" => "#fb7185",
        "emerald" => "#34d399",
        "pink" => "#f472b6",
        "violet" => "#a78bfa",
        "sky" => "#38bdf8",
        "amber" => "#fbbf24",
        "cyan" => "#22d3ee",
        "lime" => "#a3e635",
        _ => "#94a3b8", // slate-400
    }
}

/// Hex node color for a wiki `kind`.
pub fn kind_color(kind: &str) -> &'static str {
    stem_color(stem_for_kind(kind))
}

/// Hex node color for a community id.
pub fn community_color(community: u32) -> &'static str {
    stem_color(community_stem(community))
}

/// Tailwind background-color class (`bg-<stem>-400`) for a palette stem,
/// returned as a literal (same scanner requirement as
/// [`stem_text_class`]). Used for legend swatches.
pub fn stem_bg_class(stem: &str) -> &'static str {
    match stem {
        "blue" => "bg-blue-400",
        "purple" => "bg-purple-400",
        "orange" => "bg-orange-400",
        "green" => "bg-green-400",
        "red" => "bg-red-400",
        "yellow" => "bg-yellow-400",
        "teal" => "bg-teal-400",
        "fuchsia" => "bg-fuchsia-400",
        "rose" => "bg-rose-400",
        "emerald" => "bg-emerald-400",
        "pink" => "bg-pink-400",
        "violet" => "bg-violet-400",
        "sky" => "bg-sky-400",
        "amber" => "bg-amber-400",
        "cyan" => "bg-cyan-400",
        "lime" => "bg-lime-400",
        _ => "bg-slate-400",
    }
}

/// Swatch background class for a wiki `kind`.
pub fn kind_bg_class(kind: &str) -> &'static str {
    stem_bg_class(stem_for_kind(kind))
}

/// Swatch background class for a community id.
pub fn community_bg_class(community: u32) -> &'static str {
    stem_bg_class(community_stem(community))
}

/// Human-readable label for a wiki `kind` — title-cased, hyphens to
/// spaces (e.g. `methodology` → `Methodology`, `key-finding` → `Key
/// Finding`). The TS app sources these from i18n; this is the
/// English-only fallback.
pub fn kind_label(kind: &str) -> String {
    kind.split(['-', '_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_kinds_map_to_fixed_stems() {
        assert_eq!(NodeKind::from_kind("Concept").stem(), "purple");
        assert_eq!(stem_for_kind("source"), "orange");
        assert_eq!(stem_for_kind("other"), "slate");
    }

    #[test]
    fn unknown_kinds_hash_stably() {
        let a = stem_for_kind("custom-type");
        let b = stem_for_kind("custom-type");
        assert_eq!(a, b);
    }

    #[test]
    fn community_stem_cycles() {
        assert_eq!(community_stem(0), community_stem(12));
    }
}
