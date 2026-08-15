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

use scripture_proto::{VerseBacklink, VerseRange};

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
                },
            });
        }
    }
    out
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
