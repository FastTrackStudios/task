//! Charts on the **Assets shelf** — where a chart lives now, and how
//! its source is carried inside an ordinary vault document.
//!
//! ADR 0004 decision 1. The tier itself — what `Assets/` means, how a
//! page is recognised as one, and why any of it is a vault concern —
//! is [`vault_proto::assets`], and the constants are re-exported here
//! so a chart caller reads one module. This file holds only the two
//! things that are about *charts*: their path, and the encoding that
//! lets a chart be one markdown file.
//!
//! # The chart source lives in the body, in a fence
//!
//! Under ADR 0003 a chart was two files: `<slug>.md` (the manifest) and
//! `<slug>.kf` (the source, verbatim). That split cannot survive the
//! move, and the reason is mechanical rather than aesthetic: the vault
//! walker collects `.md` and `.base` and nothing else
//! (`vault_live::walker`), so a `.kf` under the vault root would be a
//! file the vault does not know about — not indexed, not searched, not
//! in the graph, not a page anyone can open, and above all not a thing
//! `vault-collab` would ever be asked to hold a document for. Charts
//! would have moved house and gained nothing.
//!
//! So the source becomes a fenced block in the document itself:
//!
//! ```text
//! ---
//! type: asset
//! asset_kind: chart
//! slug: doxology
//! ---
//! # Doxology
//!
//! ```keyflow
//! [Verse]
//! | G | C | D | G |
//! ```
//!
//! ## Notes
//! ```
//!
//! One file, one path, one CRDT document. Two people editing "a chart"
//! are editing one vault markdown document through exactly the path two
//! people editing a note use, and the prose they write around the chart
//! converges with the chart itself — which is what "these are all just
//! manipulations of the markdown files" has to mean if it means
//! anything. A fenced code block is also the most boring possible
//! representation: every markdown editor renders it, and an editor that
//! has never heard of Task shows a person their chart rather than a
//! blob.
//!
//! Something is given up and it should be named: an outside editor can
//! no longer open a file that is *only* the chart. What it opens is a
//! document whose largest block is the chart. In exchange the chart
//! becomes collaborative, searchable, linkable and reviewable, which is
//! the trade ADR 0004 makes on purpose.
//!
//! The one byte the round trip does not preserve: a fence cannot
//! represent a source that does not end in a newline, so [`fence`]
//! adds one. Recorded rather than hidden.

pub use vault_proto::assets::{ASSETS_DIR, KIND_KEY, TYPE_ASSET, TYPE_KEY, is_asset_path};

/// The per-kind subdirectory charts live in — `Assets/Charts/`.
///
/// Per-kind rather than flat because assets are heterogeneous by
/// definition ("any file and any directory"), and a person opening
/// `Assets/` should see kinds, not a thousand slugs.
pub const CHARTS_DIR: &str = "Charts";

/// [`KIND_KEY`] value for a Keyflow chart.
pub const CHART_KIND: &str = "chart";

/// The per-kind subdirectory songs live in — `Assets/Songs/`.
pub const SONGS_DIR: &str = "Songs";

/// [`KIND_KEY`] value for a song.
pub const SONG_KIND: &str = "song";

/// The info string on the fence holding a chart's source. Also the
/// language tag a markdown renderer highlights on, which is why it is
/// the notation name rather than something Task-specific.
pub const CHART_FENCE: &str = "keyflow";

/// `Assets/Charts` — the vault-relative directory charts live in.
#[must_use]
pub fn charts_dir() -> String {
    format!("{ASSETS_DIR}/{CHARTS_DIR}")
}

/// `Assets/Charts/<slug>.md` — the vault-relative path of one chart.
///
/// The single place this string is composed. An application should
/// prefer the `rel_path` the upsert hands back; a server-side caller
/// that holds only a slug (migration, seed, cross-org resolution) asks
/// here.
#[must_use]
pub fn chart_path(slug: &str) -> String {
    format!("{ASSETS_DIR}/{CHARTS_DIR}/{slug}.md")
}

/// `Assets/Songs` — the vault-relative directory songs live in.
#[must_use]
pub fn songs_dir() -> String {
    format!("{ASSETS_DIR}/{SONGS_DIR}")
}

