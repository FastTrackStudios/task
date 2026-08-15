//! Write-side mutations against a `Vault`.
//!
//! Mirrors the Obsidian CLI surface: `create`, `append`, `prepend`,
//! `delete`, `move`/`rename`, `property:set`, `property:remove`.
//!
//! All ops route through the `SelfWriteGuard` so the watcher can
//! filter out echo events from our own writes, and they update the
//! in-memory `Vault` snapshot so callers don't have to re-`open()`.

use std::path::{Component, Path, PathBuf};

use crate::obsidian_parse::{
    FrontmatterEntry, parse_frontmatter, parse_page, serialize_frontmatter,
};

use crate::guard::SelfWriteGuard;
use crate::vault::{Vault, VaultPage};

#[derive(Debug, thiserror::Error)]
pub enum MutateError {
    #[error("io: {0}")]
    Io(String),
    #[error("page not found: {0}")]
    NotFound(String),
    #[error("already exists: {0}")]
    AlreadyExists(String),
    #[error("invalid path: {0}")]
    InvalidPath(String),
}

// ── Path helpers ────────────────────────────────────────────────────

/// Reject absolute paths and any path containing `..` components, so
/// `rel_path` can't escape the vault root.
fn validate_rel(rel: &str) -> Result<(), MutateError> {
    if rel.is_empty() {
        return Err(MutateError::InvalidPath("empty".into()));
    }
    if rel.starts_with('/') || rel.starts_with('\\') {
        return Err(MutateError::InvalidPath(rel.into()));
    }
    let p = Path::new(rel);
    if p.is_absolute() {
        return Err(MutateError::InvalidPath(rel.into()));
    }
    for c in p.components() {
        match c {
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(MutateError::InvalidPath(rel.into()));
            }
            _ => {}
        }
    }
    Ok(())
}

fn abs_path(vault: &Vault, rel: &str) -> PathBuf {
    vault.root.join(rel)
}

fn split_rel(rel: &str) -> (String, String) {
    let p = PathBuf::from(rel);
    let folder = p
        .parent()
        .map(|s| s.to_string_lossy().replace('\\', "/"))
        .unwrap_or_default();
    let basename = p
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_string();
    (folder, basename)
}

fn mtime_of(abs: &Path) -> std::time::SystemTime {
    std::fs::metadata(abs)
        .and_then(|m| m.modified())
        .unwrap_or_else(|_| std::time::SystemTime::now())
}

// ── create / delete / move ──────────────────────────────────────────

pub fn create_page(
    vault: &mut Vault,
    rel_path: &str,
    frontmatter: &[FrontmatterEntry],
    body: &str,
    guard: &SelfWriteGuard,
) -> Result<(), MutateError> {
    validate_rel(rel_path)?;
    let abs = abs_path(vault, rel_path);
    if abs.exists() {
        return Err(MutateError::AlreadyExists(rel_path.into()));
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| MutateError::Io(e.to_string()))?;
    }
    let raw = format!("{}{}", serialize_frontmatter(frontmatter), body);

    guard.mark(&abs);
    std::fs::write(&abs, &raw).map_err(|e| MutateError::Io(e.to_string()))?;
    guard.mark(&abs);

    let parsed = parse_page(&raw);
    let (folder, basename) = split_rel(rel_path);
    let mtime = mtime_of(&abs);
    let page = VaultPage {
        rel_path: rel_path.to_string(),
        basename,
        folder,
        raw,
        parsed,
        mtime,
    };
    // Replace if somehow already present in snapshot; else push.
    if let Some(existing) = vault.pages.iter_mut().find(|p| p.rel_path == rel_path) {
        *existing = page;
    } else {
        vault.pages.push(page);
    }
    Ok(())
}

