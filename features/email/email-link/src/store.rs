//! SQLite-backed reverse-lookup index for email links.
//!
//! The frontmatter on the entity's markdown file is canonical
//! (Obsidian-compat, human-editable, survives sync). This index
//! exists so a query like "which entities link to message X?"
//! is O(log N) instead of O(vault-scan). It's disposable —
//! `LinkStore::rebuild_from(entities)` repopulates from any
//! source the caller chooses.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};

use crate::entity::{EntityKind, EntityRef};
use crate::error::Result;
use crate::link::{EmailLink, bare_message_id};

const SCHEMA_V1: &str = r"
CREATE TABLE IF NOT EXISTS email_links (
    message_id  TEXT NOT NULL,
    entity_kind TEXT NOT NULL,
    entity_id   TEXT NOT NULL,
    linked_at   INTEGER,
    linked_by   TEXT,
    user_tags   TEXT NOT NULL DEFAULT '[]',
    PRIMARY KEY (message_id, entity_kind, entity_id)
);

CREATE INDEX IF NOT EXISTS idx_email_links_by_entity
    ON email_links(entity_kind, entity_id);

CREATE INDEX IF NOT EXISTS idx_email_links_by_message
    ON email_links(message_id);
";

pub struct LinkStore {
    pub root: PathBuf,
    conn: Connection,
}

impl LinkStore {
    /// Open (or create) `<root>/links.db`. Same threading
    /// posture as `email_store::Store`: one writer at a time,
    /// readers can share via `Arc<Mutex<...>>`.
    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)?;
        let conn = Connection::open(root.join("links.db"))?;
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;\n\
             PRAGMA synchronous = NORMAL;",
        )?;
        conn.execute_batch(SCHEMA_V1)?;
        Ok(Self { root, conn })
    }

    /// Insert-or-replace one link. Idempotent; calling twice
    /// with the same `(message_id, entity)` is a no-op except
    /// for refreshed `linked_at` / `linked_by` / `user_tags`.
    pub fn upsert(&mut self, link: &EmailLink) -> Result<()> {
        let bare = bare_message_id(&link.message_id).to_string();
        let tags = serde_json::to_string(&link.user_tags)?;
        let linked_at_unix = link.linked_at.map(|d| d.timestamp());
        self.conn.execute(
            "INSERT INTO email_links (message_id, entity_kind, entity_id, linked_at, linked_by, user_tags)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(message_id, entity_kind, entity_id) DO UPDATE SET
                linked_at = COALESCE(excluded.linked_at, email_links.linked_at),
                linked_by = COALESCE(excluded.linked_by, email_links.linked_by),
                user_tags = excluded.user_tags",
            params![
                bare,
                link.entity.kind.as_str(),
                link.entity.id,
                linked_at_unix,
                link.linked_by,
                tags,
            ],
        )?;
        Ok(())
    }

    /// Remove one link. Idempotent.
    pub fn unlink(&mut self, message_id: &str, entity: &EntityRef) -> Result<()> {
        let bare = bare_message_id(message_id);
        self.conn.execute(
            "DELETE FROM email_links
             WHERE message_id = ?1 AND entity_kind = ?2 AND entity_id = ?3",
            params![bare, entity.kind.as_str(), entity.id],
        )?;
        Ok(())
    }

    /// Every link pointing at one entity. Newest-first by
    /// `linked_at` (NULLs last).
    pub fn links_for_entity(&self, entity: &EntityRef) -> Result<Vec<EmailLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT message_id, entity_kind, entity_id, linked_at, linked_by, user_tags
             FROM email_links
             WHERE entity_kind = ?1 AND entity_id = ?2
             ORDER BY linked_at DESC NULLS LAST",
        )?;
        let rows = stmt.query_map(params![entity.kind.as_str(), entity.id], row_to_link)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Every entity linking to one message. Same ordering.
    pub fn links_for_message(&self, message_id: &str) -> Result<Vec<EmailLink>> {
        let bare = bare_message_id(message_id);
        let mut stmt = self.conn.prepare(
            "SELECT message_id, entity_kind, entity_id, linked_at, linked_by, user_tags
             FROM email_links
             WHERE message_id = ?1
             ORDER BY linked_at DESC NULLS LAST",
        )?;
        let rows = stmt.query_map(params![bare], row_to_link)?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Number of unique messages linked from this entity.
    pub fn count_for_entity(&self, entity: &EntityRef) -> Result<u32> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM email_links WHERE entity_kind = ?1 AND entity_id = ?2",
            params![entity.kind.as_str(), entity.id],
            |row| row.get(0),
        )?;
        Ok(n as u32)
    }

    /// Wipe + repopulate from a set of `(entity, [message_ids])`
    /// pairs. Used when rebuilding the index from on-disk
    /// frontmatter walks.
    pub fn rebuild_from<I, J>(&mut self, entities: I) -> Result<usize>
    where
        I: IntoIterator<Item = (EntityRef, J)>,
        J: IntoIterator<Item = String>,
    {
        let tx = self.conn.transaction()?;
        tx.execute("DELETE FROM email_links", [])?;
        let mut count = 0usize;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO email_links (message_id, entity_kind, entity_id, user_tags)
                 VALUES (?1, ?2, ?3, '[]')
                 ON CONFLICT DO NOTHING",
            )?;
            for (entity, message_ids) in entities {
                for mid in message_ids {
                    let bare = bare_message_id(&mid).to_string();
                    stmt.execute(params![bare, entity.kind.as_str(), entity.id])?;
                    count += 1;
                }
            }
        }
        tx.commit()?;
        Ok(count)
    }
}

