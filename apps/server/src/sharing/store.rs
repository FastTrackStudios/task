//! The owning org's Sharing Grant rows: `<org>/sharing.sqlite`.
//!
//! One table of grants and one append-only table of grant events
//! (`grant.created`, `grant.revoked`, `grant.expired`). Opened and
//! created on boot the way [`crate::memberships::Memberships`] is — one
//! small table set in its own file, no history to migrate, and a server
//! must come up clean on a data root that predates it.
//!
//! ## Immutable except for revoke
//!
//! A grant row is written once. The only change it ever takes is the
//! single revoke, which sets `revoked_at` and `revoked_by` together and
//! never clears them. That is not only a convention of this module's
//! API — which has no update — but enforced by the database itself: a
//! `BEFORE UPDATE` trigger aborts any write that touches another column
//! or touches a row already revoked, and a `BEFORE DELETE` trigger
//! aborts every delete. A change of scope is therefore a revoke and a
//! fresh grant, and the history of who could see what is never
//! rewritten.

use eyre::{Context as _, Result};
use sea_orm::{
    ConnectionTrait as _, Database, DatabaseBackend, DatabaseConnection, QueryResult, Statement,
    Value,
};
use share_proto::{GrantCapabilities, SharingGrantInfo};
use std::path::Path;
use uuid::Uuid;

/// One grant, as stored (D1). The subject holds no membership row in
/// the owner org; this row is the whole of their access.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharingGrant {
    pub id: Uuid,
    /// The owning org's slug — the org whose store this row lives in.
    pub owner_org: String,
    /// The project whose subtree is shared.
    pub root_id: Uuid,
    /// The person (fts-auth user uuid).
    pub subject: Uuid,
    /// The subject's own org, as metadata.
    pub guest_org: String,
    pub capabilities: GrantCapabilities,
    /// Unix seconds; `None` never expires.
    pub expires_at: Option<i64>,
    pub granted_by: Uuid,
    pub granted_at: i64,
    pub revoked_at: Option<i64>,
    pub revoked_by: Option<Uuid>,
    pub preview_digest: String,
}

/// Is `grant` in force at `now` (unix seconds)?
///
/// Unrevoked, and either without an expiry or not yet at it. The grant
/// lane asks this on every call, so it is the one definition of "live":
/// an expiry at exactly `now` is already past.
#[must_use]
pub fn is_live(grant: &SharingGrant, now: i64) -> bool {
    grant.revoked_at.is_none() && grant.expires_at.is_none_or(|at| now < at)
}

impl SharingGrant {
    /// Expired (and not revoked first) at `now`.
    #[must_use]
    pub fn expired(&self, now: i64) -> bool {
        self.revoked_at.is_none() && self.expires_at.is_some_and(|at| now >= at)
    }

    /// The wire shape, with liveness evaluated at `now`.
    #[must_use]
    pub fn info(&self, now: i64) -> SharingGrantInfo {
        SharingGrantInfo {
            id: self.id,
            owner_org: self.owner_org.clone(),
            root_id: self.root_id,
            subject: self.subject,
            guest_org: self.guest_org.clone(),
            capabilities: self.capabilities,
            expires_at: self.expires_at,
            granted_by: self.granted_by,
            granted_at: self.granted_at,
            revoked_at: self.revoked_at,
            revoked_by: self.revoked_by,
            preview_digest: self.preview_digest.clone(),
            live: is_live(self, now),
        }
    }
}

/// What an audit line is about.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GrantEventKind {
    Created,
    Revoked,
    Expired,
}

impl GrantEventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "grant.created",
            Self::Revoked => "grant.revoked",
            Self::Expired => "grant.expired",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "grant.created" => Some(Self::Created),
            "grant.revoked" => Some(Self::Revoked),
            "grant.expired" => Some(Self::Expired),
            _ => None,
        }
    }
}

/// One audit row: what happened to which grant, and who did it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GrantEvent {
    pub grant_id: Uuid,
    pub kind: GrantEventKind,
    pub at: i64,
    /// The person who acted; `None` for an expiry, which nobody does.
    pub actor: Option<Uuid>,
    pub subject: Uuid,
    pub root_id: Uuid,
    pub capabilities: GrantCapabilities,
    pub preview_digest: String,
}

