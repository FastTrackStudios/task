//! Disk-backed recall deck. Cards live as markdown at
//! `<vault_root>/Records/recall/<id>.md`. Writes serialize against a
//! coarse `Mutex` so concurrent UI/CLI callers don't race on a file.
//! Mirrors `inbox::VaultInbox`.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use recall_proto::{Recall, RecallCard, RecallError};

use crate::parse::{frontmatter_split, parse_recall_card};
use crate::write::serialize_recall_card;

/// Learning cards live under `Records/recall/`.
const RECALL_DIR: &str = "Records/recall";

/// Errors not covered by `RecallError` (path / root validation).
#[derive(Debug, Error)]
pub enum VaultRecallError {
    #[error("invalid vault root: {0}")]
    BadRoot(String),
}

/// Disk-backed [`Recall`] implementation. `Clone` is cheap — the root
/// is a `PathBuf` and the lock is `Arc`'d — so the server can hand a
/// clone to the mounted vox descriptor.
#[derive(Clone, architect::HasDispatcher)]
pub struct VaultRecall {
    root: PathBuf,
    write_lock: Arc<Mutex<()>>,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful mutation publishes the post-write state here
    /// ([`recall_proto::RecallEvent`]). Sliding mailbox: a slow
    /// subscriber loses its *oldest* queued events, which is correct
    /// for state-shaped payloads. Clones share the hub (`Arc`
    /// inside).
    events: architect::PubSub<recall_proto::RecallEvent>,
}

impl VaultRecall {
    /// Open a deck rooted at `vault_root`. The `Records/recall/`
    /// subdir is created lazily on first write so empty installs
    /// don't litter the vault.
    pub fn new(vault_root: impl Into<PathBuf>) -> Result<Self, VaultRecallError> {
        let root = vault_root.into();
        if !root.is_dir() {
            return Err(VaultRecallError::BadRoot(root.display().to_string()));
        }
        Ok(Self {
            root,
            write_lock: Arc::new(Mutex::new(())),
            events: architect::PubSub::sliding(256),
        })
    }

    pub fn root(&self) -> &std::path::Path {
        &self.root
    }

    fn write_file(&self, rel_path: &str, body: &str) -> Result<(), RecallError> {
        let _guard = self.write_lock.lock().map_err(|_| RecallError::Backend {
            message: "vault recall lock poisoned".into(),
        })?;
        let abs = self.root.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&abs, body).map_err(io)?;
        Ok(())
    }

    fn delete_file(&self, rel_path: &str) -> Result<(), RecallError> {
        let _guard = self.write_lock.lock().map_err(|_| RecallError::Backend {
            message: "vault recall lock poisoned".into(),
        })?;
        let abs = self.root.join(rel_path);
        if abs.exists() {
            std::fs::remove_file(&abs).map_err(io)?;
        }
        Ok(())
    }
}

impl Recall for VaultRecall {
    fn list_cards(&self) -> Result<Vec<RecallCard>, RecallError> {
        let dir = self.root.join(RECALL_DIR);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in std::fs::read_dir(&dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("md") {
                continue;
            }
            let raw = match std::fs::read_to_string(&path) {
                Ok(s) => s,
                Err(e) => {
                    tracing::warn!(?path, error = %e, "recall: skip unreadable file");
                    continue;
                }
            };
            let Some((fm, body)) = frontmatter_split(&raw) else {
                tracing::warn!(?path, "recall: skip file with no frontmatter");
                continue;
            };
            let rel = relativize(&self.root, &path);
            match parse_recall_card(&rel, fm, body) {
                Ok(card) => out.push(card),
                Err(e) => tracing::warn!(?path, error = %e, "recall: parse failed"),
            }
        }
        // Oldest first — stable order for the stream + review queue.
        out.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(out)
    }

    fn review_queue(&self, today: String) -> Result<Vec<RecallCard>, RecallError> {
        Ok(self
            .list_cards()?
            .into_iter()
            .filter(|c| c.in_review_queue(&today))
            .collect())
    }

    fn upsert_card(&self, card: &RecallCard) -> Result<(), RecallError> {
        if card.id.trim().is_empty() {
            return Err(RecallError::Invalid {
                field: "card.id".into(),
                reason: "id must be non-empty".into(),
            });
        }
        let body = serialize_recall_card(card).map_err(|e| RecallError::Backend {
            message: e.to_string(),
        })?;
        self.write_file(&format!("{RECALL_DIR}/{}.md", sanitize(&card.id)), &body)?;
        // Publish only after the write landed — subscribers fold
        // these into state fetched via `list_cards()`, so a phantom
        // event would desync them.
        self.events
            .publish(recall_proto::RecallEvent::Upserted(card.clone()));
        Ok(())
    }

    fn delete_card(&self, id: &str) -> Result<(), RecallError> {
        self.delete_file(&format!("{RECALL_DIR}/{}.md", sanitize(id)))?;
        self.events
            .publish(recall_proto::RecallEvent::Deleted(id.to_owned()));
        Ok(())
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// [`Recall`] impl above, on every successful mutation.
impl recall_proto::RecallStreamSource for VaultRecall {
    fn events_hub(&self) -> &architect::PubSub<recall_proto::RecallEvent> {
        &self.events
    }
}

fn io(e: std::io::Error) -> RecallError {
    RecallError::Backend {
        message: format!("io: {e}"),
    }
}

/// Vault-relative forward-slash path.
fn relativize(root: &std::path::Path, abs: &std::path::Path) -> String {
    abs.strip_prefix(root).map_or_else(
        |_| abs.display().to_string(),
        |p| p.to_string_lossy().replace('\\', "/"),
    )
}

/// Strip anything that doesn't belong in a vault filename so a hostile
/// id can't escape its subdir.
fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| match c {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' | '.' => c,
            _ => '-',
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use recall_proto::CardType;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, VaultRecall) {
        let tmp = TempDir::new().expect("tempdir");
        let recall = VaultRecall::new(tmp.path()).expect("new recall");
        (tmp, recall)
    }

    fn card(id: &str, project: &str) -> RecallCard {
        RecallCard::create(
            id,
            project,
            CardType::CONCEPT_QA,
            "front",
            "back",
            "2026-07-16T09:00:00Z",
        )
    }

    #[test]
    fn upsert_then_list() {
        let (_tmp, recall) = fixture();
        recall.upsert_card(&card("c1", "Bible")).unwrap();
        let list = recall.list_cards().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].front, "front");
        assert_eq!(list[0].project, "Bible");
    }

    #[test]
    fn review_queue_excludes_archived_and_scheduled() {
        let (_tmp, recall) = fixture();
        // New card → always due.
        recall.upsert_card(&card("due", "Bible")).unwrap();
        // Archived → never in queue.
        let mut archived = card("arch", "Bible");
        archived.archived = true;
        recall.upsert_card(&archived).unwrap();
        // Scheduled into the future → not due today.
        let mut future = card("later", "Bible");
        future.due = Some("2999-01-01".into());
        future.reps = 1;
        future.last_review = Some("2026-07-16".into());
        recall.upsert_card(&future).unwrap();

        let queue = recall.review_queue("2026-07-16".into()).unwrap();
        let ids: Vec<_> = queue.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["due"]);
    }

    #[test]
    fn delete_removes_card() {
        let (_tmp, recall) = fixture();
        recall.upsert_card(&card("x", "Bible")).unwrap();
        recall.delete_card("x").unwrap();
        assert!(recall.list_cards().unwrap().is_empty());
    }
}
