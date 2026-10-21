//! In-memory snapshot of a vault root.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rayon::prelude::*;
use thiserror::Error;

use crate::walker::{VaultEntry, VaultEntryKind, walk_vault};

/// One `.md` file from the vault, loaded into memory. Raw bytes
/// are preserved verbatim — block / heading / link indexes are
/// computed lazily on top.
///
/// This is the richer in-memory shape (carries `SystemTime`).
/// The wasm-clean wire shape is [`vault_proto::VaultPage`]; use
/// [`VaultPage::to_proto`] when handing a page to a parser that
/// also has to compile on wasm.
#[derive(Clone, Debug)]
pub struct VaultPage {
    /// Vault-relative, forward-slash separated, e.g.
    /// `Notes/2026/howdy.md`.
    pub rel_path: String,
    /// Filename without extension. Useful for `[[Wikilink]]`
    /// resolution.
    pub basename: String,
    /// Vault-relative parent directory; empty string at the root.
    pub folder: String,
    /// Raw file bytes at scan time. The editor sees this string;
    /// edits replace ranges within it.
    pub raw: String,
    /// `mtime` snapshot. The watcher uses it to detect external
    /// edits on reload.
    pub mtime: SystemTime,
}

impl VaultPage {
    /// Convert to the transport-shaped `vault_proto::VaultPage`.
    /// `SystemTime` → Unix milliseconds (`0` on conversion error
    /// — same convention the sync manifest uses).
    #[must_use]
    pub fn to_proto(&self) -> vault_proto::VaultPage {
        let mtime_ms = self
            .mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        vault_proto::VaultPage {
            rel_path: self.rel_path.clone(),
            basename: self.basename.clone(),
            folder: self.folder.clone(),
            raw: self.raw.clone(),
            mtime_ms,
        }
    }

    /// [`Self::to_proto`] by value: the strings move, nothing is cloned.
    /// For a caller that owns the page it converts (a scan over a
    /// freshly opened [`crate::Vault`]), where `to_proto` would copy
    /// every page's whole text to drop the original a line later.
    #[must_use]
    pub fn into_proto(self) -> vault_proto::VaultPage {
        let mtime_ms = self
            .mtime
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
            .unwrap_or(0);
        vault_proto::VaultPage {
            rel_path: self.rel_path,
            basename: self.basename,
            folder: self.folder,
            raw: self.raw,
            mtime_ms,
        }
    }
}

/// One `.base` file (Obsidian Bases). The vault loads + holds
/// the raw + parsed form; query execution lives in
/// `vault-obsidian::base_query` so we don't duplicate it
/// here.
#[derive(Clone, Debug)]
pub struct VaultBase {
    pub rel_path: String,
    pub basename: String,
    pub folder: String,
    pub raw: String,
    /// `Err` keeps the file visible (so editors can show a
    /// parse-error chip rather than hiding it).
    pub parsed: Result<crate::bases::ParsedBase, String>,
    pub mtime: SystemTime,
}

/// In-memory snapshot of a vault root.
#[derive(Clone, Debug)]
pub struct Vault {
    pub root: PathBuf,
    pub pages: Vec<VaultPage>,
    pub bases: Vec<VaultBase>,
    /// `.obsidian/types.json` — per-property type hints.
    /// Populated when the vault root has an Obsidian config
    /// directory; empty otherwise.
    pub property_types: PropertyTypes,
}

/// Property type registry parsed from `.obsidian/types.json`.
/// Maps frontmatter key → declared type ("text", "number",
/// "date", "checkbox", "multitext", "tags", "aliases").
#[derive(Clone, Debug, Default)]
pub struct PropertyTypes {
    pub map: std::collections::HashMap<String, String>,
}

impl PropertyTypes {
    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(String::as_str)
    }
}

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("io: {0}")]
    Io(String),
}

#[derive(Debug, Error)]
pub enum SaveError {
    #[error("io: {0}")]
    Io(String),
    #[error("page not found: {0}")]
    NotFound(String),
}

impl Vault {
    /// One-shot scan. Reads every `.md` and `.base` file under
    /// `root` into memory in parallel, then loads
    /// `.obsidian/types.json` if it exists. Dotfile /
    /// `.obsidian` / `.git` / `.trash` directories are skipped
    /// during the walk (see [`crate::walker::walk_vault`]).
    pub fn open(root: &Path) -> Result<Self, LoadError> {
        Self::from_entries(root, walk_vault(root))
    }

