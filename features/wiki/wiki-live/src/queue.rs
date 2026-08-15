//! Ingest queue + `record_pages` batch writer.

use std::fs;
use std::io::Write;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;
use wiki_proto::paths;

use crate::error::WikiLiveError;
use crate::raw::is_raw_path;
use crate::state::{StateFile, atomic_write};
use crate::vault::WikiLive;

/// State of one ingest task. Mirrors
/// `wiki_proto::ingest::IngestStatus` but kept simple here
/// (no Facet derive — purely on-disk JSON).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IngestStatus {
    Pending,
    Analyzing,
    Generating,
    Writing,
    Done,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceChange {
    Created,
    Modified,
    Deleted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IngestTask {
    pub id: String,
    pub source_path: String,
    pub kind: SourceChange,
    pub status: IngestStatus,
    pub source_sha256: String,
    #[serde(default)]
    pub analysis: Option<String>,
    /// Resulting page paths from `record_pages`.
    #[serde(default)]
    pub pages_written: Vec<String>,
    pub retries: u32,
    pub last_error: Option<String>,
    pub enqueued_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// One page draft accepted by [`WikiLive::record_pages`].
#[derive(Debug, Clone)]
pub struct PageDraft {
    /// Wiki-relative path (e.g. `Concepts/Foo.md`).
    pub path: String,
    /// Full markdown including frontmatter.
    pub markdown: String,
    /// Set to `true` to overwrite an existing page.
    pub overwrite: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct QueueFile {
    #[serde(default)]
    pub(crate) tasks: Vec<IngestTask>,
}

impl QueueFile {
    pub(crate) fn tasks_ref(&self) -> &[IngestTask] {
        &self.tasks
    }
}

impl StateFile for QueueFile {
    const FILENAME: &'static str = paths::INGEST_QUEUE_JSON;
}

impl WikiLive {
    pub fn enqueue_ingest(
        &self,
        source_path: &str,
        kind: SourceChange,
        source_bytes: &[u8],
    ) -> Result<IngestTask, WikiLiveError> {
        let mut queue: QueueFile = self.load_state()?;
        let now = Utc::now();
        let task = IngestTask {
            id: format!("ingest-{}", Uuid::new_v4().simple()),
            source_path: source_path.to_string(),
            kind,
            status: IngestStatus::Pending,
            source_sha256: sha256_hex(source_bytes),
            analysis: None,
            pages_written: Vec::new(),
            retries: 0,
            last_error: None,
            enqueued_at: now,
            updated_at: now,
        };
        queue.tasks.push(task.clone());
        self.save_state(&queue)?;
        Ok(task)
    }

    pub fn list_ingest(&self) -> Result<Vec<IngestTask>, WikiLiveError> {
        Ok(self.load_state::<QueueFile>()?.tasks)
    }

    /// Auto-retry recovery — call at backend init. Any task
    /// stuck in `Analyzing` / `Generating` / `Writing`
    /// (i.e. the agent crashed mid-turn) gets bumped:
    ///
    /// - If `retries < max_retries` ⇒ status → `Pending`,
    ///   `retries += 1`, recorded `last_error` set.
    /// - Otherwise ⇒ status → `Failed` for the curator to
    ///   inspect.
    ///
    /// Returns `(retried, failed)` task ids so the caller
    /// can log + report.
    pub fn auto_retry_stuck_tasks(
        &self,
        max_retries: u32,
    ) -> Result<(Vec<String>, Vec<String>), WikiLiveError> {
        let mut queue: QueueFile = self.load_state()?;
        let now = Utc::now();
        let mut retried = Vec::new();
        let mut failed = Vec::new();
        for task in &mut queue.tasks {
            let stuck = matches!(
                task.status,
                IngestStatus::Analyzing | IngestStatus::Generating | IngestStatus::Writing
            );
            if !stuck {
                continue;
            }
            if task.retries < max_retries {
                task.status = IngestStatus::Pending;
                task.retries += 1;
                task.last_error = Some("auto-retry after backend restart".to_string());
                task.updated_at = now;
                retried.push(task.id.clone());
            } else {
                task.status = IngestStatus::Failed;
                task.last_error =
                    Some(format!("auto-retry exhausted after {max_retries} attempts"));
                task.updated_at = now;
                failed.push(task.id.clone());
            }
        }
        if !retried.is_empty() || !failed.is_empty() {
            self.save_state(&queue)?;
        }
        Ok((retried, failed))
    }

    /// Pop one `Pending` task and flip to `Analyzing`.
    pub fn claim_next_ingest(&self) -> Result<Option<IngestTask>, WikiLiveError> {
        let mut queue: QueueFile = self.load_state()?;
        let now = Utc::now();
        for task in &mut queue.tasks {
            if task.status == IngestStatus::Pending {
                task.status = IngestStatus::Analyzing;
                task.updated_at = now;
                let claimed = task.clone();
                self.save_state(&queue)?;
                return Ok(Some(claimed));
            }
        }
        Ok(None)
    }

    pub fn record_analysis(
        &self,
        task_id: &str,
        analysis: String,
    ) -> Result<IngestTask, WikiLiveError> {
        let mut queue: QueueFile = self.load_state()?;
        let task = find_task(&mut queue, task_id)?;
        task.analysis = Some(analysis);
        task.status = IngestStatus::Generating;
        task.updated_at = Utc::now();
        let snap = task.clone();
        self.save_state(&queue)?;
        Ok(snap)
    }

    /// Atomically write a batch of pages and update the
    /// task. Rejects paths under the raw layer.
    pub fn record_pages(
        &self,
        task_id: &str,
        pages: &[PageDraft],
    ) -> Result<IngestTask, WikiLiveError> {
        // Reject raw paths up-front.
        for page in pages {
            if is_raw_path(&page.path) {
                return Err(WikiLiveError::RawIsImmutable {
                    path: page.path.clone(),
                });
            }
        }

        // Write each page (atomic per-file).
        for page in pages {
            let abs = self.wiki_path(&page.path)?;
            if abs.exists() && !page.overwrite {
                return Err(WikiLiveError::IllegalState(format!(
                    "page {} already exists; pass overwrite=true",
                    page.path
                )));
            }
            if let Some(parent) = abs.parent() {
                fs::create_dir_all(parent)?;
            }
            let tmp = abs.with_extension(format!("tmp.{}", Uuid::new_v4().simple()));
            let mut f = fs::File::create(&tmp)?;
            f.write_all(stamp_ai_generated(&page.markdown).as_bytes())?;
            f.sync_all()?;
            fs::rename(&tmp, &abs)?;
        }

        let mut queue: QueueFile = self.load_state()?;
        let task = find_task(&mut queue, task_id)?;
        task.pages_written = pages.iter().map(|p| p.path.clone()).collect();
        task.status = IngestStatus::Writing;
        task.updated_at = Utc::now();
        let snap = task.clone();
        self.save_state(&queue)?;
        Ok(snap)
    }

    pub fn complete_ingest(&self, task_id: &str) -> Result<IngestTask, WikiLiveError> {
        let mut queue: QueueFile = self.load_state()?;
        let task = find_task(&mut queue, task_id)?;
        task.status = IngestStatus::Done;
        task.updated_at = Utc::now();
        let snap = task.clone();
        self.save_state(&queue)?;
        Ok(snap)
    }

    pub fn fail_ingest(&self, task_id: &str, error: &str) -> Result<IngestTask, WikiLiveError> {
        let mut queue: QueueFile = self.load_state()?;
        let task = find_task(&mut queue, task_id)?;
        task.status = IngestStatus::Failed;
        task.last_error = Some(error.to_string());
        task.updated_at = Utc::now();
        let snap = task.clone();
        self.save_state(&queue)?;
        Ok(snap)
    }
}

fn find_task<'a>(
    queue: &'a mut QueueFile,
    task_id: &str,
) -> Result<&'a mut IngestTask, WikiLiveError> {
    queue
        .tasks
        .iter_mut()
        .find(|t| t.id == task_id)
        .ok_or_else(|| WikiLiveError::TaskNotFound(task_id.to_string()))
}

/// Every page written through the ingest pipeline is machine-produced,
/// so stamp `ai_generated: true` into the frontmatter (creating a
/// block when there is none) unless the draft already declares the
/// key. This is the provenance flag the UI's Wiki tree and page
/// header surface — the wiki holds knowledge that isn't the user's
/// own writing, and generated pages must say so.
fn stamp_ai_generated(markdown: &str) -> String {
    if let Some(rest) = markdown.strip_prefix("---\n") {
        if let Some(end) = rest.find("\n---") {
            let head = &rest[..end];
            if head
                .lines()
                .any(|l| l.trim_start().starts_with("ai_generated:"))
            {
                return markdown.to_string();
            }
            return format!("---\n{head}\nai_generated: true{}", &rest[end..]);
        }
    }
    format!("---\nai_generated: true\n---\n\n{markdown}")
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    format!("{:x}", h.finalize())
}

// `atomic_write` is re-exported via the state module — pulled in
// here so the `record_pages` use site can call it if we ever
// refactor.
#[allow(dead_code)]
fn touch_dead_code_for_atomic_write() {
    let _ = atomic_write::<()>;
}

#[cfg(test)]
mod stamp_tests {
    use super::stamp_ai_generated;

    #[test]
    fn adds_key_to_existing_frontmatter() {
        let md = "---\ntitle: X\nsources: [a.md]\n---\n\nBody.\n";
        let out = stamp_ai_generated(md);
        assert_eq!(
            out,
            "---\ntitle: X\nsources: [a.md]\nai_generated: true\n---\n\nBody.\n"
        );
    }

    #[test]
    fn creates_frontmatter_when_absent() {
        let out = stamp_ai_generated("# Page\n\nBody.\n");
        assert_eq!(out, "---\nai_generated: true\n---\n\n# Page\n\nBody.\n");
    }

    #[test]
    fn respects_explicit_declaration() {
        let md = "---\nai_generated: false\n---\nBody.\n";
        assert_eq!(stamp_ai_generated(md), md);
    }
}
