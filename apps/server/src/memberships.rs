//! Which orgs a principal belongs to on this server, and with what role.
//!
//! One row per `(user_id, org_slug)`. The user id is the HOME org's user
//! id — the home org's `auth.sqlite` is this server's identity authority
//! (see `apps/task/plans/one-account-per-server.md`), so a principal is
//! "a user in the home org, plus the orgs it has rows for".
//!
//! ## Why this table exists at all
//!
//! Membership used to be a side effect of which database answered:
//! `AppState` opens one `AuthState` per org, so "you are a member here"
//! meant "this org's store validated your token", and the permission
//! gate then mapped any validated user to `member` via
//! `default_user_role`. That is why `.well-known` could only report
//! membership for the one org that issued the token, and why "All
//! organizations" in the client collapsed to the home org.
//!
//! With this table membership is an explicit, per-org fact carrying its
//! own role, so one account can be an owner in one org and a reader in
//! another.
//!
//! ## The fence
//!
//! After the org lane learns to accept home-issued tokens, a row here is
//! the ONLY thing between an org's data and any valid home token.
//! `role_for` returning `None` must therefore be a refusal, never a
//! fallback to a default role — the absence of a row is the answer.

use eyre::{Context as _, Result};
use sea_orm::{ConnectionTrait as _, Database, DatabaseBackend, DatabaseConnection, Statement};
use std::path::Path;
use uuid::Uuid;

/// One org a principal belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub org_slug: String,
    /// The role this principal holds in THIS org — `admin`, `member`,
    /// whatever the org's own account carried when it was adopted.
    /// `None` is a member with no elevated role, matching
    /// `architect_auth`'s own `Option<String>` role column.
    pub role: Option<String>,
}

/// The memberships table, opened against one file.
pub struct Memberships {
    conn: DatabaseConnection,
}

impl Memberships {
    /// Open (creating the file and table if absent).
    ///
    /// Creating on open rather than through `sea-orm-migration`: this is
    /// one table in its own file with no history to migrate, and the
    /// server must come up clean on a data root that predates it.
    pub async fn open(path: &Path) -> Result<Self> {
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let conn = Database::connect(&url)
            .await
            .wrap_err_with(|| format!("open memberships store at {}", path.display()))?;
        conn.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "CREATE TABLE IF NOT EXISTS memberships (
                 user_id    TEXT NOT NULL,
                 org_slug   TEXT NOT NULL,
                 role       TEXT,
                 created_at INTEGER NOT NULL,
                 PRIMARY KEY (user_id, org_slug)
             )"
            .to_owned(),
        ))
        .await
        .wrap_err("create memberships table")?;
        Ok(Self { conn })
    }

    /// Open read-only — for reporting beside a live server.
    pub async fn open_ro(path: &Path) -> Result<Self> {
        let url = format!("sqlite://{}?mode=ro", path.display());
        let conn = Database::connect(&url)
            .await
            .wrap_err_with(|| format!("open memberships store (ro) at {}", path.display()))?;
        Ok(Self { conn })
    }

    /// Add or update one membership. Idempotent on `(user_id, org_slug)`
    /// so re-running the adopt command is how a role change is applied
    /// — nothing else reads the org's own role column afterwards.
    pub async fn upsert(&self, user_id: Uuid, org_slug: &str, role: Option<&str>) -> Result<()> {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0));
        let role_sql = role.map_or_else(|| "NULL".to_owned(), |r| format!("'{}'", esc(r)));
        self.conn
            .execute(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!(
                    "INSERT INTO memberships (user_id, org_slug, role, created_at)
                     VALUES ('{user_id}', '{}', {role_sql}, {now})
                     ON CONFLICT(user_id, org_slug) DO UPDATE SET role = excluded.role",
                    esc(org_slug)
                ),
            ))
            .await
            .wrap_err_with(|| format!("upsert membership {user_id} in `{org_slug}`"))?;
        Ok(())
    }

    /// Every org this principal belongs to, slug order.
    pub async fn for_user(&self, user_id: Uuid) -> Result<Vec<Membership>> {
        let rows = self
            .conn
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!(
                    "SELECT org_slug, role FROM memberships
                     WHERE user_id = '{user_id}' ORDER BY org_slug"
                ),
            ))
            .await
            .wrap_err("list memberships")?;
        rows.into_iter()
            .map(|r| {
                Ok(Membership {
                    org_slug: r.try_get("", "org_slug")?,
                    role: r.try_get("", "role")?,
                })
            })
            .collect()
    }

    /// This principal's role in one org, or `None` when there is no row.
    ///
    /// `None` means NOT A MEMBER. Callers must refuse on it; treating it
    /// as "member with the default role" would hand every org's data to
    /// anyone holding a valid home token.
    pub async fn role_for(&self, user_id: Uuid, org_slug: &str) -> Result<Option<Membership>> {
        let row = self
            .conn
            .query_one(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!(
                    "SELECT org_slug, role FROM memberships
                     WHERE user_id = '{user_id}' AND org_slug = '{}'",
                    esc(org_slug)
                ),
            ))
            .await
            .wrap_err("read membership")?;
        row.map(|r| {
            Ok(Membership {
                org_slug: r.try_get("", "org_slug")?,
                role: r.try_get("", "role")?,
            })
        })
        .transpose()
    }

    /// Remove a membership — the revoke path, and the rollback for an
    /// adopt that named the wrong org.
    pub async fn revoke(&self, user_id: Uuid, org_slug: &str) -> Result<()> {
        self.conn
            .execute(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!(
                    "DELETE FROM memberships WHERE user_id = '{user_id}' AND org_slug = '{}'",
                    esc(org_slug)
                ),
            ))
            .await
            .wrap_err("revoke membership")?;
        Ok(())
    }
}