    /// Build a snapshot from a pre-resolved entry list. Useful
    /// when a caller wants their own walker semantics (e.g.
    /// `vault-obsidian` honoring `userIgnoreFilters` from
    /// `.obsidian/app.json`) while still going through this
    /// crate's parallel-load + indexing pipeline.
    ///
    /// `root` is the path entries are relative to;
    /// `.obsidian/types.json` is still loaded from
    /// `root/.obsidian/types.json` regardless of the entry
    /// list.
    pub fn from_entries(root: &Path, entries: Vec<VaultEntry>) -> Result<Self, LoadError> {
        // Read every file in parallel. I/O dominates the cold
        // scan (350ms single-threaded for an 8k-page vault);
        // rayon brings it under 100ms on a multi-core box.
        //
        // The entries are consumed: each one's `rel_path` moves into its
        // page rather than being cloned out of a list dropped a line
        // later.
        let loaded: Vec<LoadedEntry> = entries
            .into_par_iter()
            .filter_map(|entry| load_entry(entry).ok())
            .collect();
        let (pages, bases) = split_by_kind(loaded);
        let property_types = read_property_types(root);
        Ok(Self {
            root: root.to_path_buf(),
            pages,
            bases,
            property_types,
        })
    }

    /// Re-scan disk from scratch. Drops the current snapshot
    /// and rebuilds it. Cheap enough for the watcher to call
    /// on every debounced batch.
    pub fn reload(&mut self) -> Result<(), LoadError> {
        let fresh = Self::open(&self.root)?;
        self.pages = fresh.pages;
        self.bases = fresh.bases;
        self.property_types = fresh.property_types;
        Ok(())
    }

    /// Find a page by basename (case-insensitive). Used by
    /// `[[Wikilink]]` resolution.
    #[must_use]
    pub fn page_by_basename(&self, basename: &str) -> Option<&VaultPage> {
        self.pages
            .iter()
            .find(|p| p.basename.eq_ignore_ascii_case(basename))
    }

    /// Find a page by vault-relative path.
    #[must_use]
    pub fn page_by_rel_path(&self, rel_path: &str) -> Option<&VaultPage> {
        self.pages.iter().find(|p| p.rel_path == rel_path)
    }

    /// Find a `.base` file by basename.
    #[must_use]
    pub fn base_by_basename(&self, basename: &str) -> Option<&VaultBase> {
        self.bases
            .iter()
            .find(|b| b.basename.eq_ignore_ascii_case(basename))
    }
}

enum LoadedEntry {
    Page(VaultPage),
    Base(VaultBase),
}

fn split_by_kind(entries: Vec<LoadedEntry>) -> (Vec<VaultPage>, Vec<VaultBase>) {
    let mut pages = Vec::new();
    let mut bases = Vec::new();
    for e in entries {
        match e {
            LoadedEntry::Page(p) => pages.push(p),
            LoadedEntry::Base(b) => bases.push(b),
        }
    }
    (pages, bases)
}

fn load_entry(entry: VaultEntry) -> Result<LoadedEntry, std::io::Error> {
    let VaultEntry {
        abs_path,
        rel_path,
        kind,
    } = entry;
    // One open for the text and its mtime, where `read_to_string` and
    // `metadata` each resolved the path again.
    let (raw, mtime) = read_with_mtime(&abs_path)?;
    let (basename, folder) = name_parts(&rel_path);
    let (basename, folder) = (basename.to_owned(), folder.to_owned());
    Ok(match kind {
        VaultEntryKind::Markdown => LoadedEntry::Page(VaultPage {
            rel_path,
            basename,
            folder,
            raw,
            mtime,
        }),
        VaultEntryKind::Base => {
            let parsed = crate::bases::parse(&raw).map_err(|e| format!("{e}"));
            LoadedEntry::Base(VaultBase {
                rel_path,
                basename,
                folder,
                raw,
                parsed,
                mtime,
            })
        }
    })
}