impl GrantEvent {
    #[must_use]
    pub fn of(grant: &SharingGrant, kind: GrantEventKind, at: i64, actor: Option<Uuid>) -> Self {
        Self {
            grant_id: grant.id,
            kind,
            at,
            actor,
            subject: grant.subject,
            root_id: grant.root_id,
            capabilities: grant.capabilities,
            preview_digest: grant.preview_digest.clone(),
        }
    }
}

/// The result of [`GrantStore::revoke`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Revoked {
    /// The row as it now stands.
    pub grant: SharingGrant,
    /// `true` only for the call that actually revoked it.
    pub newly: bool,
}

/// One org's grant rows.
pub struct GrantStore {
    conn: DatabaseConnection,
    owner_org: String,
}

const SCHEMA: &[&str] = &[
    "CREATE TABLE IF NOT EXISTS sharing_grants (
         id             TEXT PRIMARY KEY NOT NULL,
         owner_org      TEXT NOT NULL,
         root_id        TEXT NOT NULL,
         subject        TEXT NOT NULL,
         guest_org      TEXT NOT NULL,
         capabilities   TEXT NOT NULL,
         expires_at     INTEGER,
         granted_by     TEXT NOT NULL,
         granted_at     INTEGER NOT NULL,
         revoked_at     INTEGER,
         revoked_by     TEXT,
         preview_digest TEXT NOT NULL
     )",
    "CREATE INDEX IF NOT EXISTS sharing_grants_subject ON sharing_grants (subject, root_id)",
    // The revoke is the one write a row ever takes after its insert, and
    // it happens once: both revoke columns set, nothing else touched, on
    // a row not already revoked. Anything else aborts.
    "CREATE TRIGGER IF NOT EXISTS sharing_grants_immutable
     BEFORE UPDATE ON sharing_grants
     WHEN OLD.revoked_at IS NOT NULL
       OR NEW.revoked_at IS NULL
       OR NEW.revoked_by IS NULL
       OR NEW.id IS NOT OLD.id
       OR NEW.owner_org IS NOT OLD.owner_org
       OR NEW.root_id IS NOT OLD.root_id
       OR NEW.subject IS NOT OLD.subject
       OR NEW.guest_org IS NOT OLD.guest_org
       OR NEW.capabilities IS NOT OLD.capabilities
       OR NEW.expires_at IS NOT OLD.expires_at
       OR NEW.granted_by IS NOT OLD.granted_by
       OR NEW.granted_at IS NOT OLD.granted_at
       OR NEW.preview_digest IS NOT OLD.preview_digest
     BEGIN
         SELECT RAISE(ABORT, 'sharing grants are immutable except for revoke');
     END",
    "CREATE TRIGGER IF NOT EXISTS sharing_grants_kept
     BEFORE DELETE ON sharing_grants
     BEGIN
         SELECT RAISE(ABORT, 'sharing grants are never deleted; revoke instead');
     END",
    // Each kind happens to a grant at most once, which is what makes a
    // repeated revoke (or a re-observed expiry) write no second line.
    "CREATE TABLE IF NOT EXISTS sharing_grant_events (
         seq            INTEGER PRIMARY KEY AUTOINCREMENT,
         grant_id       TEXT NOT NULL,
         kind           TEXT NOT NULL,
         at             INTEGER NOT NULL,
         actor          TEXT,
         subject        TEXT NOT NULL,
         root_id        TEXT NOT NULL,
         capabilities   TEXT NOT NULL,
         preview_digest TEXT NOT NULL,
         UNIQUE (grant_id, kind)
     )",
];

const COLUMNS: &str = "id, owner_org, root_id, subject, guest_org, capabilities, expires_at, \
                       granted_by, granted_at, revoked_at, revoked_by, preview_digest";

fn stmt(sql: &str, values: Vec<Value>) -> Statement {
    Statement::from_sql_and_values(DatabaseBackend::Sqlite, sql, values)
}

fn uuid_col(row: &QueryResult, col: &str) -> Result<Uuid> {
    let s: String = row.try_get("", col)?;
    Uuid::parse_str(&s).wrap_err_with(|| format!("sharing_grants.{col} is not a uuid: {s}"))
}

