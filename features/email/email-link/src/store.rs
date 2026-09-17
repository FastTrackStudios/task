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
    entity_org  TEXT NOT NULL DEFAULT '',
    PRIMARY KEY (message_id, entity_kind, entity_id)
);

CREATE INDEX IF NOT EXISTS idx_email_links_by_entity
    ON email_links(entity_kind, entity_id);

CREATE INDEX IF NOT EXISTS idx_email_links_by_message
    ON email_links(message_id);
";

/// Add `entity_org` to a table created before it existed.
///
/// `CREATE TABLE IF NOT EXISTS` does nothing to a table that is
/// already there, so a database from before this column would keep
/// its old shape and every query naming the column would fail. The
/// default of `''` is the honest answer for those rows: they were
/// written when a link could only mean something in the store's own
/// org, and that is exactly what empty means.
///
/// The primary key deliberately does NOT grow to include the org.
/// Widening it would need the table rebuilt, and the same (message,
/// kind, id) in two different orgs is not a case worth that: ids
/// here are UUIDs and vault paths, not per-org counters.
/// Indexes that name `entity_org`, created only after the migration
/// above has guaranteed the column exists. They cannot sit in
/// `SCHEMA_V1`: on an old database `CREATE TABLE IF NOT EXISTS` is a
/// no-op, so the index would be built against a table that has no such
/// column and the whole open would fail. A test covers exactly this.
const SCHEMA_ORG_INDEXES: &str = r"
CREATE INDEX IF NOT EXISTS idx_email_links_by_org_entity
    ON email_links(entity_org, entity_kind, entity_id);
";