/// Single-quote escaping for the string literals above. Slugs and roles
/// are operator-supplied, not user-supplied, but a slug with an
/// apostrophe would otherwise produce a syntax error at best.
fn esc(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn store() -> (tempfile::TempDir, Memberships) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("memberships.sqlite");
        let m = Memberships::open(&path).await.unwrap();
        (dir, m)
    }

    #[tokio::test]
    async fn a_principal_holds_a_different_role_in_each_org() {
        let (_d, m) = store().await;
        let cody = Uuid::new_v4();
        m.upsert(cody, "codywright", Some("admin")).await.unwrap();
        m.upsert(cody, "cbu", Some("member")).await.unwrap();
        m.upsert(cody, "days-to-praise", None).await.unwrap();

        let all = m.for_user(cody).await.unwrap();
        assert_eq!(all.len(), 3);
        assert_eq!(
            m.role_for(cody, "codywright").await.unwrap().unwrap().role,
            Some("admin".into())
        );
        assert_eq!(
            m.role_for(cody, "cbu").await.unwrap().unwrap().role,
            Some("member".into())
        );
        // A row with no role is still a member — absence of a ROW and
        // absence of a ROLE are different answers.
        assert!(m.role_for(cody, "days-to-praise").await.unwrap().is_some());
        assert_eq!(
            m.role_for(cody, "days-to-praise")
                .await
                .unwrap()
                .unwrap()
                .role,
            None
        );
    }

    #[tokio::test]
    async fn no_row_is_not_a_member() {
        let (_d, m) = store().await;
        let stranger = Uuid::new_v4();
        assert!(m.role_for(stranger, "codywright").await.unwrap().is_none());
        assert!(m.for_user(stranger).await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn adopting_twice_updates_the_role_instead_of_duplicating() {
        // Re-running `adopt-principal` is the only way a role changes
        // once the org's own role column stops being read.
        let (_d, m) = store().await;
        let cody = Uuid::new_v4();
        m.upsert(cody, "cbu", Some("member")).await.unwrap();
        m.upsert(cody, "cbu", Some("admin")).await.unwrap();
        let all = m.for_user(cody).await.unwrap();
        assert_eq!(all.len(), 1, "one row per (user, org)");
        assert_eq!(all[0].role, Some("admin".into()));
    }

    #[tokio::test]
    async fn revoking_removes_only_that_org() {
        let (_d, m) = store().await;
        let cody = Uuid::new_v4();
        m.upsert(cody, "cbu", Some("admin")).await.unwrap();
        m.upsert(cody, "codywright", Some("admin")).await.unwrap();
        m.revoke(cody, "cbu").await.unwrap();
        let all = m.for_user(cody).await.unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].org_slug, "codywright");
    }

    #[tokio::test]
    async fn two_principals_do_not_see_each_others_rows() {
        let (_d, m) = store().await;
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        m.upsert(a, "cbu", Some("admin")).await.unwrap();
        m.upsert(b, "cbu", Some("member")).await.unwrap();
        assert_eq!(m.for_user(a).await.unwrap().len(), 1);
        assert_eq!(
            m.role_for(b, "cbu").await.unwrap().unwrap().role,
            Some("member".into())
        );
    }
}