/// `Assets/Songs/<slug>.md` — the vault-relative path of one song.
///
/// Note what is *not* here: a song's audio. `manifest.json` and the
/// stems stay at `<org>/resources/songs/<slug>/`, because they are
/// Resources in ADR 0004's sense — imported bytes nobody types into,
/// which the `/media` route serves and a cross-org subscription
/// materialises. The document moved; the payload did not, and the two
/// having shared a directory was the accident.
#[must_use]
pub fn song_path(slug: &str) -> String {
    format!("{ASSETS_DIR}/{SONGS_DIR}/{slug}.md")
}

/// The longest run of backticks anywhere in `text`.
fn longest_backtick_run(text: &str) -> usize {
    let mut best = 0;
    let mut run = 0;
    for ch in text.chars() {
        if ch == '`' {
            run += 1;
            best = best.max(run);
        } else {
            run = 0;
        }
    }
    best
}

/// Render `source` as a fenced block tagged `info`.
///
/// The fence is one backtick longer than the longest run inside the
/// source (minimum three), which is CommonMark's own escape hatch and
/// means a chart containing backticks — a comment, a quoted lyric —
/// cannot break out of its own block. A source that does not end in a
/// newline gets one; see the module docs.
#[must_use]
pub fn fence(source: &str, info: &str) -> String {
    let ticks = "`".repeat(longest_backtick_run(source).max(2) + 1);
    let body = if source.is_empty() || source.ends_with('\n') {
        source.to_owned()
    } else {
        format!("{source}\n")
    };
    format!("{ticks}{info}\n{body}{ticks}\n")
}

/// Where a fenced block tagged `info` sits in `body`: the byte range
/// covering the opening fence line through the closing fence line, and
/// the opening fence's backtick count.
fn fenced_range(body: &str, info: &str) -> Option<(std::ops::Range<usize>, usize)> {
    let mut offset = 0usize;
    let mut open: Option<(usize, usize)> = None;
    for line in body.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let trimmed = line.trim_end_matches(['\n', '\r']);
        let ticks = trimmed.chars().take_while(|c| *c == '`').count();
        match open {
            None => {
                if ticks >= 3 && trimmed[ticks..].trim() == info {
                    open = Some((start, ticks));
                }
            }
            Some((open_start, opened)) => {
                if ticks >= opened && trimmed[ticks..].trim().is_empty() {
                    return Some((open_start..offset, opened));
                }
            }
        }
    }
    // An unterminated fence still owns the rest of the document: that is
    // how a markdown parser reads it, and disagreeing with the parser
    // would let a truncated write append a second block below the
    // wreckage instead of repairing it.
    open.map(|(start, ticks)| (start..body.len(), ticks))
}

/// The contents of the first fenced block tagged `info`, or `None` when
/// the document has no such block.
///
/// Returns the text *between* the fences, so
/// `extract_fenced(&fence(s, i), i) == Some(s)` for every `s` that ends
/// in a newline.
#[must_use]
pub fn extract_fenced(body: &str, info: &str) -> Option<String> {
    let (range, ticks) = fenced_range(body, info)?;
    let block = &body[range];
    let mut lines = block.split_inclusive('\n').peekable();
    lines.next()?; // the opening fence
    let mut out = String::new();
    while let Some(line) = lines.next() {
        let is_last = lines.peek().is_none();
        if is_last {
            let trimmed = line.trim_end_matches(['\n', '\r']);
            let closing = trimmed.chars().take_while(|c| *c == '`').count();
            // The closing fence is not content. An unterminated fence
            // has no closing line, so its last line is.
            if closing >= ticks && trimmed[closing..].trim().is_empty() {
                break;
            }
        }
        out.push_str(line);
    }
    Some(out)
}