fn grant_from(row: &QueryResult) -> Result<SharingGrant> {
    let caps: String = row.try_get("", "capabilities")?;
    let revoked_by: Option<String> = row.try_get("", "revoked_by")?;
    Ok(SharingGrant {
        id: uuid_col(row, "id")?,
        owner_org: row.try_get("", "owner_org")?,
        root_id: uuid_col(row, "root_id")?,
        subject: uuid_col(row, "subject")?,
        guest_org: row.try_get("", "guest_org")?,
        capabilities: GrantCapabilities::parse(&caps)
            .ok_or_else(|| eyre::eyre!("unknown grant capabilities `{caps}`"))?,
        expires_at: row.try_get("", "expires_at")?,
        granted_by: uuid_col(row, "granted_by")?,
        granted_at: row.try_get("", "granted_at")?,
        revoked_at: row.try_get("", "revoked_at")?,
        revoked_by: revoked_by
            .map(|s| Uuid::parse_str(&s))
            .transpose()
            .wrap_err("sharing_grants.revoked_by is not a uuid")?,
        preview_digest: row.try_get("", "preview_digest")?,
    })
}

impl GrantStore {
    /// Open `<org>/sharing.sqlite` for `owner_org`, creating the tables
    /// and triggers if absent.
    pub async fn open(path: &Path, owner_org: &str) -> Result<Self> {
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let conn = Database::connect(&url)
            .await
            .wrap_err_with(|| format!("open sharing store at {}", path.display()))?;
        for sql in SCHEMA {
            conn.execute(Statement::from_string(DatabaseBackend::Sqlite, *sql))
                .await
                .wrap_err("create sharing tables")?;
        }
        Ok(Self {
            conn,
            owner_org: owner_org.to_owned(),
        })
    }

    /// The pool, for the snapshot engine's checkpoint pass.
    #[must_use]
    pub fn conn(&self) -> &DatabaseConnection {
        &self.conn
    }

    #[must_use]
    pub fn owner_org(&self) -> &str {
        &self.owner_org
    }

    /// Write a new grant and its `grant.created` line together.
    pub async fn insert(&self, grant: &SharingGrant) -> Result<()> {
        use sea_orm::TransactionTrait as _;
        eyre::ensure!(
            grant.owner_org == self.owner_org,
            "a grant of `{}` cannot live in `{}`'s store",
            grant.owner_org,
            self.owner_org
        );
        eyre::ensure!(
            grant.revoked_at.is_none() && grant.revoked_by.is_none(),
            "a grant is born unrevoked"
        );
        let txn = self.conn.begin().await.wrap_err("begin grant insert")?;
        txn.execute(stmt(
            &format!(
                "INSERT INTO sharing_grants ({COLUMNS}) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?)"
            ),
            vec![
                grant.id.to_string().into(),
                grant.owner_org.clone().into(),
                grant.root_id.to_string().into(),
                grant.subject.to_string().into(),
                grant.guest_org.clone().into(),
                grant.capabilities.as_str().into(),
                grant.expires_at.into(),
                grant.granted_by.to_string().into(),
                grant.granted_at.into(),
                grant.preview_digest.clone().into(),
            ],
        ))
        .await
        .wrap_err("insert grant")?;
        insert_event(
            &txn,
            &GrantEvent::of(
                grant,
                GrantEventKind::Created,
                grant.granted_at,
                Some(grant.granted_by),
            ),
        )
        .await?;
        txn.commit().await.wrap_err("commit grant insert")?;
        Ok(())
    }

    pub async fn get(&self, id: Uuid) -> Result<Option<SharingGrant>> {
        let row = self
            .conn
            .query_one(stmt(
                &format!("SELECT {COLUMNS} FROM sharing_grants WHERE id = ?"),
                vec![id.to_string().into()],
            ))
            .await
            .wrap_err("read grant")?;
        row.as_ref().map(grant_from).transpose()
    }

