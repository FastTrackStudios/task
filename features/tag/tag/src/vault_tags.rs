//! Disk-backed tag registry. The org's tags live as a single JSON
//! document at `<vault_root>/Records/tags.json` (a `Vec<Tag>`). Writes
//! serialize against a coarse `Mutex` so concurrent UI/CLI callers don't
//! race on the file. Mirrors `inbox::VaultInbox`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use tag_proto::{Tag, TagError, TagService};

/// The registry lives at `Records/tags.json` under the vault root.
const TAGS_FILE: &str = "Records/tags.json";

/// Errors not covered by [`TagError`] (path / root validation).
#[derive(Debug, Error)]
pub enum VaultTagsError {
    #[error("invalid vault root: {0}")]
    BadRoot(String),
}

/// Disk-backed [`TagService`]. `Clone` is cheap — the root is a
/// `PathBuf` and the lock is `Arc`'d — so the server can hand a clone to
/// the mounted vox descriptor.
#[derive(Clone, architect::HasDispatcher)]
pub struct VaultTags {
    root: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl VaultTags {
    /// Open the registry rooted at `vault_root`. The `Records/` subdir is
    /// created lazily on first write so empty installs don't litter the
    /// vault.
    pub fn new(vault_root: impl Into<PathBuf>) -> Result<Self, VaultTagsError> {
        let root = vault_root.into();
        if !root.is_dir() {
            return Err(VaultTagsError::BadRoot(root.display().to_string()));
        }
        Ok(Self {
            root,
            write_lock: Arc::new(Mutex::new(())),
        })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    /// Read the whole registry. Missing file → empty registry.
    fn read_all(&self) -> Result<Vec<Tag>, TagError> {
        let path = self.root.join(TAGS_FILE);
        match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| TagError::Backend {
                message: format!("parse {TAGS_FILE}: {e}"),
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
            Err(e) => Err(TagError::Backend {
                message: format!("read {TAGS_FILE}: {e}"),
            }),
        }
    }

    /// Persist the whole registry (pretty JSON, stable name order).
    fn write_all(&self, mut tags: Vec<Tag>) -> Result<(), TagError> {
        tags.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        let body = serde_json::to_vec_pretty(&tags).map_err(|e| TagError::Backend {
            message: format!("serialize tags: {e}"),
        })?;
        let path = self.root.join(TAGS_FILE);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| TagError::Backend {
                message: format!("mkdir: {e}"),
            })?;
        }
        std::fs::write(&path, body).map_err(|e| TagError::Backend {
            message: format!("write {TAGS_FILE}: {e}"),
        })
    }
}

impl TagService for VaultTags {
    fn list_tags(&self) -> Result<Vec<Tag>, TagError> {
        let mut tags = self.read_all()?;
        tags.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        Ok(tags)
    }

    fn get_tag(&self, id: &str) -> Result<Tag, TagError> {
        self.read_all()?
            .into_iter()
            .find(|t| t.id == id)
            .ok_or_else(|| TagError::NotFound { id: id.to_string() })
    }

    fn upsert_tag(&self, tag: &Tag) -> Result<(), TagError> {
        if tag.id.trim().is_empty() {
            return Err(TagError::Invalid {
                field: "tag.id".into(),
                reason: "id must be non-empty".into(),
            });
        }
        if tag.name.trim().is_empty() {
            return Err(TagError::Invalid {
                field: "tag.name".into(),
                reason: "name must be non-empty".into(),
            });
        }
        let _guard = self.write_lock.lock().map_err(|_| TagError::Backend {
            message: "vault tags lock poisoned".into(),
        })?;
        let mut tags = self.read_all()?;
        match tags.iter_mut().find(|t| t.id == tag.id) {
            Some(existing) => *existing = tag.clone(),
            None => tags.push(tag.clone()),
        }
        self.write_all(tags)
    }

    fn delete_tag(&self, id: &str) -> Result<(), TagError> {
        let _guard = self.write_lock.lock().map_err(|_| TagError::Backend {
            message: "vault tags lock poisoned".into(),
        })?;
        let mut tags = self.read_all()?;
        let before = tags.len();
        tags.retain(|t| t.id != id);
        if tags.len() == before {
            return Err(TagError::NotFound { id: id.to_string() });
        }
        self.write_all(tags)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tag_proto::TagIcon;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, VaultTags) {
        let tmp = TempDir::new().expect("tempdir");
        let store = VaultTags::new(tmp.path()).expect("new store");
        (tmp, store)
    }

    fn tag(id: &str, name: &str) -> Tag {
        let mut t = Tag::new(id, name);
        t.icon = TagIcon::named("utensils");
        t
    }

    #[test]
    fn upsert_list_get_roundtrip() {
        let (_tmp, store) = fixture();
        assert!(store.list_tags().unwrap().is_empty());

        store.upsert_tag(&tag("id-1", "Food")).unwrap();
        let got = store.get_tag("id-1").unwrap();
        assert_eq!(got.name, "Food");
        assert_eq!(got.icon, TagIcon::named("utensils"));
        assert_eq!(store.list_tags().unwrap().len(), 1);
    }

    #[test]
    fn upsert_replaces_by_id() {
        let (_tmp, store) = fixture();
        store.upsert_tag(&tag("id-1", "Food")).unwrap();
        let mut renamed = tag("id-1", "Cooking");
        renamed.color = Some("ff8800".into());
        store.upsert_tag(&renamed).unwrap();
        let list = store.list_tags().unwrap();
        assert_eq!(list.len(), 1, "same id replaces, not appends");
        assert_eq!(list[0].name, "Cooking");
        assert_eq!(list[0].color.as_deref(), Some("ff8800"));
    }

    #[test]
    fn list_is_name_sorted_case_insensitive() {
        let (_tmp, store) = fixture();
        store.upsert_tag(&tag("b", "zeta")).unwrap();
        store.upsert_tag(&tag("a", "Alpha")).unwrap();
        let names: Vec<_> = store
            .list_tags()
            .unwrap()
            .into_iter()
            .map(|t| t.name)
            .collect();
        assert_eq!(names, ["Alpha", "zeta"]);
    }

    #[test]
    fn delete_removes_and_errs_when_absent() {
        let (_tmp, store) = fixture();
        store.upsert_tag(&tag("id-1", "Food")).unwrap();
        store.delete_tag("id-1").unwrap();
        assert!(store.list_tags().unwrap().is_empty());
        assert!(matches!(
            store.delete_tag("id-1"),
            Err(TagError::NotFound { .. })
        ));
    }

    #[test]
    fn rejects_empty_id_and_name() {
        let (_tmp, store) = fixture();
        assert!(matches!(
            store.upsert_tag(&tag("", "x")),
            Err(TagError::Invalid { .. })
        ));
        assert!(matches!(
            store.upsert_tag(&tag("id", "  ")),
            Err(TagError::Invalid { .. })
        ));
    }
}
