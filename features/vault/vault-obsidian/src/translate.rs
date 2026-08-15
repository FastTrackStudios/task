//! The "open an Obsidian vault as a Task vault" tool.
//!
//! Bridges this crate's Obsidian-aware walker
//! ([`crate::walker::walk_vault`]) — which respects
//! `.obsidian/app.json`'s `userIgnoreFilters` and the
//! `.obsidian` / `.trash` / `.git` skip set — to the canonical
//! [`vault_live::Vault`] / [`vault_live::Backend`] in the sibling crate.
//!
//! The point: a directory that an Obsidian user opens with their
//! editor can be opened by Task with the same semantics, and
//! served over the [`vault_proto::VaultSync`] wire trait to
//! desktop / web clients without them caring that the source
//! was Obsidian.
//!
//! Use either:
//! - [`open_as_vault`] — load an in-memory `vault_live::Vault`
//!   snapshot. Cheapest path; right for read-only inspection,
//!   editor opens, etc.
//! - [`open_as_backend`] — register the same directory under a
//!   `vault_id` on a new [`vault_live::Backend`] so it can be
//!   mounted on a vox router. Use this when you want the
//!   directory exposed via RPC.

use std::collections::HashMap;
use std::path::Path;

use crate::config::read_obsidian_config;
use crate::walker::{VaultEntry as ObsidianEntry, VaultEntryKind as ObsidianKind, walk_vault};

/// Open an Obsidian-shaped directory as a [`vault_live::Vault`]
/// snapshot. Reads `.obsidian/app.json` to honor
/// `userIgnoreFilters`; skips `.obsidian`, `.trash`, `.git`,
/// dotfiles, and unsupported file types in the same way
/// Obsidian itself does.
///
/// Attachments + canvas files in the source vault are *not*
/// loaded — [`vault_live::Vault`] only models `.md` pages and
/// `.base` files. They remain on disk and round-trip via the
/// byte-level [`vault_proto::VaultSync::get_file`] /
/// `put_file` path.
pub fn open_as_vault(root: &Path) -> Result<vault_live::Vault, vault_live::LoadError> {
    let cfg = read_obsidian_config(root);
    let obsidian_entries = walk_vault(root, &cfg);
    let entries = obsidian_entries
        .into_iter()
        .filter_map(translate_entry)
        .collect();
    vault_live::Vault::from_entries(root, entries)
}

/// Open an Obsidian-shaped directory and register it on a new
/// single-vault [`vault_live::Backend`] under `vault_id`. Use this
/// when you want to mount the directory via
/// `vault_proto::serve(backend)` on a vox router.
///
/// The Backend is in `Explicit` layout with just this one entry
/// — call [`vault_live::Backend::start_watcher`] separately to
/// receive external-edit events from Obsidian / vim / git.
pub fn open_as_backend(
    vault_id: impl Into<String>,
    root: &Path,
) -> Result<vault_live::Backend, std::io::Error> {
    let mut roots = HashMap::with_capacity(1);
    roots.insert(vault_id.into(), root.to_path_buf());
    std::fs::create_dir_all(root)?;
    Ok(vault_live::Backend::with_roots(roots))
}

/// Translate one of this crate's [`ObsidianEntry`]s into the
/// `vault` crate's [`vault_live::VaultEntry`] shape. Canvas +
/// attachment entries are dropped — `vault_live::Vault` doesn't
/// model them, and they're available byte-for-byte through
/// `VaultSync::get_file` anyway.
fn translate_entry(entry: ObsidianEntry) -> Option<vault_live::VaultEntry> {
    let kind = match entry.kind {
        ObsidianKind::Markdown => vault_live::VaultEntryKind::Markdown,
        ObsidianKind::Base => vault_live::VaultEntryKind::Base,
        ObsidianKind::Canvas | ObsidianKind::Attachment => return None,
    };
    Some(vault_live::VaultEntry {
        abs_path: entry.abs_path,
        rel_path: entry.rel_path,
        kind,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path, contents: &str) {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, contents).unwrap();
    }

    #[test]
    fn open_as_vault_honors_obsidian_skip_set() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("Note.md"), "# kept");
        touch(&root.join(".obsidian/app.json"), "{}");
        touch(&root.join(".trash/dropped.md"), "# dropped");
        touch(&root.join(".git/HEAD"), "ref: refs/heads/main");

        let v = open_as_vault(root).unwrap();
        let paths: Vec<_> = v.pages.iter().map(|p| p.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["Note.md"]);
    }

    #[test]
    fn open_as_vault_honors_user_ignore_filters() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("Real.md"), "# real");
        touch(&root.join("Drafts/wip.md"), "# wip");
        touch(
            &root.join(".obsidian/app.json"),
            r#"{"userIgnoreFilters":["Drafts/"]}"#,
        );

        let v = open_as_vault(root).unwrap();
        let paths: Vec<_> = v.pages.iter().map(|p| p.rel_path.as_str()).collect();
        assert_eq!(paths, vec!["Real.md"]);
    }

    #[test]
    fn open_as_vault_picks_up_base_files() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("Notes.md"), "");
        touch(
            &root.join("Tasks.base"),
            "views:\n  - name: All\n    type: table\n",
        );

        let v = open_as_vault(root).unwrap();
        assert_eq!(v.pages.len(), 1);
        assert_eq!(v.bases.len(), 1);
        assert_eq!(v.bases[0].basename, "Tasks");
    }

    #[test]
    fn open_as_backend_registers_single_explicit_vault() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        touch(&root.join("a.md"), "x");

        let backend = open_as_backend("personal", root).unwrap();
        // The registered id resolves; an unknown id should not
        // — confirms we're in `Explicit` layout, not the
        // server's `UnderParent`.
        use vault_proto::VaultSync;
        assert_eq!(backend.manifest("personal").unwrap().files.len(), 1);
        assert!(backend.manifest("other").is_err());
    }
}