    /// Every grant, newest first.
    pub async fn list(&self) -> Result<Vec<SharingGrant>> {
        let rows = self
            .conn
            .query_all(stmt(
                &format!("SELECT {COLUMNS} FROM sharing_grants ORDER BY granted_at DESC, id"),
                vec![],
            ))
            .await
            .wrap_err("list grants")?;
        rows.iter().map(grant_from).collect()
    }

    /// The grants naming `subject` on `root_id`, live or not. The grant
    /// lane re-reads this on every call and asks [`is_live`] of it.
    pub async fn for_subject(&self, subject: Uuid, root_id: Uuid) -> Result<Vec<SharingGrant>> {
        let rows = self
            .conn
            .query_all(stmt(
                &format!(
                    "SELECT {COLUMNS} FROM sharing_grants \
                     WHERE subject = ? AND root_id = ? ORDER BY granted_at DESC, id"
                ),
                vec![subject.to_string().into(), root_id.to_string().into()],
            ))
            .await
            .wrap_err("read grants for subject")?;
        rows.iter().map(grant_from).collect()
    }

    /// Revoke `id` as `by` at `now`. Idempotent: a grant already revoked
    /// comes back as it was, with `newly == false`, and no second line
    /// is written. `None` when there is no such grant.
    pub async fn revoke(&self, id: Uuid, by: Uuid, now: i64) -> Result<Option<Revoked>> {
        use sea_orm::TransactionTrait as _;
        let txn = self.conn.begin().await.wrap_err("begin revoke")?;
        let done = txn
            .execute(stmt(
                "UPDATE sharing_grants SET revoked_at = ?, revoked_by = ? \
                 WHERE id = ? AND revoked_at IS NULL",
                vec![now.into(), by.to_string().into(), id.to_string().into()],
            ))
            .await
            .wrap_err("revoke grant")?
            .rows_affected()
            == 1;
        let row = txn
            .query_one(stmt(
                &format!("SELECT {COLUMNS} FROM sharing_grants WHERE id = ?"),
                vec![id.to_string().into()],
            ))
            .await
            .wrap_err("read revoked grant")?;
        let Some(grant) = row.as_ref().map(grant_from).transpose()? else {
            txn.rollback().await.ok();
            return Ok(None);
        };
        if done {
            insert_event(
                &txn,
                &GrantEvent::of(&grant, GrantEventKind::Revoked, now, Some(by)),
            )
            .await?;
        }
        txn.commit().await.wrap_err("commit revoke")?;
        Ok(Some(Revoked { grant, newly: done }))
    }

    /// Record `grant.expired` for `grant` if it has expired by `now` and
    /// no line says so yet. `Some(event)` only for the call that wrote
    /// it, so the audit line is emitted once however often the expiry is
    /// observed.
    pub async fn note_expired(&self, grant: &SharingGrant, now: i64) -> Result<Option<GrantEvent>> {
        if !grant.expired(now) {
            return Ok(None);
        }
        let event = GrantEvent::of(grant, GrantEventKind::Expired, now, None);
        let written = insert_event(&self.conn, &event).await?;
        Ok(written.then_some(event))
    }

    /// The audit lines for one grant, oldest first.
    pub async fn events(&self, grant_id: Uuid) -> Result<Vec<GrantEvent>> {
        let rows = self
            .conn
            .query_all(stmt(
                "SELECT grant_id, kind, at, actor, subject, root_id, capabilities, preview_digest \
                 FROM sharing_grant_events WHERE grant_id = ? ORDER BY seq",
                vec![grant_id.to_string().into()],
            ))
            .await
            .wrap_err("read grant events")?;
        rows.iter()
            .map(|r| {
                let kind: String = r.try_get("", "kind")?;
                let caps: String = r.try_get("", "capabilities")?;
                let actor: Option<String> = r.try_get("", "actor")?;
                Ok(GrantEvent {
                    grant_id: uuid_col(r, "grant_id")?,
                    kind: GrantEventKind::parse(&kind)
                        .ok_or_else(|| eyre::eyre!("unknown grant event `{kind}`"))?,
                    at: r.try_get("", "at")?,
                    actor: actor.map(|a| Uuid::parse_str(&a)).transpose()?,
                    subject: uuid_col(r, "subject")?,
                    root_id: uuid_col(r, "root_id")?,
                    capabilities: GrantCapabilities::parse(&caps)
                        .ok_or_else(|| eyre::eyre!("unknown grant capabilities `{caps}`"))?,
                    preview_digest: r.try_get("", "preview_digest")?,
                })
            })
            .collect()
    }