fn migrate_add_entity_org(conn: &Connection) -> Result<()> {
    let present: bool = conn
        .prepare("SELECT 1 FROM pragma_table_info('email_links') WHERE name = 'entity_org'")?
        .exists([])?;
    if !present {
        conn.execute_batch(
            "ALTER TABLE email_links ADD COLUMN entity_org TEXT NOT NULL DEFAULT '';",
        )?;
    }
    Ok(())
}

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
        migrate_add_entity_org(&conn)?;
        conn.execute_batch(SCHEMA_ORG_INDEXES)?;
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
            "INSERT INTO email_links (message_id, entity_kind, entity_id, linked_at, linked_by, user_tags, entity_org)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(message_id, entity_kind, entity_id) DO UPDATE SET
                linked_at  = COALESCE(excluded.linked_at, email_links.linked_at),
                linked_by  = COALESCE(excluded.linked_by, email_links.linked_by),
                user_tags  = excluded.user_tags,
                entity_org = excluded.entity_org",
            params![
                bare,
                link.entity.kind.as_str(),
                link.entity.id,
                linked_at_unix,
                link.linked_by,
                tags,
                link.entity.org,
            ],
        )?;
        Ok(())
    }

    /// Remove one link. Idempotent.
    pub fn unlink(&mut self, message_id: &str, entity: &EntityRef) -> Result<()> {
        let bare = bare_message_id(message_id);
        self.conn.execute(
            "DELETE FROM email_links
             WHERE message_id = ?1 AND entity_kind = ?2 AND entity_id = ?3
               AND entity_org = ?4",
            params![bare, entity.kind.as_str(), entity.id, entity.org],
        )?;
        Ok(())
    }

    /// Every link pointing at one entity. Newest-first by
    /// `linked_at` (NULLs last).
    pub fn links_for_entity(&self, entity: &EntityRef) -> Result<Vec<EmailLink>> {
        let mut stmt = self.conn.prepare(
            "SELECT message_id, entity_kind, entity_id, linked_at, linked_by, user_tags, entity_org
             FROM email_links
             WHERE entity_org = ?1 AND entity_kind = ?2 AND entity_id = ?3
             ORDER BY linked_at DESC NULLS LAST",
        )?;
        let rows = stmt.query_map(
            params![entity.org, entity.kind.as_str(), entity.id],
            row_to_link,
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Every entity linking to one message. Same ordering.
    pub fn links_for_message(&self, message_id: &str) -> Result<Vec<EmailLink>> {
        let bare = bare_message_id(message_id);
        let mut stmt = self.conn.prepare(
            "SELECT message_id, entity_kind, entity_id, linked_at, linked_by, user_tags, entity_org
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
            "SELECT COUNT(*) FROM email_links WHERE entity_org = ?1 AND entity_kind = ?2 AND entity_id = ?3",
            params![entity.org, entity.kind.as_str(), entity.id],
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
                "INSERT INTO email_links (message_id, entity_kind, entity_id, user_tags, entity_org)
                 VALUES (?1, ?2, ?3, '[]', ?4)
                 ON CONFLICT DO NOTHING",
            )?;
            for (entity, message_ids) in entities {
                for mid in message_ids {
                    let bare = bare_message_id(&mid).to_string();
                    stmt.execute(params![bare, entity.kind.as_str(), entity.id, entity.org])?;
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
    let org: String = row.get(6)?;
    let tags: Vec<String> = serde_json::from_str(&tags_json).unwrap_or_default();
    Ok(EmailLink {
        message_id,
        entity: EntityRef::new(EntityKind::new(kind), id).in_org(org),
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
    fn the_same_project_id_in_two_orgs_is_two_different_things() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LinkStore::open(dir.path()).unwrap();

        // Ids here are UUIDs and vault paths, so a collision across orgs
        // is unlikely — but "unlikely" is not the reason to get this
        // right. A link that answered for the wrong org would put one
        // person's mail on another org's project, which is the failure
        // this whole column exists to prevent.
        let theirs = EntityRef::project("proj-1").in_org("tombrooksmusic");
        let mine = EntityRef::project("proj-1").in_org("codywright");

        store
            .upsert(&link("<a@example.com>", theirs.clone()))
            .unwrap();
        store
            .upsert(&link("<b@example.com>", mine.clone()))
            .unwrap();

        let for_theirs = store.links_for_entity(&theirs).unwrap();
        assert_eq!(for_theirs.len(), 1);
        assert_eq!(for_theirs[0].message_id, "a@example.com");

        let for_mine = store.links_for_entity(&mine).unwrap();
        assert_eq!(for_mine.len(), 1);
        assert_eq!(for_mine[0].message_id, "b@example.com");
    }

    #[test]
    fn the_org_survives_the_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = LinkStore::open(dir.path()).unwrap();
        let target = EntityRef::task("t-9").in_org("tombrooksmusic");
        store
            .upsert(&link("<m@example.com>", target.clone()))
            .unwrap();

        // Read back the other way — by message — because that is the
        // path that has to reconstruct the org from the row rather than
        // being handed it by the caller.
        let rows = store.links_for_message("<m@example.com>").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].entity.org, "tombrooksmusic");
        assert_eq!(rows[0].entity.id, "t-9");
    }

    #[test]
    fn a_database_from_before_the_column_still_opens() {
        // The old schema, exactly as it shipped. `CREATE TABLE IF NOT
        // EXISTS` would leave this untouched, so without the migration
        // every query naming entity_org would fail against a real
        // deployment's existing file.
        let dir = tempfile::tempdir().unwrap();
        {
            let conn = Connection::open(dir.path().join("links.db")).unwrap();
            conn.execute_batch(
                "CREATE TABLE email_links (
                    message_id  TEXT NOT NULL,
                    entity_kind TEXT NOT NULL,
                    entity_id   TEXT NOT NULL,
                    linked_at   INTEGER,
                    linked_by   TEXT,
                    user_tags   TEXT NOT NULL DEFAULT '[]',
                    PRIMARY KEY (message_id, entity_kind, entity_id)
                 );
                 INSERT INTO email_links (message_id, entity_kind, entity_id, user_tags)
                 VALUES ('old@example.com', 'task', 't-1', '[]');",
            )
            .unwrap();
        }

        let store = LinkStore::open(dir.path()).unwrap();
        let rows = store.links_for_message("<old@example.com>").unwrap();
        assert_eq!(rows.len(), 1);
        // Empty is the honest reading of a row written when a link could
        // only mean something in this store's own org.
        assert_eq!(rows[0].entity.org, "");
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
