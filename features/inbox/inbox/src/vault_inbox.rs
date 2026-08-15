//! Disk-backed inbox. Captured items live as markdown at
//! `<vault_root>/Records/inbox/<id>.md`. Writes serialize against a
//! coarse `Mutex` so concurrent UI/CLI callers don't race on a file.
//! Mirrors `scheduling::VaultScheduler` (minus the sidecar stores —
//! the inbox has no app-state to cache).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use thiserror::Error;

use inbox_proto::{Inbox, InboxError, InboxItem};

use crate::parse::{frontmatter_split, parse_inbox_item};
use crate::write::serialize_inbox_item;

/// Captured items live under `Records/inbox/` — an append-as-you-go
/// history the daily review drains.
const INBOX_DIR: &str = "Records/inbox";

/// Errors not covered by `InboxError` (path / root validation).
#[derive(Debug, Error)]
pub enum VaultInboxError {
    #[error("invalid vault root: {0}")]
    BadRoot(String),
}

/// Disk-backed [`Inbox`] implementation. `Clone` is cheap — the root
/// is a `PathBuf` and the lock is `Arc`'d — so the server can hand a
/// clone to the mounted vox descriptor.
#[derive(Clone, architect::HasDispatcher)]
pub struct VaultInbox {
    root: PathBuf,
    write_lock: Arc<Mutex<()>>,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful mutation publishes the post-write state here
    /// ([`inbox_proto::InboxEvent`]). Sliding mailbox: a slow
    /// subscriber loses its *oldest* queued events, which is correct
    /// for state-shaped payloads. Clones share the hub (`Arc`
    /// inside), so the service mount and the stream mount can each
    /// hold a backend clone.
    events: architect::PubSub<inbox_proto::InboxEvent>,
}