    /// Run raw SQL against the store — for tests that prove the database
    /// itself refuses what the API never offers.
    #[cfg(test)]
    pub(crate) async fn raw(&self, sql: &str) -> Result<u64> {
        Ok(self
            .conn
            .execute(Statement::from_string(DatabaseBackend::Sqlite, sql))
            .await?
            .rows_affected())
    }
}

/// Append one event; `false` when that kind was already recorded for
/// the grant.
async fn insert_event<C: sea_orm::ConnectionTrait>(conn: &C, e: &GrantEvent) -> Result<bool> {
    let n = conn
        .execute(stmt(
            "INSERT OR IGNORE INTO sharing_grant_events \
             (grant_id, kind, at, actor, subject, root_id, capabilities, preview_digest) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
            vec![
                e.grant_id.to_string().into(),
                e.kind.as_str().into(),
                e.at.into(),
                e.actor.map(|a| a.to_string()).into(),
                e.subject.to_string().into(),
                e.root_id.to_string().into(),
                e.capabilities.as_str().into(),
                e.preview_digest.clone().into(),
            ],
        ))
        .await
        .wrap_err_with(|| format!("record {}", e.kind.as_str()))?
        .rows_affected();
    Ok(n == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(owner: &str, expires_at: Option<i64>) -> SharingGrant {
        SharingGrant {
            id: Uuid::new_v4(),
            owner_org: owner.into(),
            root_id: Uuid::new_v4(),
            subject: Uuid::new_v4(),
            guest_org: "rockstars-of-tomorrow".into(),
            capabilities: GrantCapabilities::ReadComment,
            expires_at,
            granted_by: Uuid::new_v4(),
            granted_at: 1_000,
            revoked_at: None,
            revoked_by: None,
            preview_digest: "d".repeat(64),
        }
    }

    async fn store() -> (GrantStore, tempfile::TempDir) {
        let tmp = tempfile::tempdir().unwrap();
        let s = GrantStore::open(&tmp.path().join("sharing.sqlite"), "fta")
            .await
            .unwrap();
        (s, tmp)
    }

    #[test]
    fn is_live_is_unrevoked_and_unexpired() {
        let mut g = grant("fta", None);
        assert!(is_live(&g, i64::MAX), "no expiry never lapses");
        g.expires_at = Some(2_000);
        assert!(is_live(&g, 1_999));
        assert!(!is_live(&g, 2_000), "an expiry at now is already past");
        assert!(!is_live(&g, 2_001));
        assert!(g.expired(2_000));
        g.expires_at = None;
        g.revoked_at = Some(1_500);
        g.revoked_by = Some(Uuid::new_v4());
        assert!(
            !is_live(&g, 1_000),
            "revoked is not live, whatever the clock"
        );
        assert!(
            !g.expired(i64::MAX),
            "a revoked grant is revoked, not expired"
        );
    }

    #[tokio::test]
    async fn a_grant_round_trips_and_lists_newest_first() {
        let (s, _tmp) = store().await;
        let older = grant("fta", None);
        let mut newer = grant("fta", Some(9_999));
        newer.granted_at = 2_000;
        s.insert(&older).await.unwrap();
        s.insert(&newer).await.unwrap();
        assert_eq!(s.get(older.id).await.unwrap(), Some(older.clone()));
        let all = s.list().await.unwrap();
        assert_eq!(all, vec![newer.clone(), older.clone()]);
        assert_eq!(
            s.for_subject(older.subject, older.root_id).await.unwrap(),
            vec![older.clone()]
        );
        assert!(
            s.for_subject(older.subject, newer.root_id)
                .await
                .unwrap()
                .is_empty()
        );
        let created = s.events(older.id).await.unwrap();
        assert_eq!(created.len(), 1);
        assert_eq!(created[0].kind, GrantEventKind::Created);
        assert_eq!(created[0].actor, Some(older.granted_by));
        assert_eq!(created[0].preview_digest, older.preview_digest);
    }

    #[tokio::test]
    async fn a_grant_of_another_org_does_not_land_here() {
        let (s, _tmp) = store().await;
        assert!(s.insert(&grant("rot", None)).await.is_err());
    }

    #[tokio::test]
    async fn revoke_is_idempotent_and_audited_once() {
        let (s, _tmp) = store().await;
        let g = grant("fta", None);
        s.insert(&g).await.unwrap();
        let by = Uuid::new_v4();
        let first = s.revoke(g.id, by, 3_000).await.unwrap().unwrap();
        assert!(first.newly);
        assert_eq!(first.grant.revoked_at, Some(3_000));
        assert_eq!(first.grant.revoked_by, Some(by));
        let again = s
            .revoke(g.id, Uuid::new_v4(), 4_000)
            .await
            .unwrap()
            .unwrap();
        assert!(!again.newly, "the second revoke changes nothing");
        assert_eq!(again.grant, first.grant, "first revoke's stamp stands");
        let kinds: Vec<_> = s
            .events(g.id)
            .await
            .unwrap()
            .into_iter()
            .map(|e| e.kind)
            .collect();
        assert_eq!(
            kinds,
            vec![GrantEventKind::Created, GrantEventKind::Revoked]
        );
        assert!(s.revoke(Uuid::new_v4(), by, 3_000).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_database_refuses_every_mutation_but_the_revoke() {
        let (s, _tmp) = store().await;
        let g = grant("fta", None);
        s.insert(&g).await.unwrap();
        let id = g.id;
        for sql in [
            format!(
                "UPDATE sharing_grants SET root_id = '{}' WHERE id = '{id}'",
                Uuid::new_v4()
            ),
            format!(
                "UPDATE sharing_grants SET subject = '{}' WHERE id = '{id}'",
                Uuid::new_v4()
            ),
            format!("UPDATE sharing_grants SET expires_at = 1 WHERE id = '{id}'"),
            format!("UPDATE sharing_grants SET capabilities = 'read_write' WHERE id = '{id}'"),
            format!("UPDATE sharing_grants SET preview_digest = 'x' WHERE id = '{id}'"),
            format!("UPDATE sharing_grants SET guest_org = 'elsewhere' WHERE id = '{id}'"),
            // Half a revoke is not a revoke.
            format!("UPDATE sharing_grants SET revoked_at = 5 WHERE id = '{id}'"),
            format!("DELETE FROM sharing_grants WHERE id = '{id}'"),
        ] {
            assert!(s.raw(&sql).await.is_err(), "allowed: {sql}");
        }
        assert_eq!(s.get(id).await.unwrap(), Some(g.clone()), "row untouched");

        let by = Uuid::new_v4();
        s.revoke(id, by, 3_000).await.unwrap().unwrap();
        for sql in [
            format!(
                "UPDATE sharing_grants SET revoked_at = NULL, revoked_by = NULL WHERE id = '{id}'"
            ),
            format!(
                "UPDATE sharing_grants SET revoked_at = 9, revoked_by = '{by}' WHERE id = '{id}'"
            ),
            format!("DELETE FROM sharing_grants WHERE id = '{id}'"),
        ] {
            assert!(s.raw(&sql).await.is_err(), "allowed after revoke: {sql}");
        }
        assert_eq!(s.get(id).await.unwrap().unwrap().revoked_at, Some(3_000));
    }

    #[tokio::test]
    async fn an_expiry_is_noted_once() {
        let (s, _tmp) = store().await;
        let g = grant("fta", Some(2_000));
        s.insert(&g).await.unwrap();
        assert!(
            s.note_expired(&g, 1_999).await.unwrap().is_none(),
            "not yet"
        );
        let e = s
            .note_expired(&g, 2_000)
            .await
            .unwrap()
            .expect("expired now");
        assert_eq!(e.kind, GrantEventKind::Expired);
        assert_eq!(e.actor, None);
        assert!(s.note_expired(&g, 2_500).await.unwrap().is_none(), "once");
        assert_eq!(s.events(g.id).await.unwrap().len(), 2);
    }
}
