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
        .filter_entry(|e| !is_skip(e.path(), root))
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
        let abs = entry.path().to_path_buf();
        let ext = abs
            .extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase);
        let kind = match ext.as_deref() {
            Some("md" | "markdown") => VaultEntryKind::Markdown,
            Some("base") => VaultEntryKind::Base,
            _ => continue,
        };
        let Ok(rel) = abs.strip_prefix(root) else {
            continue;
        };
        out.push(VaultEntry {
            abs_path: abs.clone(),
            rel_path: rel.to_string_lossy().replace('\\', "/"),
            kind,
        });
    }
    out
}

/// Filter applied during the walk so we don't descend into
/// directories we know to skip (`.git`, `.obsidian`, …) and
/// hidden files.
fn is_skip(path: &Path, root: &Path) -> bool {
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
    // Conventional vault-trash directory.
    if path.is_dir() && matches!(name, "trash" | "Trash") {
        return true;
    }
    false
}
