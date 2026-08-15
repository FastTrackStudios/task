//! Sync orchestrator on top of `vault_proto::VaultSync`.
//!
//! Two halves, mirroring `cooklang/cooklang-sync`'s indexer +
//! syncer split:
//!
//! - [`index_local`] — walk the local vault root, compute a
//!   sha-256 per file, return a sorted `Vec<LocalEntry>`.
//! - [`plan_sync`] — diff `local` against `remote_manifest`
//!   and emit [`SyncOp`]s (`Pull` / `Push` / `Conflict`).
//! - [`apply_ops`] — executes the plan against a
//!   [`VaultSyncBackend`] (any sync impl of
//!   `vault_proto::VaultSync`).
//!
//! Conflict policy is **newer-mtime-wins** in v1; the diff
//! emits a `Conflict` variant when the SHAs differ, and the
//! `apply_ops` helper picks the side with the higher
//! `mtime_ms` and uses `IfMatch::Force` to overwrite. Callers
//! who want richer conflict UX should call [`plan_sync`]
//! directly and resolve themselves.
//!
//! No content-defined chunking — Task vaults are
//! markdown-heavy and full-file `put_file` is the simpler
//! starting point. Per `cooklang-sync`'s notes the chunking
//! optimization only matters at scale (large binaries, many
//! small edits to the same file) — we'll revisit when we
//! measure.

use std::path::Path;

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use thiserror::Error;
use vault_proto::{IfMatch, Manifest, ManifestEntry, VaultSync, VaultSyncError};

/// Local-side counterpart to `ManifestEntry`. Computed by
/// walking the vault root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalEntry {
    pub path: String,
    pub sha256: String,
    pub mtime_ms: i64,
    pub size: u64,
}

/// One unit of sync work after diffing local vs. remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncOp {
    /// File exists on the server, not locally. Apply will
    /// `get_file` + write.
    Pull { path: String, remote_sha: String },
    /// File exists locally, not on the server. Apply will
    /// `put_file` with `IfMatch::CreateOnly`.
    Push { path: String, local_sha: String },
    /// File exists on both sides with the same SHA. No-op,
    /// kept in the plan so summaries are honest.
    InSync { path: String },
    /// SHAs differ. Newer `mtime_ms` is the canonical side
    /// (recorded in `winning_side`); `apply_ops` overwrites
    /// the other.
    Conflict {
        path: String,
        local_sha: String,
        local_mtime_ms: i64,
        remote_sha: String,
        remote_mtime_ms: i64,
        winning_side: Side,
    },
}

/// Which side won a conflict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Local,
    Remote,
}

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("walk {path}: {source}")]
    Walk {
        path: String,
        #[source]
        source: walkdir::Error,
    },
    #[error("io {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("remote: {0}")]
    Remote(#[from] VaultSyncError),
    #[error("bad path `{0}`: not under vault root")]
    BadPath(String),
}

