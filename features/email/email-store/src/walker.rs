//! Maildir walker. Yields `(folder, path, bytes, flags)` tuples
//! suitable for feeding into `Store::upsert_envelope`. We don't
//! parse here — that's the caller's job — so the walker stays
//! cheap and reusable.
//!
//! Layout assumed (matches `email-maildir::Backend`):
//! - `<root>/{cur,new,tmp}/`            → INBOX
//! - `<root>/.<folder-name>/{...}`      → sub-mailboxes
//! - hierarchy is encoded with `.` in the dir name
//!   (`.Lists.rust-users`)

use std::path::{Path, PathBuf};

/// One on-disk message. `flags` is the Maildir info section
/// after `:2,` (`Seen`, `Replied`, etc, as single chars).
#[derive(Debug, Clone)]
pub struct MailEntry {
    /// Folder id (wire name): `"INBOX"`, `"Sent"`, `"Lists.rust-users"`.
    pub folder: String,
    /// Path relative to the account root.
    pub rel_path: PathBuf,
    /// Absolute path on disk.
    pub abs_path: PathBuf,
    /// Maildir flag chars (e.g. `['S', 'R']`).
    pub flags: Vec<char>,
}

/// Iterator over every message under one folder maildir.
/// `folder_path` is the directory containing `cur/new/tmp` (so
/// for INBOX it's the account root itself).
pub fn walk_folder<'a>(
    account_root: &'a Path,
    folder_path: &'a Path,
    folder_id: &'a str,
) -> impl Iterator<Item = MailEntry> + 'a {
    ["cur", "new"]
        .into_iter()
        .flat_map(move |sub| {
            let dir = folder_path.join(sub);
            let entries = std::fs::read_dir(&dir).ok();
            entries
                .into_iter()
                .flatten()
                .filter_map(std::result::Result::ok)
        })
        .filter_map(move |entry| {
            let abs = entry.path();
            if !abs.is_file() {
                return None;
            }
            let rel = abs.strip_prefix(account_root).ok()?.to_path_buf();
            let name = entry.file_name().to_string_lossy().into_owned();
            let flags = name
                .split(":2,")
                .nth(1)
                .map(|info| info.chars().collect::<Vec<_>>())
                .unwrap_or_default();
            Some(MailEntry {
                folder: folder_id.to_string(),
                rel_path: rel,
                abs_path: abs,
                flags,
            })
        })
}

/// Walk every folder under `account_root` — INBOX (the root
/// itself) plus every Maildir++ sibling whose name starts with
/// `.`. Returns an owned `Vec` because the iterator chain would
/// otherwise have to hold borrows across multiple `read_dir`
/// calls.
#[must_use]
pub fn walk_account(account_root: &Path) -> Vec<MailEntry> {
    let mut out = Vec::new();

    // INBOX = the root itself, if it actually is a maildir.
    if is_maildir(account_root) {
        out.extend(walk_folder(account_root, account_root, "INBOX"));
    }

    // Sub-mailboxes: `.Foo`, `.Lists.rust-users`, …
    let Ok(read) = std::fs::read_dir(account_root) else {
        return out;
    };
    for entry in read.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = match entry.file_name().into_string() {
            Ok(s) => s,
            Err(_) => continue,
        };
        let Some(folder_id) = name.strip_prefix('.') else {
            continue;
        };
        if folder_id.is_empty() {
            continue;
        }
        if !is_maildir(&path) {
            continue;
        }
        out.extend(walk_folder(account_root, &path, folder_id));
    }

    out
}

fn is_maildir(path: &Path) -> bool {
    path.join("cur").is_dir() && path.join("new").is_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for sub in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(root.join(sub)).unwrap();
        }
        for sub in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(root.join(".Sent").join(sub)).unwrap();
        }
        std::fs::write(root.join("new/1.M.host"), "msg1").unwrap();
        std::fs::write(root.join("cur/2.M.host:2,SR"), "msg2").unwrap();
        std::fs::write(root.join(".Sent/cur/3.M.host:2,S"), "msg3").unwrap();
        dir
    }

    #[test]
    fn walks_inbox_and_sent() {
        let d = fixture();
        let entries = walk_account(d.path());
        let folders: Vec<_> = entries.iter().map(|e| e.folder.as_str()).collect();
        assert!(folders.contains(&"INBOX"));
        assert!(folders.contains(&"Sent"));
        assert_eq!(entries.len(), 3);
    }

    #[test]
    fn flag_chars_parsed_from_info_section() {
        let d = fixture();
        let entries = walk_account(d.path());
        let with_flags = entries
            .iter()
            .find(|e| e.flags.contains(&'R'))
            .expect("expected to find the :2,SR entry");
        assert!(with_flags.flags.contains(&'S'));
        assert!(with_flags.flags.contains(&'R'));
    }

    #[test]
    fn skips_non_maildir_dirs() {
        let d = fixture();
        // Dir starting with `.` but missing cur/new should be
        // ignored.
        std::fs::create_dir_all(d.path().join(".not-a-maildir")).unwrap();
        let entries = walk_account(d.path());
        assert!(!entries.iter().any(|e| e.folder == "not-a-maildir"));
    }
}
