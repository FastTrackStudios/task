//! Walk a vault root, collecting `.md` files. Skips dotfile
//! directories (`.git`, `.obsidian`, `.trash`, etc.) and any
//! filename starting with `.`.
//!
//! Lighter than `vault-obsidian::walker` — we only care about
//! pages here; Bases / canvas / attachments are someone else's
//! problem (consume `vault-obsidian` for that). The intent is
//! that this crate stays small and editor-focused.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

/// What an entry is. Drives whether the loader treats it as
/// markdown content or an Obsidian `.base` view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultEntryKind {
    Markdown,
    Base,
}

/// One file the walker has decided is part of the vault.
#[derive(Clone, Debug)]
pub struct VaultEntry {
    /// Full disk path.
    pub abs_path: PathBuf,
    /// Vault-relative path with extension. Forward-slash
    /// separated (`Notes/2026/howdy.md`).
    pub rel_path: String,
    pub kind: VaultEntryKind,
}

/// Synchronous walk. Returns entries in walkdir's default order
/// (folders before children, lexicographic within a folder).
pub fn walk_vault(root: &Path) -> Vec<VaultEntry> {
    let mut out = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|e| !is_skip(e, root))
    {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                tracing::warn!(?e, "vault walk error — skipping");
                continue;
            }
        };
        if entry.file_type().is_dir() {
            continue;
        }
        // Classified on the borrowed path: every file in the vault
        // (attachments included) passes through here and only pages
        // are kept, so nothing is allocated until one is.
        let Some(kind) = classify(entry.path()) else {
            continue;
        };
        let Ok(rel) = entry.path().strip_prefix(root) else {
            continue;
        };
        let rel_path = rel_string(rel);
        out.push(VaultEntry {
            // `into_path` moves walkdir's own buffer out of the entry,
            // where this used to copy it and then clone the copy.
            abs_path: entry.into_path(),
            rel_path,
            kind,
        });
    }
    out
}

/// A page's kind by its extension, case-insensitively and without
/// lowering a copy of it. `None` for every file the vault does not load.
fn classify(path: &Path) -> Option<VaultEntryKind> {
    let ext = path.extension()?.to_str()?;
    if ext.eq_ignore_ascii_case("md") || ext.eq_ignore_ascii_case("markdown") {
        Some(VaultEntryKind::Markdown)
    } else if ext.eq_ignore_ascii_case("base") {
        Some(VaultEntryKind::Base)
    } else {
        None
    }
}

/// A vault-relative path as the forward-slash string the vault keys on,
/// in one allocation: the separator rewrite only runs when there is a
/// backslash to rewrite (never, on Unix).
#[must_use]
pub fn rel_string(rel: &Path) -> String {
    let s = rel.to_string_lossy();
    if s.contains('\\') {
        s.replace('\\', "/")
    } else {
        s.into_owned()
    }
}

/// Filter applied during the walk so we don't descend into
/// directories we know to skip (`.git`, `.obsidian`, …), hidden
/// files, or a **nested shelf**.
fn is_skip(entry: &walkdir::DirEntry, root: &Path) -> bool {
    let path = entry.path();
    if path == root {
        return false;
    }
    let name = match path.file_name().and_then(|s| s.to_str()) {
        Some(n) => n,
        None => return false,
    };
    if name.starts_with('.') {
        return true;
    }
    // Only directories are pruned below, and walkdir already knows
    // which entries are directories: asking it is free, where
    // `path.is_dir()` was a `stat` per check (three per entry).
    if !entry.file_type().is_dir() {
        return false;
    }
    // A node-private directory is never a page source, whatever it is
    // called (`vault.private.node-only`).
    if files_proto::consts::is_private_dir(path) {
        return true;
    }
    // Conventional vault-trash directory.
    if matches!(name, "trash" | "Trash") {
        return true;
    }
    // A directory that declares itself a root of its own belongs to
    // that root and not to this one — see `crate::shelf_boundary` for
    // why a file in two roots at once is the worst outcome available.
    // `filter_entry` does not descend into a rejected directory, so one
    // check at the boundary prunes the whole subtree.
    if crate::shelf_boundary::is_nested_shelf(root, path) {
        return true;
    }
    false
}