/// Replace the first fenced block tagged `info` with `source`, leaving
/// every other byte of `body` alone.
///
/// This is the whole of the "app-owned vs authored" contract for an
/// asset's body: the app owns its fence, the person owns the prose
/// around it, and a re-save touches one and never the other — the same
/// promise `chart::refresh_manifest` makes about frontmatter keys, and
/// the same promise the sermon sync makes about a manifest body.
///
/// A body with no such fence gets one appended, so a chart whose fence
/// somebody deleted heals on the next save rather than silently losing
/// its source.
#[must_use]
pub fn replace_fenced(body: &str, info: &str, source: &str) -> String {
    let block = fence(source, info);
    match fenced_range(body, info) {
        Some((range, _)) => {
            let mut out = String::with_capacity(body.len() + block.len());
            out.push_str(&body[..range.start]);
            out.push_str(&block);
            out.push_str(&body[range.end..]);
            out
        }
        None if body.is_empty() => block,
        None if body.ends_with("\n\n") => format!("{body}{block}"),
        None if body.ends_with('\n') => format!("{body}\n{block}"),
        None => format!("{body}\n\n{block}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chart_path_is_composed_in_exactly_one_place() {
        assert_eq!(chart_path("doxology"), "Assets/Charts/doxology.md");
        assert_eq!(charts_dir(), "Assets/Charts");
        assert!(is_asset_path(&chart_path("doxology")));
        assert_eq!(song_path("doxology"), "Assets/Songs/doxology.md");
        assert_eq!(songs_dir(), "Assets/Songs");
        assert!(is_asset_path(&song_path("doxology")));
        assert_ne!(
            chart_path("doxology"),
            song_path("doxology"),
            "a song and its chart are two documents, not one"
        );
    }

    #[test]
    fn a_source_round_trips_through_its_fence() {
        let src = "[Verse]\n| G | C | D | G |\n";
        let block = fence(src, CHART_FENCE);
        assert!(block.starts_with("```keyflow\n"), "{block}");
        assert_eq!(extract_fenced(&block, CHART_FENCE).as_deref(), Some(src));
    }

    /// The escape hatch that stops a chart breaking out of its own
    /// block — a source holding a fence of its own.
    #[test]
    fn backticks_in_the_source_widen_the_fence() {
        let src = "before\n```\ninner\n```\nafter\n";
        let block = fence(src, CHART_FENCE);
        assert!(block.starts_with("````keyflow\n"), "{block}");
        assert_eq!(extract_fenced(&block, CHART_FENCE).as_deref(), Some(src));
    }

    #[test]
    fn an_empty_source_is_representable() {
        let block = fence("", CHART_FENCE);
        assert_eq!(block, "```keyflow\n```\n");
        assert_eq!(extract_fenced(&block, CHART_FENCE).as_deref(), Some(""));
    }

    /// The body contract: the app rewrites its fence, and the prose a
    /// person wrote around it comes back byte for byte.
    #[test]
    fn replacing_the_fence_leaves_the_prose_alone() {
        let body = "# Doxology\n\n```keyflow\n| G |\n```\n\n## Notes\n\n- play it slower\n";
        let out = replace_fenced(body, CHART_FENCE, "| C | Am |\n");
        assert_eq!(
            out,
            "# Doxology\n\n```keyflow\n| C | Am |\n```\n\n## Notes\n\n- play it slower\n"
        );
        assert_eq!(
            extract_fenced(&out, CHART_FENCE).as_deref(),
            Some("| C | Am |\n")
        );
    }

    #[test]
    fn a_body_that_lost_its_fence_gets_one_back() {
        let out = replace_fenced("# Doxology\n", CHART_FENCE, "| G |\n");
        assert_eq!(out, "# Doxology\n\n```keyflow\n| G |\n```\n");
        assert_eq!(
            extract_fenced(&out, CHART_FENCE).as_deref(),
            Some("| G |\n")
        );
    }

    /// A fence somebody left unterminated owns the rest of the file, the
    /// way a markdown parser reads it — so a re-save repairs all of it
    /// rather than appending a second block below the wreckage.
    #[test]
    fn an_unterminated_fence_is_still_the_block() {
        let body = "# D\n\n```keyflow\n| G |\n";
        assert_eq!(
            extract_fenced(body, CHART_FENCE).as_deref(),
            Some("| G |\n")
        );
        let out = replace_fenced(body, CHART_FENCE, "| C |\n");
        assert_eq!(out, "# D\n\n```keyflow\n| C |\n```\n");
    }

    /// A fence tagged something else is somebody's code sample, not the
    /// chart, and must survive untouched.
    #[test]
    fn another_languages_fence_is_not_the_charts() {
        let body = "```rust\nfn main() {}\n```\n\n```keyflow\n| G |\n```\n";
        assert_eq!(
            extract_fenced(body, CHART_FENCE).as_deref(),
            Some("| G |\n")
        );
        let out = replace_fenced(body, CHART_FENCE, "| C |\n");
        assert!(out.contains("fn main() {}"), "{out}");
        assert!(out.contains("| C |"), "{out}");
    }
}