fn row_to_link(row: &rusqlite::Row<'_>) -> rusqlite::Result<EmailLink> {
    let message_id: String = row.get(0)?;
    let kind: String = row.get(1)?;
    let id: String = row.get(2)?;
    let linked_at: Option<i64> = row.get(3)?;
    let linked_by: Option<String> = row.get(4)?;
    let tags_json: String = row.get(5)?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(EmailLink {
        message_id,
        entity: EntityRef::new(EntityKind::new(kind), id),
        linked_at: linked_at.and_then(|s| DateTime::<Utc>::from_timestamp(s, 0)),
        linked_by,
        user_tags: tags,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(mid: &str, entity: EntityRef) -> EmailLink {
        EmailLink {
            message_id: mid.into(),
            entity,
            linked_at: Some(Utc::now()),
            linked_by: Some("user".into()),
            user_tags: vec!["urgent".into()],
        }
    }

    #[test]
    fn upsert_and_lookup_bidirectional() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LinkStore::open(dir.path()).unwrap();
        let task = EntityRef::task("task-1");
        let project = EntityRef::project("proj-1");

        store
            .upsert(&link("<a@example.com>", task.clone()))
            .unwrap();
        store
            .upsert(&link("<a@example.com>", project.clone()))
            .unwrap();
        store
            .upsert(&link("<b@example.com>", task.clone()))
            .unwrap();

        // Forward: the task lists two messages.
        let task_links = store.links_for_entity(&task).unwrap();
        assert_eq!(task_links.len(), 2);

        // Reverse: <a> is linked to both task + project.
        let a_links = store.links_for_message("<a@example.com>").unwrap();
        assert_eq!(a_links.len(), 2);
        let kinds: Vec<_> = a_links.iter().map(|l| l.entity.kind.as_str()).collect();
        assert!(kinds.contains(&"task"));
        assert!(kinds.contains(&"project"));
    }

    #[test]
    fn message_id_brackets_normalized() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LinkStore::open(dir.path()).unwrap();
        let task = EntityRef::task("task-1");

        store
            .upsert(&link("<a@example.com>", task.clone()))
            .unwrap();
        // Look up by bare id — should still hit.
        let links = store.links_for_message("a@example.com").unwrap();
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].message_id, "a@example.com");
    }

    #[test]
    fn unlink_removes_one_side_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LinkStore::open(dir.path()).unwrap();
        let task = EntityRef::task("task-1");
        let project = EntityRef::project("proj-1");
        store.upsert(&link("<a@b.com>", task.clone())).unwrap();
        store.upsert(&link("<a@b.com>", project.clone())).unwrap();

        store.unlink("<a@b.com>", &task).unwrap();

        assert_eq!(store.links_for_entity(&task).unwrap().len(), 0);
        assert_eq!(store.links_for_entity(&project).unwrap().len(), 1);
    }

    #[test]
    fn upsert_idempotent_on_repeat() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LinkStore::open(dir.path()).unwrap();
        let task = EntityRef::task("t");
        store.upsert(&link("<a@b.com>", task.clone())).unwrap();
        store.upsert(&link("<a@b.com>", task.clone())).unwrap();
        store.upsert(&link("<a@b.com>", task.clone())).unwrap();
        assert_eq!(store.count_for_entity(&task).unwrap(), 1);
    }

    #[test]
    fn rebuild_from_replaces_all_links() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LinkStore::open(dir.path()).unwrap();
        let task = EntityRef::task("t");
        let project = EntityRef::project("p");

        store.upsert(&link("<stale@b.com>", task.clone())).unwrap();

        let n = store
            .rebuild_from(vec![
                (
                    task.clone(),
                    vec!["<a@b.com>".to_string(), "<b@b.com>".to_string()],
                ),
                (project.clone(), vec!["<a@b.com>".to_string()]),
            ])
            .unwrap();
        assert_eq!(n, 3);

        // Stale link is gone.
        let task_links = store.links_for_entity(&task).unwrap();
        assert_eq!(task_links.len(), 2);
        assert!(!task_links.iter().any(|l| l.message_id == "stale@b.com"));
    }

    #[test]
    fn count_for_entity_matches_links_len() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LinkStore::open(dir.path()).unwrap();
        let task = EntityRef::task("t");
        for i in 0..5 {
            store
                .upsert(&link(&format!("<m{i}@b.com>"), task.clone()))
                .unwrap();
        }
        assert_eq!(store.count_for_entity(&task).unwrap(), 5);
    }
}