/// Read a file's text and its mtime through one open handle. The
/// buffer is sized from that handle's `fstat`, so the read does not
/// grow it. An unreadable mtime is the epoch, as before.
fn read_with_mtime(abs: &Path) -> Result<(String, SystemTime), std::io::Error> {
    use std::io::Read as _;
    let mut file = std::fs::File::open(abs)?;
    let meta = file.metadata().ok();
    let mtime = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let len = meta.map_or(0, |m| usize::try_from(m.len()).unwrap_or(0));
    let mut raw = String::with_capacity(len);
    file.read_to_string(&mut raw)?;
    Ok((raw, mtime))
}

/// A vault-relative, `/`-separated path's file stem and folder, as
/// borrowed slices of it: what `Path::file_stem` and `Path::parent`
/// returned, without building a `PathBuf` of the path to ask.
///
/// The stem follows `file_stem`'s rule (a leading dot is part of the
/// name, `..` is its own stem); a path with no folder has `""`.
fn name_parts(rel: &str) -> (&str, &str) {
    let (folder, name) = rel.rsplit_once('/').unwrap_or(("", rel));
    if name.is_empty() {
        return (rel, folder);
    }
    let stem = if name == ".." {
        name
    } else {
        match name.rsplit_once('.') {
            Some(("", _)) | None => name,
            Some((before, _)) => before,
        }
    };
    (stem, folder)
}

/// Read `.obsidian/types.json` if present. Empty map otherwise.
fn read_property_types(root: &Path) -> PropertyTypes {
    #[derive(serde::Deserialize)]
    struct Wrapper {
        #[serde(default)]
        types: std::collections::HashMap<String, String>,
    }
    let path = root.join(".obsidian").join("types.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return PropertyTypes::default();
    };
    let Ok(parsed) = serde_json::from_str::<Wrapper>(&raw) else {
        tracing::warn!(path = %path.display(), "types.json failed to parse");
        return PropertyTypes::default();
    };
    PropertyTypes { map: parsed.types }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `name_parts` answers what `Path::file_stem` and `Path::parent`
    /// answered for every shape of vault-relative path the walker
    /// produces, without the `PathBuf`.
    #[test]
    fn name_parts_agrees_with_path_file_stem_and_parent() {
        for rel in [
            "note.md",
            "Notes/2026/howdy.md",
            "a/b.c.md",
            ".hidden.md",
            "dir/.md",
            "dir/x.",
            "dir/no-extension",
            "Views/Tasks.base",
            "a/b/c/d/e.markdown",
        ] {
            let path = Path::new(rel);
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or(rel);
            let folder = path
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            assert_eq!(name_parts(rel), (stem, folder.as_str()), "{rel}");
        }
    }

    /// The single-handle read returns the text and the same mtime
    /// `metadata` reports.
    #[test]
    fn read_with_mtime_returns_text_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("n.md");
        std::fs::write(&file, "---\ntype: note\n---\nbody\n").unwrap();
        let (raw, mtime) = read_with_mtime(&file).unwrap();
        assert_eq!(raw, "---\ntype: note\n---\nbody\n");
        assert_eq!(mtime, std::fs::metadata(&file).unwrap().modified().unwrap());
        assert!(read_with_mtime(&dir.path().join("missing.md")).is_err());
    }

    /// `open` still loads pages and bases with their names and folders,
    /// now that the entries are moved into them instead of cloned.
    #[test]
    fn open_moves_each_entry_into_its_page() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("Notes/2026")).unwrap();
        std::fs::write(dir.path().join("Notes/2026/howdy.md"), "hi").unwrap();
        std::fs::write(dir.path().join("Top.MD"), "upper").unwrap();
        std::fs::write(dir.path().join("pic.png"), [0u8; 4]).unwrap();
        let vault = Vault::open(dir.path()).unwrap();
        let mut pages: Vec<_> = vault
            .pages
            .iter()
            .map(|p| {
                (
                    p.rel_path.as_str(),
                    p.basename.as_str(),
                    p.folder.as_str(),
                    p.raw.as_str(),
                )
            })
            .collect();
        pages.sort_unstable();
        assert_eq!(
            pages,
            vec![
                ("Notes/2026/howdy.md", "howdy", "Notes/2026", "hi"),
                ("Top.MD", "Top", "", "upper"),
            ]
        );
    }
}
