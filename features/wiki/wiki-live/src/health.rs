//! Cheap pollable snapshot of wiki state — counts +
//! timestamps. Matches `wiki_proto::health::WikiHealth`.

use std::fs;

use chrono::{DateTime, Utc};
use wiki_proto::paths;

use crate::error::WikiLiveError;
use crate::queue::{IngestStatus, IngestTask, QueueFile};

use crate::vault::WikiLive;

/// Trimmed-down version of `wiki_proto::health::WikiHealth`
/// that lives entirely in `wiki-live`. The proto type is
/// what would land on the wire; this is the local-side
/// reading.
#[derive(Debug, Clone)]
pub struct WikiHealth {
    pub bootstrap_done: bool,
    pub schema_present: bool,
    pub purpose_present: bool,
    pub page_count: u32,
    pub source_count: u32,
    pub queue_depth: u32,
    pub queue_failed: u32,
    pub last_ingest_at: Option<DateTime<Utc>>,
    pub last_rescan_at: Option<DateTime<Utc>>,
}

impl WikiLive {
    pub fn health(&self) -> Result<WikiHealth, WikiLiveError> {
        let bootstrap_done = self.is_bootstrapped();
        if !bootstrap_done {
            return Ok(WikiHealth {
                bootstrap_done: false,
                schema_present: false,
                purpose_present: false,
                page_count: 0,
                source_count: 0,
                queue_depth: 0,
                queue_failed: 0,
                last_ingest_at: None,
                last_rescan_at: None,
            });
        }
        let root = self.wiki_root();
        let schema_present = root.join(paths::SCHEMA_MD).is_file()
            && fs::metadata(root.join(paths::SCHEMA_MD))?.len() > 0;
        let purpose_present = root.join(paths::PURPOSE_MD).is_file()
            && fs::metadata(root.join(paths::PURPOSE_MD))?.len() > 0;
        let page_count = count_pages(&root)?;
        let source_count = count_files(&root.join(paths::SOURCES_DIR))?;
        let queue: QueueFile = self.load_state()?;
        let mut depth = 0u32;
        let mut failed = 0u32;
        let mut last_ingest_at: Option<DateTime<Utc>> = None;
        for task in queue_tasks(&queue) {
            match task.status {
                IngestStatus::Pending
                | IngestStatus::Analyzing
                | IngestStatus::Generating
                | IngestStatus::Writing => depth += 1,
                IngestStatus::Failed => failed += 1,
                IngestStatus::Done => {
                    last_ingest_at = match last_ingest_at {
                        Some(prev) if prev >= task.updated_at => Some(prev),
                        _ => Some(task.updated_at),
                    };
                }
                IngestStatus::Cancelled => {}
            }
        }
        let snap: crate::snapshot::Snapshot = self.load_state()?;
        let last_rescan_at = snap.updated_at;
        Ok(WikiHealth {
            bootstrap_done,
            schema_present,
            purpose_present,
            page_count,
            source_count,
            queue_depth: depth,
            queue_failed: failed,
            last_ingest_at,
            last_rescan_at,
        })
    }
}

fn count_pages(root: &std::path::Path) -> Result<u32, WikiLiveError> {
    const SKIP_FILES: &[&str] = &[
        paths::SCHEMA_MD,
        paths::PURPOSE_MD,
        paths::INDEX_MD,
        paths::LOG_MD,
        paths::OVERVIEW_MD,
    ];
    let mut count = 0u32;
    for entry in walkdir::WalkDir::new(root)
        .into_iter()
        .filter_map(Result::ok)
    {
        let p = entry.path();
        if !p.is_file() || p.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let rel = p
            .strip_prefix(root)
            .map(|r| r.to_string_lossy().to_string())
            .unwrap_or_default();
        if SKIP_FILES.contains(&rel.as_str()) {
            continue;
        }
        if rel.starts_with("raw/") || rel.starts_with("_state/") || rel.starts_with("media/") {
            continue;
        }
        count += 1;
    }
    Ok(count)
}

fn count_files(dir: &std::path::Path) -> Result<u32, WikiLiveError> {
    if !dir.is_dir() {
        return Ok(0);
    }
    let mut count = 0u32;
    for entry in walkdir::WalkDir::new(dir)
        .into_iter()
        .filter_map(Result::ok)
    {
        let p = entry.path();
        if !p.is_file() {
            continue;
        }
        if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            if name.starts_with('.') {
                continue;
            }
        }
        count += 1;
    }
    Ok(count)
}

fn queue_tasks(queue: &QueueFile) -> &[IngestTask] {
    queue.tasks_ref()
}
