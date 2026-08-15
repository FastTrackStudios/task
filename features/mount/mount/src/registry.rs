//! [`MountRegistry`] — TOML-backed `project_id → Mount` map.
//!
//! Lives at `$XDG_CONFIG_HOME/task/mounts.toml` by default
//! (override with `TASK_MOUNTS_TOML`). Per-machine — never
//! synced as part of an org's metadata. Adding the same
//! project to two machines means two separate `mount add`
//! invocations.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mount_proto::Mount;
use uuid::Uuid;

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("resolve mounts.toml: {0}")]
    Resolve(String),
    #[error("read {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("write {path}: {source}")]
    Write {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("parse {path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
    #[error("serialize: {source}")]
    Serialize {
        #[source]
        source: toml::ser::Error,
    },
    #[error(
        "project `{project_id}` already mounted at `{existing}`; pass `--replace` to overwrite"
    )]
    AlreadyMounted { project_id: Uuid, existing: String },
}

/// Default mounts.toml path:
/// `$TASK_MOUNTS_TOML` →
/// `$XDG_CONFIG_HOME/task/mounts.toml` →
/// `$HOME/.config/task/mounts.toml`.
pub fn default_mounts_path() -> Result<PathBuf, RegistryError> {
    if let Ok(explicit) = std::env::var("TASK_MOUNTS_TOML") {
        if !explicit.is_empty() {
            return Ok(PathBuf::from(explicit));
        }
    }
    let base = match std::env::var("XDG_CONFIG_HOME") {
        Ok(v) if !v.is_empty() => PathBuf::from(v),
        _ => {
            let home = std::env::var("HOME").map_err(|_| {
                RegistryError::Resolve(
                    "neither TASK_MOUNTS_TOML, XDG_CONFIG_HOME nor HOME is set".into(),
                )
            })?;
            PathBuf::from(home).join(".config")
        }
    };
    Ok(base.join("task").join("mounts.toml"))
}

/// In-memory registry. `load` + `save` are the boundary; the
/// rest is plain map manipulation.
#[derive(Debug, Clone)]
pub struct MountRegistry {
    path: PathBuf,
    mounts: BTreeMap<Uuid, Mount>,
}

impl MountRegistry {
    /// Create an empty registry rooted at `path`. Doesn't
    /// touch disk; call [`Self::save`] to persist.
    #[must_use]
    pub fn empty(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            mounts: BTreeMap::new(),
        }
    }

    /// Load (or create empty) from [`default_mounts_path`].
    pub fn from_env() -> Result<Self, RegistryError> {
        Self::load(default_mounts_path()?)
    }

    /// Load (or create empty) from an explicit path.
    pub fn load(path: impl Into<PathBuf>) -> Result<Self, RegistryError> {
        let path = path.into();
        if !path.exists() {
            return Ok(Self::empty(path));
        }
        let raw = std::fs::read_to_string(&path).map_err(|source| RegistryError::Read {
            path: path.display().to_string(),
            source,
        })?;
        let parsed: TomlRoot = toml::from_str(&raw).map_err(|source| RegistryError::Parse {
            path: path.display().to_string(),
            source,
        })?;
        Ok(Self {
            path,
            mounts: parsed.mount.unwrap_or_default(),
        })
    }

    /// Persist to disk. Creates parent dirs as needed.
    pub fn save(&self) -> Result<(), RegistryError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| RegistryError::Write {
                path: parent.display().to_string(),
                source,
            })?;
        }
        let root = TomlRoot {
            mount: if self.mounts.is_empty() {
                None
            } else {
                Some(self.mounts.clone())
            },
        };
        let body =
            toml::to_string_pretty(&root).map_err(|source| RegistryError::Serialize { source })?;
        std::fs::write(&self.path, body).map_err(|source| RegistryError::Write {
            path: self.path.display().to_string(),
            source,
        })
    }

    /// File-system path this registry persists to.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Register a mount. Refuses to overwrite an existing
    /// entry unless `replace = true`.
    pub fn add(&mut self, mount: Mount, replace: bool) -> Result<(), RegistryError> {
        if let Some(existing) = self.mounts.get(&mount.project_id) {
            if !replace {
                return Err(RegistryError::AlreadyMounted {
                    project_id: mount.project_id,
                    existing: existing.path.clone(),
                });
            }
        }
        self.mounts.insert(mount.project_id, mount);
        Ok(())
    }

    /// Look up a mount by project id.
    #[must_use]
    pub fn get(&self, project_id: Uuid) -> Option<&Mount> {
        self.mounts.get(&project_id)
    }

    /// Remove a mount. Returns the removed row, or `None` if
    /// the project wasn't mounted.
    pub fn remove(&mut self, project_id: Uuid) -> Option<Mount> {
        self.mounts.remove(&project_id)
    }

    /// Iterate all mounts in stable id order.
    pub fn iter(&self) -> impl Iterator<Item = &Mount> {
        self.mounts.values()
    }

    /// Number of registered mounts.
    #[must_use]
    pub fn len(&self) -> usize {
        self.mounts.len()
    }

    /// True if no mounts are registered.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.mounts.is_empty()
    }
}

/// Serde shape for the TOML file:
///
/// ```toml
/// [mount.7f3a2b1e-...-...]
/// backend = "filesystem"
/// path    = "/home/cody/Projects/foo"
/// # url, label, created_at, updated_at...
/// ```
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct TomlRoot {
    #[serde(skip_serializing_if = "Option::is_none")]
    mount: Option<BTreeMap<Uuid, Mount>>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use mount_proto::BackendKind;

    #[test]
    fn save_load_round_trip() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mounts.toml");
        let mut reg = MountRegistry::empty(&path);
        let id = Uuid::new_v4();
        reg.add(Mount::filesystem(id, "/tmp/foo"), false).unwrap();
        reg.save().unwrap();

        let reloaded = MountRegistry::load(&path).unwrap();
        assert_eq!(reloaded.len(), 1);
        let got = reloaded.get(id).unwrap();
        assert_eq!(got.path, "/tmp/foo");
        assert_eq!(got.backend, BackendKind::Filesystem);
    }

    #[test]
    fn add_refuses_duplicate_without_replace() {
        let mut reg = MountRegistry::empty("/tmp/unused.toml");
        let id = Uuid::new_v4();
        reg.add(Mount::filesystem(id, "/a"), false).unwrap();
        let err = reg.add(Mount::filesystem(id, "/b"), false).unwrap_err();
        assert!(matches!(err, RegistryError::AlreadyMounted { .. }));
        reg.add(Mount::filesystem(id, "/b"), true).unwrap();
        assert_eq!(reg.get(id).unwrap().path, "/b");
    }

    #[test]
    fn empty_registry_saves_clean_file() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mounts.toml");
        let reg = MountRegistry::empty(&path);
        reg.save().unwrap();
        let body = std::fs::read_to_string(&path).unwrap();
        // No `[mount]` section when empty.
        assert!(!body.contains("[mount"));
    }
}
