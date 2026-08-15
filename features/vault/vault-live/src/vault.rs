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
        let loaded: Vec<LoadedEntry> = entries
            .par_iter()
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

fn load_entry(entry: &VaultEntry) -> Result<LoadedEntry, std::io::Error> {
    let raw = std::fs::read_to_string(&entry.abs_path)?;
    let mtime = std::fs::metadata(&entry.abs_path)
        .and_then(|m| m.modified())
        .unwrap_or(SystemTime::UNIX_EPOCH);
    let path = PathBuf::from(&entry.rel_path);
    let basename = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&entry.rel_path)
        .to_string();
    let folder = path
        .parent()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    Ok(match entry.kind {
        VaultEntryKind::Markdown => LoadedEntry::Page(VaultPage {
            rel_path: entry.rel_path.clone(),
            basename,
            folder,
            raw,
            mtime,
        }),
        VaultEntryKind::Base => {
            let parsed = crate::bases::parse(&raw).map_err(|e| format!("{e}"));
            LoadedEntry::Base(VaultBase {
                rel_path: entry.rel_path.clone(),
                basename,
                folder,
                raw,
                parsed,
                mtime,
            })
        }
    })
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