pub fn delete_page(
    vault: &mut Vault,
    rel_path: &str,
    guard: &SelfWriteGuard,
) -> Result<(), MutateError> {
    validate_rel(rel_path)?;
    let abs = abs_path(vault, rel_path);
    if !abs.exists() {
        return Err(MutateError::NotFound(rel_path.into()));
    }
    guard.mark(&abs);
    std::fs::remove_file(&abs).map_err(|e| MutateError::Io(e.to_string()))?;
    guard.mark(&abs);
    vault.pages.retain(|p| p.rel_path != rel_path);
    Ok(())
}

pub fn move_page(
    vault: &mut Vault,
    from_rel: &str,
    to_rel: &str,
    guard: &SelfWriteGuard,
) -> Result<(), MutateError> {
    validate_rel(from_rel)?;
    validate_rel(to_rel)?;
    let from_abs = abs_path(vault, from_rel);
    let to_abs = abs_path(vault, to_rel);
    if !from_abs.exists() {
        return Err(MutateError::NotFound(from_rel.into()));
    }
    if to_abs.exists() {
        return Err(MutateError::AlreadyExists(to_rel.into()));
    }
    if let Some(parent) = to_abs.parent() {
        std::fs::create_dir_all(parent).map_err(|e| MutateError::Io(e.to_string()))?;
    }
    guard.mark(&from_abs);
    guard.mark(&to_abs);
    std::fs::rename(&from_abs, &to_abs).map_err(|e| MutateError::Io(e.to_string()))?;
    guard.mark(&from_abs);
    guard.mark(&to_abs);

    let (folder, basename) = split_rel(to_rel);
    let mtime = mtime_of(&to_abs);
    if let Some(page) = vault.pages.iter_mut().find(|p| p.rel_path == from_rel) {
        page.rel_path = to_rel.to_string();
        page.folder = folder;
        page.basename = basename;
        page.mtime = mtime;
    }
    Ok(())
}

// ── append / prepend ────────────────────────────────────────────────

pub fn append_to_page(
    vault: &mut Vault,
    rel_path: &str,
    text: &str,
    inline: bool,
    guard: &SelfWriteGuard,
) -> Result<(), MutateError> {
    validate_rel(rel_path)?;
    let current = vault
        .page(rel_path)
        .ok_or_else(|| MutateError::NotFound(rel_path.into()))?
        .raw
        .clone();
    let sep = if inline || current.is_empty() || current.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let new_raw = format!("{current}{sep}{text}");
    vault
        .save_page(rel_path, &new_raw, guard)
        .map_err(save_to_mutate)
}

pub fn prepend_to_page(
    vault: &mut Vault,
    rel_path: &str,
    text: &str,
    inline: bool,
    guard: &SelfWriteGuard,
) -> Result<(), MutateError> {
    validate_rel(rel_path)?;
    let current = vault
        .page(rel_path)
        .ok_or_else(|| MutateError::NotFound(rel_path.into()))?
        .raw
        .clone();
    let (_, body_offset) = parse_frontmatter(&current);
    let head = &current[..body_offset];
    let body = &current[body_offset..];

    let sep = if inline || text.ends_with('\n') {
        ""
    } else {
        "\n"
    };
    let new_raw = format!("{head}{text}{sep}{body}");
    vault
        .save_page(rel_path, &new_raw, guard)
        .map_err(save_to_mutate)
}

// ── property:set / property:remove ──────────────────────────────────

pub fn set_property(
    vault: &mut Vault,
    rel_path: &str,
    key: &str,
    value: serde_json::Value,
    guard: &SelfWriteGuard,
) -> Result<(), MutateError> {
    validate_rel(rel_path)?;
    let current = vault
        .page(rel_path)
        .ok_or_else(|| MutateError::NotFound(rel_path.into()))?
        .raw
        .clone();
    let (mut fm, body_offset) = parse_frontmatter(&current);
    let body = &current[body_offset..];

    if let Some(existing) = fm.iter_mut().find(|e| e.key == key) {
        existing.value = value;
    } else {
        fm.push(FrontmatterEntry {
            key: key.to_string(),
            value,
        });
    }
    let new_raw = format!("{}{}", serialize_frontmatter(&fm), body);
    vault
        .save_page(rel_path, &new_raw, guard)
        .map_err(save_to_mutate)
}