impl VaultInbox {
    /// Open an inbox rooted at `vault_root`. The `Records/inbox/`
    /// subdir is created lazily on first write so empty installs
    /// don't litter the vault.
    pub fn new(vault_root: impl Into<PathBuf>) -> Result<Self, VaultInboxError> {
        let root = vault_root.into();
        if !root.is_dir() {
            return Err(VaultInboxError::BadRoot(root.display().to_string()));
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

    fn write_file(&self, rel_path: &str, body: &str) -> Result<(), InboxError> {
        let _guard = self.write_lock.lock().map_err(|_| InboxError::Backend {
            message: "vault inbox lock poisoned".into(),
        })?;
        let abs = self.root.join(rel_path);
        if let Some(parent) = abs.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(&abs, body).map_err(io)?;
        Ok(())
    }

    fn delete_file(&self, rel_path: &str) -> Result<(), InboxError> {
        let _guard = self.write_lock.lock().map_err(|_| InboxError::Backend {
            message: "vault inbox lock poisoned".into(),
        })?;
        let abs = self.root.join(rel_path);
        if abs.exists() {
            std::fs::remove_file(&abs).map_err(io)?;
        }
        Ok(())
    }
}

impl Inbox for VaultInbox {
    fn list_inbox(&self) -> Result<Vec<InboxItem>, InboxError> {
        let dir = self.root.join(INBOX_DIR);
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
                    tracing::warn!(?path, error = %e, "inbox: skip unreadable file");
                    continue;
                }
            };
            let Some((fm, body)) = frontmatter_split(&raw) else {
                tracing::warn!(?path, "inbox: skip file with no frontmatter");
                continue;
            };
            let rel = relativize(&self.root, &path);
            match parse_inbox_item(&rel, fm, body) {
                Ok(item) => out.push(item),
                Err(e) => tracing::warn!(?path, error = %e, "inbox: parse failed"),
            }
        }
        // Oldest first — the daily review drains the queue from the top.
        out.sort_by(|a, b| a.created.cmp(&b.created));
        Ok(out)
    }

    fn review_queue(&self, today: String) -> Result<Vec<InboxItem>, InboxError> {
        // list_inbox already returns oldest-first; keep only what
        // surfaces today per the shared InboxItem rule.
        Ok(self
            .list_inbox()?
            .into_iter()
            .filter(|i| i.in_review_queue(&today))
            .collect())
    }

    fn get_inbox_item(&self, id: &str) -> Result<InboxItem, InboxError> {
        let rel = format!("{INBOX_DIR}/{}.md", sanitize(id));
        let abs = self.root.join(&rel);
        if !abs.is_file() {
            return Err(InboxError::NotFound { id: id.to_string() });
        }
        let raw = std::fs::read_to_string(&abs).map_err(io)?;
        let Some((fm, body)) = frontmatter_split(&raw) else {
            return Err(InboxError::Backend {
                message: format!("no frontmatter in {rel}"),
            });
        };
        parse_inbox_item(&rel, fm, body).map_err(|e| InboxError::Backend {
            message: format!("parse {rel}: {e}"),
        })
    }

    fn upsert_inbox_item(&self, item: &InboxItem) -> Result<(), InboxError> {
        if item.id.trim().is_empty() {
            return Err(InboxError::Invalid {
                field: "item.id".into(),
                reason: "id must be non-empty".into(),
            });
        }
        let body = serialize_inbox_item(item).map_err(|e| InboxError::Backend {
            message: e.to_string(),
        })?;
        self.write_file(&format!("{INBOX_DIR}/{}.md", sanitize(&item.id)), &body)?;
        // Publish only after the write landed — subscribers fold
        // these into state fetched via `list_inbox()`, so a phantom
        // event would desync them.
        self.events
            .publish(inbox_proto::InboxEvent::Upserted(item.clone()));
        Ok(())
    }

    fn delete_inbox_item(&self, id: &str) -> Result<(), InboxError> {
        self.delete_file(&format!("{INBOX_DIR}/{}.md", sanitize(id)))?;
        self.events
            .publish(inbox_proto::InboxEvent::Deleted(id.to_owned()));
        Ok(())
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// [`Inbox`] impl above, on every successful mutation.
impl inbox_proto::InboxStreamSource for VaultInbox {
    fn events_hub(&self) -> &architect::PubSub<inbox_proto::InboxEvent> {
        &self.events
    }
}

fn io(e: std::io::Error) -> InboxError {
    InboxError::Backend {
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

/// Strip anything that doesn't belong in a vault filename so a
/// hostile id can't escape its subdir.
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
    use tempfile::TempDir;

    fn fixture() -> (TempDir, VaultInbox) {
        let tmp = TempDir::new().expect("tempdir");
        let inbox = VaultInbox::new(tmp.path()).expect("new inbox");
        (tmp, inbox)
    }

    #[test]
    fn capture_then_list_and_get() {
        let (_tmp, inbox) = fixture();
        let item = InboxItem::capture("id-1", "buy nvidia stock", "cli", "2026-05-31T09:00:00Z");
        inbox.upsert_inbox_item(&item).unwrap();

        let got = inbox.get_inbox_item("id-1").unwrap();
        assert_eq!(got.body, "buy nvidia stock");
        assert!(got.is_open());

        let list = inbox.list_inbox().unwrap();
        assert_eq!(list.len(), 1);
    }

    #[test]
    fn list_is_oldest_first() {
        let (_tmp, inbox) = fixture();
        inbox
            .upsert_inbox_item(&InboxItem::capture(
                "b",
                "second",
                "cli",
                "2026-05-31T10:00:00Z",
            ))
            .unwrap();
        inbox
            .upsert_inbox_item(&InboxItem::capture(
                "a",
                "first",
                "cli",
                "2026-05-31T08:00:00Z",
            ))
            .unwrap();
        let list = inbox.list_inbox().unwrap();
        assert_eq!(list[0].body, "first");
        assert_eq!(list[1].body, "second");
    }

    #[test]
    fn delete_removes_item() {
        let (_tmp, inbox) = fixture();
        inbox
            .upsert_inbox_item(&InboxItem::capture(
                "x",
                "ephemeral",
                "cli",
                "2026-05-31T09:00:00Z",
            ))
            .unwrap();
        inbox.delete_inbox_item("x").unwrap();
        assert!(matches!(
            inbox.get_inbox_item("x"),
            Err(InboxError::NotFound { .. })
        ));
        assert!(inbox.list_inbox().unwrap().is_empty());
    }
}
