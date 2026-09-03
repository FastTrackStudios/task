//! Build the verse↔notes reverse index by scanning a vault.
//!
//! This is the Obsidian-beating trick: instead of one file per verse
//! (the 31k-file vault), we keep verses as stable [`VerseId`] keys and
//! scan the user's notes for `[[John 3:16]]`-style links, accumulating
//! the ranges each note references. A note that links a *span*
//! (`[[John 3:16-20]]`) backlinks every verse it covers — overlap is
//! computed at query time (see `Store::chapter_backlinks`).
//!
//! The vault here is small (hundreds of notes), so a full walk per query
//! is fine; a cached/watched index is a later optimization.

use std::path::Path;

use scripture_proto::{Book, VerseBacklink, VerseRange};

use crate::refs::extract_verse_refs;

/// A referenced range paired with the note that referenced it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeBacklink {
    pub range: VerseRange,
    pub link: VerseBacklink,
}

/// Scan every markdown note under `vault_root` and return one
/// [`RangeBacklink`] per (note, distinct range). Order: by note path,
/// then document order within a note.
#[must_use]
pub fn scan_vault(vault_root: &Path) -> Vec<RangeBacklink> {
    let mut files = Vec::new();
    collect_markdown(vault_root, &mut files);
    files.sort();

    let mut out = Vec::new();
    for path in files {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let rel = path.strip_prefix(vault_root).unwrap_or(&path);
        let note_path = rel.to_string_lossy().into_owned();
        let note_title = title_of(&text, &path);

        // One backlink per distinct range per note (first line wins).
        let mut seen = std::collections::BTreeSet::new();
        for hit in extract_verse_refs(&text) {
            if !seen.insert((hit.range.start, hit.range.end)) {
                continue;
            }
            out.push(RangeBacklink {
                range: hit.range,
                link: VerseBacklink {
                    note_path: note_path.clone(),
                    note_title: note_title.clone(),
                    excerpt: hit.excerpt,
                    source_kind: String::new(),
                    secs: 0,
                },
            });
        }
    }
    out
}

/// A media source's reference: what it points at (a verse range, or a
/// whole chapter) and the backlink row to show for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaBacklink {
    /// `Some` for a verse or range; `None` when the whole chapter was
    /// named (`Rom.8`).
    pub range: Option<VerseRange>,
    pub book: Book,
    pub chapter: u16,
    pub link: VerseBacklink,
}

/// Every `sermon:` / `song:` / `video:` → `verse:` link in the typed-link
/// store, as backlink rows. Titles come from the resource manifests
/// under `resources_root` (`sermons/**/<slug>.md`, `songs/…`); a source
/// with no manifest shows its slug.
#[must_use]
pub fn scan_media_links(links: &links::Store, resources_root: &Path) -> Vec<MediaBacklink> {
    use links::{LinksService as _, NodeKind};

    let Ok(all) = links.graph(links::Confidence::Speculative, true) else {
        return Vec::new();
    };
    let mut titles: Option<std::collections::HashMap<String, String>> = None;
    let mut out = Vec::new();
    for l in all {
        let kind = match l.source.kind {
            NodeKind::Sermon => "sermon",
            NodeKind::Song => "song",
            NodeKind::Video => "video",
            _ => continue,
        };
        if l.target.kind != NodeKind::Verse {
            continue;
        }
        let Some((book, chapter, range)) = parse_target(&l.target.id) else {
            continue;
        };
        // `t:<secs>` or a clip `t:<start>-<end>`: the moment to open at.
        let secs = l
            .source
            .anchor
            .strip_prefix("t:")
            .and_then(|t| t.split('-').next())
            .and_then(|t| t.parse().ok())
            .unwrap_or(0);
        let titles = titles.get_or_insert_with(|| manifest_titles(resources_root));
        let title = titles
            .get(&format!("{kind}:{}", l.source.id))
            .cloned()
            .unwrap_or_else(|| l.source.id.clone());
        // The sync writes `Title · MM:SS — <what was said>`; keep the
        // spoken part as the excerpt.
        let excerpt = l
            .note
            .split_once(" — ")
            .map_or(l.note.as_str(), |(_, said)| said)
            .to_string();
        out.push(MediaBacklink {
            range,
            book,
            chapter,
            link: VerseBacklink {
                note_path: format!("{kind}:{}", l.source.id),
                note_title: title,
                excerpt,
                source_kind: kind.to_string(),
                secs,
            },
        });
    }
    out
}

/// A `verse:` node id: `John.3.16`, `John.3.16-John.3.18`, or the
/// chapter-only `John.3`. Returns `(book, chapter, range)`.
fn parse_target(id: &str) -> Option<(Book, u16, Option<VerseRange>)> {
    if let Ok(range) = VerseRange::parse(id) {
        return Some((range.start.book, range.start.chapter, Some(range)));
    }
    let (book, chapter) = id.rsplit_once('.')?;
    let book = Book::lookup(book)?;
    let chapter: u16 = chapter.parse().ok()?;
    (chapter > 0).then_some((book, chapter, None))
}

/// `kind:slug → title` for every resource manifest under the root
/// (`sermons/**/*.md` → `sermon:<slug>`, `songs/…` → `song:`).
fn manifest_titles(resources_root: &Path) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for (dir, kind) in [
        ("sermons", "sermon"),
        ("songs", "song"),
        ("videos", "video"),
    ] {
        let mut files = Vec::new();
        collect_markdown(&resources_root.join(dir), &mut files);
        for path in files {
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let field = |key: &str| {
                text.lines()
                    .take_while(|l| !(l.trim() == "---" && !text.starts_with(l)))
                    .find_map(|l| l.strip_prefix(key))
                    .map(|v| v.trim().trim_matches(['"', '\'']).to_string())
            };
            let slug = field("slug:").unwrap_or_else(|| {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            });
            let title = field("title:").unwrap_or_else(|| title_of(&text, &path));
            map.insert(format!("{kind}:{slug}"), title);
        }
    }
    map
}

/// Recursively collect `*.md` paths, skipping hidden dirs (`.obsidian`,
/// `.git`, …).
fn collect_markdown(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.filter_map(Result::ok) {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') {
            continue;
        }
        if path.is_dir() {
            collect_markdown(&path, out);
        } else if path
            .extension()
            .is_some_and(|x| x.eq_ignore_ascii_case("md"))
        {
            out.push(path);
        }
    }
}

/// First `# ` heading, else the file stem.
fn title_of(text: &str, path: &Path) -> String {
    for line in text.lines() {
        if let Some(h) = line.trim().strip_prefix("# ") {
            return h.trim().to_string();
        }
    }
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use scripture_proto::VerseId;

    #[test]
    fn scans_notes_into_range_backlinks() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("Journal")).unwrap();
        std::fs::write(
            dir.path().join("grace.md"),
            "# On Grace\nPaul leans on [[John 3:16]] here.\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("Journal/2026-06-16.md"),
            "The whole discourse [[John 3:16-21]] matters; also [[Project Plan]].\n",
        )
        .unwrap();

        let index = scan_vault(dir.path());
        assert_eq!(index.len(), 2, "two distinct range references");
        // The span covers verse 18 even though no note names it directly.
        let covers_18 = index
            .iter()
            .filter(|rb| rb.range.contains(VerseId::parse("John 3:18").unwrap()))
            .count();
        assert_eq!(covers_18, 1);
        assert!(index.iter().any(|rb| rb.link.note_title == "On Grace"));
    }
}