/// Walk `vault_root` and return one [`LocalEntry`] per regular
/// file. Hidden files (`.git`, `.obsidian`, anything beginning
/// with `.`) are skipped — they're operational state, not vault
/// content.
pub fn index_local(vault_root: &Path) -> Result<Vec<LocalEntry>, SyncError> {
    let mut out = Vec::new();
    for entry in walkdir::WalkDir::new(vault_root)
        .into_iter()
        .filter_entry(|e| {
            // Always keep the root itself — its basename can start
            // with `.` (e.g. `tempfile::tempdir()` uses dot-prefixed
            // names; in production `vault_root` is `<org>/vault/`
            // which is fine but defensive against future renames).
            if e.depth() == 0 {
                return true;
            }
            e.file_name().to_str().is_none_or(|s| !s.starts_with('.'))
        })
    {
        let entry = entry.map_err(|source| SyncError::Walk {
            path: vault_root.display().to_string(),
            source,
        })?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(vault_root)
            .map_err(|_| SyncError::BadPath(entry.path().display().to_string()))?;
        let rel_str = rel.to_string_lossy().replace('\\', "/");
        let bytes = std::fs::read(entry.path()).map_err(|source| SyncError::Io {
            path: entry.path().display().to_string(),
            source,
        })?;
        let sha256 = hex::encode(Sha256::digest(&bytes));
        let metadata = entry.metadata().map_err(|source| SyncError::Walk {
            path: entry.path().display().to_string(),
            source,
        })?;
        let mtime_ms = metadata
            .modified()
            .ok()
            .map_or(0, |t| DateTime::<Utc>::from(t).timestamp_millis());
        out.push(LocalEntry {
            path: rel_str,
            sha256,
            mtime_ms,
            size: bytes.len() as u64,
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Diff local + remote, emit a plan. Pure function — no I/O.
#[must_use]
pub fn plan_sync(local: &[LocalEntry], remote: &Manifest) -> Vec<SyncOp> {
    use std::collections::HashMap;
    let local_map: HashMap<&str, &LocalEntry> =
        local.iter().map(|e| (e.path.as_str(), e)).collect();
    let remote_map: HashMap<&str, &ManifestEntry> =
        remote.files.iter().map(|e| (e.path.as_str(), e)).collect();

    let mut all_paths: Vec<&str> = local_map.keys().chain(remote_map.keys()).copied().collect();
    all_paths.sort_unstable();
    all_paths.dedup();

    let mut ops = Vec::with_capacity(all_paths.len());
    for path in all_paths {
        match (local_map.get(path), remote_map.get(path)) {
            (Some(l), None) => ops.push(SyncOp::Push {
                path: path.to_owned(),
                local_sha: l.sha256.clone(),
            }),
            (None, Some(r)) => ops.push(SyncOp::Pull {
                path: path.to_owned(),
                remote_sha: r.sha256.clone(),
            }),
            (Some(l), Some(r)) => {
                if l.sha256 == r.sha256 {
                    ops.push(SyncOp::InSync {
                        path: path.to_owned(),
                    });
                } else {
                    let winning = if l.mtime_ms >= r.mtime_ms {
                        Side::Local
                    } else {
                        Side::Remote
                    };
                    ops.push(SyncOp::Conflict {
                        path: path.to_owned(),
                        local_sha: l.sha256.clone(),
                        local_mtime_ms: l.mtime_ms,
                        remote_sha: r.sha256.clone(),
                        remote_mtime_ms: r.mtime_ms,
                        winning_side: winning,
                    });
                }
            }
            (None, None) => unreachable!("path came from one of the two maps"),
        }
    }
    ops
}

/// Trait the orchestrator calls against. Any `VaultSync` impl
/// fits — the local backend (`vault::Backend`) for in-process
/// dry runs, or the vox-generated `VaultSyncClient` for the
/// real over-the-wire flow.
///
/// The async layer lives outside this crate; callers pick the
/// runtime + driver and call `apply_one` per op.
pub trait VaultSyncBackend: VaultSync {}
impl<T: VaultSync> VaultSyncBackend for T {}

/// Apply a single op against `backend`. Pure-trait call site —
/// no async. The vault-proto trait methods are sync; for the
/// vox-generated client the caller wraps this in their async
/// runtime as needed.
pub fn apply_one<B: VaultSyncBackend>(
    backend: &B,
    vault_id: &str,
    vault_root: &Path,
    op: &SyncOp,
) -> Result<(), SyncError> {
    match op {
        SyncOp::InSync { .. } => Ok(()),
        SyncOp::Pull { path, .. } => {
            let bytes = backend.get_file(vault_id, path)?;
            write_file(vault_root, path, &bytes.0)
        }
        SyncOp::Push { path, .. } => {
            let abs = vault_root.join(path);
            let bytes = std::fs::read(&abs).map_err(|source| SyncError::Io {
                path: abs.display().to_string(),
                source,
            })?;
            backend.put_file(vault_id, path, bytes, IfMatch::CreateOnly)?;
            Ok(())
        }
        SyncOp::Conflict {
            path,
            local_sha,
            remote_sha,
            winning_side,
            ..
        } => match winning_side {
            Side::Local => {
                let abs = vault_root.join(path);
                let bytes = std::fs::read(&abs).map_err(|source| SyncError::Io {
                    path: abs.display().to_string(),
                    source,
                })?;
                backend.put_file(vault_id, path, bytes, IfMatch::Sha(remote_sha.clone()))?;
                Ok(())
            }
            Side::Remote => {
                let bytes = backend.get_file(vault_id, path)?;
                write_file(vault_root, path, &bytes.0)?;
                // Track the SHA we last saw on the server so a
                // follow-up sync sees `InSync`.
                let _ = local_sha;
                Ok(())
            }
        },
    }
}

/// Aggregate counts for one sync pass — convenient for the
/// CLI's summary line.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncSummary {
    pub pulled: usize,
    pub pushed: usize,
    pub in_sync: usize,
    pub conflicts_local_won: usize,
    pub conflicts_remote_won: usize,
}

impl SyncSummary {
    /// Tally `ops` into the summary's counters.
    pub fn record(&mut self, op: &SyncOp) {
        match op {
            SyncOp::Pull { .. } => self.pulled += 1,
            SyncOp::Push { .. } => self.pushed += 1,
            SyncOp::InSync { .. } => self.in_sync += 1,
            SyncOp::Conflict { winning_side, .. } => match winning_side {
                Side::Local => self.conflicts_local_won += 1,
                Side::Remote => self.conflicts_remote_won += 1,
            },
        }
    }
}

/// Resolve and write `path` under `vault_root`, refusing `..`
/// segments to keep the writer from escaping.
fn write_file(vault_root: &Path, path: &str, bytes: &[u8]) -> Result<(), SyncError> {
    if path.split(['/', '\\']).any(|seg| seg == "..") {
        return Err(SyncError::BadPath(path.to_owned()));
    }
    let abs = vault_root.join(path);
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent).map_err(|source| SyncError::Io {
            path: parent.display().to_string(),
            source,
        })?;
    }
    std::fs::write(&abs, bytes).map_err(|source| SyncError::Io {
        path: abs.display().to_string(),
        source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn me(path: &str, sha: &str, mtime: i64) -> ManifestEntry {
        ManifestEntry {
            path: path.into(),
            sha256: sha.into(),
            mtime_ms: mtime,
            size: 0,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }
    fn le(path: &str, sha: &str, mtime: i64) -> LocalEntry {
        LocalEntry {
            path: path.into(),
            sha256: sha.into(),
            mtime_ms: mtime,
            size: 0,
        }
    }

    #[test]
    fn local_only_is_push() {
        let local = vec![le("a.md", "h1", 100)];
        let remote = Manifest {
            vault_id: "v1".into(),
            files: vec![],
        };
        let ops = plan_sync(&local, &remote);
        assert!(matches!(ops.as_slice(), [SyncOp::Push { path, .. }] if path == "a.md"));
    }

    #[test]
    fn remote_only_is_pull() {
        let remote = Manifest {
            vault_id: "v1".into(),
            files: vec![me("b.md", "h2", 50)],
        };
        let ops = plan_sync(&[], &remote);
        assert!(matches!(ops.as_slice(), [SyncOp::Pull { path, .. }] if path == "b.md"));
    }

    #[test]
    fn matching_shas_are_in_sync() {
        let local = vec![le("c.md", "hX", 100)];
        let remote = Manifest {
            vault_id: "v1".into(),
            files: vec![me("c.md", "hX", 200)],
        };
        let ops = plan_sync(&local, &remote);
        assert!(matches!(ops.as_slice(), [SyncOp::InSync { path }] if path == "c.md"));
    }

    #[test]
    fn newer_local_wins_conflict() {
        let local = vec![le("d.md", "hL", 500)];
        let remote = Manifest {
            vault_id: "v1".into(),
            files: vec![me("d.md", "hR", 100)],
        };
        let ops = plan_sync(&local, &remote);
        match ops.as_slice() {
            [SyncOp::Conflict { winning_side, .. }] => {
                assert_eq!(*winning_side, Side::Local);
            }
            _ => panic!("expected Conflict, got {ops:?}"),
        }
    }

    #[test]
    fn newer_remote_wins_conflict() {
        let local = vec![le("e.md", "hL", 100)];
        let remote = Manifest {
            vault_id: "v1".into(),
            files: vec![me("e.md", "hR", 500)],
        };
        let ops = plan_sync(&local, &remote);
        match ops.as_slice() {
            [SyncOp::Conflict { winning_side, .. }] => {
                assert_eq!(*winning_side, Side::Remote);
            }
            _ => panic!("expected Conflict, got {ops:?}"),
        }
    }

    #[test]
    fn index_local_skips_hidden() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::write(root.join("a.md"), b"hello").unwrap();
        std::fs::create_dir_all(root.join(".obsidian")).unwrap();
        std::fs::write(root.join(".obsidian/config"), b"secret").unwrap();
        let entries = index_local(root).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, "a.md");
    }
}
