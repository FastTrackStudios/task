//! `Wiki/_state/snapshot.json` — sha256-keyed bookkeeping
//! of every tracked raw source. Used by [`rescan_sources`]
//! to skip re-ingest of unchanged bytes (matches `llm_wiki`'s
//! `file_sync` model).

use std::collections::HashMap;
use std::fs;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use wiki_proto::paths;

use crate::error::WikiLiveError;
use crate::state::StateFile;
use crate::vault::WikiLive;

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    #[serde(default)]
    pub(crate) version: u32,
    #[serde(default)]
    pub(crate) updated_at: Option<DateTime<Utc>>,
    /// Wiki-relative path → sha256 hex.
    #[serde(default)]
    pub(crate) files: HashMap<String, SnapshotEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(crate) struct SnapshotEntry {
    pub(crate) sha256: String,
    pub(crate) size: u64,
    pub(crate) last_seen: DateTime<Utc>,
}

impl StateFile for Snapshot {
    const FILENAME: &'static str = paths::SNAPSHOT_JSON;
}

/// Diff result returned by [`WikiLive::rescan_sources`].
#[derive(Debug, Clone)]
pub struct RescanDiff {
    /// New files since the last snapshot.
    pub created: Vec<String>,
    /// Files whose bytes changed.
    pub modified: Vec<String>,
    /// Files that have disappeared from disk since the
    /// last scan.
    pub deleted: Vec<String>,
}

impl WikiLive {
    /// Walk `Wiki/raw/sources/`, diff against
    /// `snapshot.json`, refresh the snapshot, and return
    /// what changed. Doesn't enqueue ingest tasks itself —
    /// callers (CLI / agent) decide whether to feed each
    /// diff into the queue.
    pub fn rescan_sources(&self) -> Result<RescanDiff, WikiLiveError> {
        if !self.is_bootstrapped() {
            return Err(WikiLiveError::NotBootstrapped);
        }
        let sources_dir = self.wiki_root().join(paths::SOURCES_DIR);
        let mut on_disk: HashMap<String, (Vec<u8>, u64)> = HashMap::new();
        if sources_dir.is_dir() {
            collect_files(&sources_dir, &sources_dir, paths::SOURCES_DIR, &mut on_disk)?;
        }

        let mut snap: Snapshot = self.load_state()?;
        if snap.version == 0 {
            snap.version = 1;
        }
        let mut diff = RescanDiff {
            created: Vec::new(),
            modified: Vec::new(),
            deleted: Vec::new(),
        };
        for (rel, (bytes, size)) in &on_disk {
            let hash = sha256_hex(bytes);
            let entry = SnapshotEntry {
                sha256: hash.clone(),
                size: *size,
                last_seen: Utc::now(),
            };
            match snap.files.get(rel) {
                None => {
                    diff.created.push(rel.clone());
                    snap.files.insert(rel.clone(), entry);
                }
                Some(prev) if prev.sha256 != hash => {
                    diff.modified.push(rel.clone());
                    snap.files.insert(rel.clone(), entry);
                }
                Some(_) => {
                    // Up-to-date; refresh last_seen.
                    snap.files.insert(rel.clone(), entry);
                }
            }
        }
        let known: Vec<String> = snap.files.keys().cloned().collect();
        for rel in known {
            if !on_disk.contains_key(&rel) {
                diff.deleted.push(rel.clone());
                snap.files.remove(&rel);
            }
        }
        snap.updated_at = Some(Utc::now());
        self.save_state(&snap)?;
        Ok(diff)
    }

    /// Look up the recorded sha256 for a raw source.
    /// `None` ⇒ not in the snapshot.
    pub fn snapshot_hash(&self, rel_path: &str) -> Result<Option<String>, WikiLiveError> {
        let snap: Snapshot = self.load_state()?;
        Ok(snap.files.get(rel_path).map(|e| e.sha256.clone()))
    }
}

fn collect_files(
    root: &std::path::Path,
    dir: &std::path::Path,
    prefix: &str,
    out: &mut HashMap<String, (Vec<u8>, u64)>,
) -> Result<(), WikiLiveError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(root, &path, prefix, out)?;
            continue;
        }
        let rel = path.strip_prefix(root).map_or_else(
            |_| path.to_string_lossy().to_string(),
            |p| format!("{prefix}/{}", p.to_string_lossy()),
        );
        // `.gitkeep` and other dotfiles are ignored.
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if name.starts_with('.') {
                continue;
            }
        }
        let bytes = fs::read(&path)?;
        let size = bytes.len() as u64;
        out.insert(rel, (bytes, size));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}