pub fn remove_property(
    vault: &mut Vault,
    rel_path: &str,
    key: &str,
    guard: &SelfWriteGuard,
) -> Result<(), MutateError> {
    validate_rel(rel_path)?;
    let current = vault
        .page(rel_path)
        .ok_or_else(|| MutateError::NotFound(rel_path.into()))?
        .raw
        .clone();
    let (mut fm, body_offset) = parse_frontmatter(&current);
    let body = &current[body_offset..];

    let before = fm.len();
    fm.retain(|e| e.key != key);
    if fm.len() == before {
        // Absent — no-op, but keep file untouched to avoid spurious mtime churn.
        return Ok(());
    }
    let new_raw = format!("{}{}", serialize_frontmatter(&fm), body);
    vault
        .save_page(rel_path, &new_raw, guard)
        .map_err(save_to_mutate)
}

fn save_to_mutate(e: crate::vault::SaveError) -> MutateError {
    match e {
        crate::vault::SaveError::Io(s) => MutateError::Io(s),
        crate::vault::SaveError::NotFound(s) => MutateError::NotFound(s),
    }
}

// ── tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    fn open_empty() -> (tempfile::TempDir, Vault, SelfWriteGuard) {
        let dir = tempfile::tempdir().unwrap();
        // Need a non-empty vault for `Vault::open` to be happy — touch a placeholder.
        touch(&dir.path().join(".obsidian/app.json"), "{}");
        let v = Vault::open(dir.path()).unwrap();
        let g = SelfWriteGuard::new();
        (dir, v, g)
    }

    #[test]
    fn create_writes_file_and_updates_snapshot() {
        let (dir, mut v, g) = open_empty();
        let fm = vec![FrontmatterEntry {
            key: "tags".into(),
            value: serde_json::json!(["x"]),
        }];
        create_page(&mut v, "Sub/New.md", &fm, "hello\n", &g).unwrap();
        let on_disk = fs::read_to_string(dir.path().join("Sub/New.md")).unwrap();
        assert!(on_disk.starts_with("---\n"));
        assert!(on_disk.ends_with("hello\n"));
        assert!(v.page("Sub/New.md").is_some());
        // Duplicate create rejects.
        let err = create_page(&mut v, "Sub/New.md", &[], "", &g).unwrap_err();
        assert!(matches!(err, MutateError::AlreadyExists(_)));
    }

    #[test]
    fn create_rejects_escaping_paths() {
        let (_dir, mut v, g) = open_empty();
        assert!(matches!(
            create_page(&mut v, "../escape.md", &[], "", &g),
            Err(MutateError::InvalidPath(_))
        ));
        assert!(matches!(
            create_page(&mut v, "/abs.md", &[], "", &g),
            Err(MutateError::InvalidPath(_))
        ));
    }

    #[test]
    fn append_adds_leading_newline_when_missing() {
        let (dir, mut v, g) = open_empty();
        create_page(&mut v, "A.md", &[], "first line", &g).unwrap();
        append_to_page(&mut v, "A.md", "second", false, &g).unwrap();
        let on_disk = fs::read_to_string(dir.path().join("A.md")).unwrap();
        assert_eq!(on_disk, "first line\nsecond");

        append_to_page(&mut v, "A.md", " inline", true, &g).unwrap();
        let on_disk = fs::read_to_string(dir.path().join("A.md")).unwrap();
        assert_eq!(on_disk, "first line\nsecond inline");
    }

    #[test]
    fn prepend_lands_after_frontmatter() {
        let (dir, mut v, g) = open_empty();
        let fm = vec![FrontmatterEntry {
            key: "k".into(),
            value: serde_json::json!("v"),
        }];
        create_page(&mut v, "P.md", &fm, "body\n", &g).unwrap();
        prepend_to_page(&mut v, "P.md", "TOP", false, &g).unwrap();
        let on_disk = fs::read_to_string(dir.path().join("P.md")).unwrap();
        assert!(on_disk.starts_with("---\n"));
        // After the closing `---\n` we expect `TOP\nbody\n`.
        let after_fm = on_disk.rsplit_once("---\n").unwrap().1;
        assert_eq!(after_fm, "TOP\nbody\n");
    }

    #[test]
    fn delete_removes_file_and_snapshot_entry() {
        let (dir, mut v, g) = open_empty();
        create_page(&mut v, "Del.md", &[], "x", &g).unwrap();
        assert!(dir.path().join("Del.md").exists());
        delete_page(&mut v, "Del.md", &g).unwrap();
        assert!(!dir.path().join("Del.md").exists());
        assert!(v.page("Del.md").is_none());
        assert!(matches!(
            delete_page(&mut v, "Del.md", &g),
            Err(MutateError::NotFound(_))
        ));
    }

    #[test]
    fn move_renames_file_and_updates_snapshot() {
        let (dir, mut v, g) = open_empty();
        create_page(&mut v, "From.md", &[], "x", &g).unwrap();
        move_page(&mut v, "From.md", "Folder/To.md", &g).unwrap();
        assert!(!dir.path().join("From.md").exists());
        assert!(dir.path().join("Folder/To.md").exists());
        assert!(v.page("From.md").is_none());
        let moved = v.page("Folder/To.md").expect("moved page in snapshot");
        assert_eq!(moved.folder, "Folder");
        assert_eq!(moved.basename, "To");

        // Destination collision.
        create_page(&mut v, "Other.md", &[], "", &g).unwrap();
        assert!(matches!(
            move_page(&mut v, "Folder/To.md", "Other.md", &g),
            Err(MutateError::AlreadyExists(_))
        ));
    }

    #[test]
    fn set_property_inserts_then_updates() {
        let (dir, mut v, g) = open_empty();
        create_page(&mut v, "Pr.md", &[], "body\n", &g).unwrap();
        set_property(&mut v, "Pr.md", "status", serde_json::json!("draft"), &g).unwrap();
        let on_disk = fs::read_to_string(dir.path().join("Pr.md")).unwrap();
        assert!(on_disk.contains("status: draft"));
        assert!(on_disk.ends_with("body\n"));

        set_property(&mut v, "Pr.md", "status", serde_json::json!("done"), &g).unwrap();
        let on_disk = fs::read_to_string(dir.path().join("Pr.md")).unwrap();
        assert!(on_disk.contains("status: done"));
        assert!(!on_disk.contains("status: draft"));

        let page = v.page("Pr.md").unwrap();
        let entry = page
            .parsed
            .frontmatter
            .iter()
            .find(|e| e.key == "status")
            .unwrap();
        assert_eq!(entry.value, serde_json::json!("done"));
    }

    #[test]
    fn remove_property_drops_key_and_noops_when_absent() {
        let (dir, mut v, g) = open_empty();
        let fm = vec![
            FrontmatterEntry {
                key: "a".into(),
                value: serde_json::json!(1),
            },
            FrontmatterEntry {
                key: "b".into(),
                value: serde_json::json!(2),
            },
        ];
        create_page(&mut v, "R.md", &fm, "body\n", &g).unwrap();
        remove_property(&mut v, "R.md", "a", &g).unwrap();
        let on_disk = fs::read_to_string(dir.path().join("R.md")).unwrap();
        assert!(!on_disk.contains("a:"));
        assert!(on_disk.contains("b:"));

        // No-op for absent key.
        remove_property(&mut v, "R.md", "missing", &g).unwrap();
    }
}
