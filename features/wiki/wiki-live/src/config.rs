//! Reading and writing a wiki's declaration (`_state/wiki.json`).
//!
//! The shape is [`wiki_proto::config::WikiConfig`]; this is the disk
//! half. Two things live here beyond load/save:
//!
//! - **Retired slugs.** `wiki.many.identity`: a slug once used is never
//!   reassigned to a different wiki. Deleting a wiki writes its slug to
//!   `<wikis-dir>/.retired.json`, and creation refuses anything in that
//!   list. The list is beside the wikis rather than inside one because
//!   the wiki it describes is gone.
//! - **Implicit configs.** Every wiki that existed before this file did
//!   reads as [`WikiConfig::implicit`] — private, no Editors, no gate —
//!   so nothing that worked stops working and nothing that was private
//!   becomes public because a file was missing.

use std::path::{Path, PathBuf};

use wiki_proto::config::WikiConfig;
use wiki_proto::error::WikiError;
use wiki_proto::paths;

/// Where a wiki's config lives.
#[must_use]
pub fn config_path(wiki_root: &Path) -> PathBuf {
    wiki_root.join(paths::STATE_DIR).join(paths::WIKI_JSON)
}

/// The config for a wiki root, implicit when none has been written.
///
/// The slug passed is the directory's — when the file disagrees, the
/// file wins and the caller learns the directory was renamed, which is
/// the check `wiki.many.identity` wants.
pub fn load(wiki_root: &Path, slug: &str) -> Result<WikiConfig, WikiError> {
    let path = config_path(wiki_root);
    match std::fs::read(&path) {
        Ok(bytes) if bytes.is_empty() => Ok(WikiConfig::implicit(slug)),
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| WikiError::Backend(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(WikiConfig::implicit(slug)),
        Err(e) => Err(WikiError::Io(format!("{}: {e}", path.display()))),
    }
}

/// Write the config. Atomic (temp + rename), like every other state
/// file here, so a crash mid-write leaves the previous declaration in
/// force rather than an unparsable one.
pub fn save(wiki_root: &Path, config: &WikiConfig) -> Result<(), WikiError> {
    let path = config_path(wiki_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| WikiError::Io(e.to_string()))?;
    }
    let json = serde_json::to_vec_pretty(config).map_err(|e| WikiError::Backend(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| WikiError::Io(format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| WikiError::Io(format!("{}: {e}", path.display())))
}

/// Read, change, write.
pub fn update(
    wiki_root: &Path,
    slug: &str,
    f: impl FnOnce(&mut WikiConfig),
) -> Result<WikiConfig, WikiError> {
    let mut config = load(wiki_root, slug)?;
    f(&mut config);
    save(wiki_root, &config)?;
    Ok(config)
}

/// The file that remembers every slug this org has ever used.
#[must_use]
pub fn retired_path(wikis_dir: &Path) -> PathBuf {
    wikis_dir.join(".retired.json")
}

/// Slugs that may never be reused here.
pub fn retired(wikis_dir: &Path) -> Result<Vec<String>, WikiError> {
    let path = retired_path(wikis_dir);
    match std::fs::read(&path) {
        Ok(bytes) if bytes.is_empty() => Ok(Vec::new()),
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|e| WikiError::Backend(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(WikiError::Io(format!("{}: {e}", path.display()))),
    }
}

/// Retire a slug. Idempotent.
pub fn retire(wikis_dir: &Path, slug: &str) -> Result<(), WikiError> {
    let mut list = retired(wikis_dir)?;
    if list.iter().any(|s| s == slug) {
        return Ok(());
    }
    list.push(slug.to_owned());
    list.sort();
    std::fs::create_dir_all(wikis_dir).map_err(|e| WikiError::Io(e.to_string()))?;
    let path = retired_path(wikis_dir);
    let json = serde_json::to_vec_pretty(&list).map_err(|e| WikiError::Backend(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| WikiError::Io(format!("{}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, &path).map_err(|e| WikiError::Io(format!("{}: {e}", path.display())))
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiki_proto::config::Visibility;

    #[test]
    fn a_wiki_without_a_file_is_private_and_ungoverned() {
        let dir = tempfile::tempdir().unwrap();
        let c = load(dir.path(), "theory").unwrap();
        assert_eq!(c.slug, "theory");
        assert_eq!(c.visibility, Visibility::Private);
        assert!(!c.has_edit_lane());
    }

    #[test]
    fn save_then_load_round_trips_and_update_changes_one_field() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = WikiConfig::implicit("theory");
        c.title = "Music Theory".into();
        c.editors.push("alice".into());
        save(dir.path(), &c).unwrap();
        assert_eq!(load(dir.path(), "theory").unwrap(), c);
        let after = update(dir.path(), "theory", |c| c.visibility = Visibility::Public).unwrap();
        assert_eq!(after.visibility, Visibility::Public);
        assert_eq!(after.editors, vec!["alice".to_string()]);
        assert!(!config_path(dir.path()).with_extension("json.tmp").exists());
    }

    /// t[verify wiki.many.identity] — a retired slug stays retired
    /// across writes, and retiring twice is one entry.
    #[test]
    fn retired_slugs_accumulate_and_never_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        assert!(retired(dir.path()).unwrap().is_empty());
        retire(dir.path(), "cooking").unwrap();
        retire(dir.path(), "cooking").unwrap();
        retire(dir.path(), "bible").unwrap();
        assert_eq!(retired(dir.path()).unwrap(), vec!["bible", "cooking"]);
    }
}
