//! Which orgs a principal belongs to on this server, and with what role.
//!
//! One row per `(user_id, org_slug)`. The user id names a principal and
//! nothing else — on a server with no central issuer that is the HOME
//! org's user id (the home org's `auth.sqlite` is then this server's
//! identity authority), and on a server that delegates identity it is
//! the ISSUER's user id. Either way a principal is "an id, plus the orgs
//! it has rows for"; this table never asks which store minted the id.
//!
//! ## Invites: one account, no shadow accounts
//!
//! The id above is the problem an operator actually hits. Membership is
//! keyed to a principal, but an operator granting access knows an
//! EMAIL — and under central auth the principal is a uuid only the
//! issuer can mint, which this server first learns when that person
//! signs in. The original answer was to create a local account in every
//! org so `adopt-principal` had somewhere to read an id out of, which
//! meant one shadow account per person per org: N accounts pretending
//! to be one.
//!
//! The `invites` table removes them. An operator writes
//! `(email, org_slug, role)` with no principal at all; the first time a
//! token resolves to that address the invite becomes a membership keyed
//! to the real principal and is consumed. One account at the issuer,
//! many orgs, nothing local to keep in step.
//!
//! `adopt-principal` remains the migration path for servers that
//! already grew those shadow accounts — it reads them and writes the
//! rows this table wants. Nothing new needs it.
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
        conn.execute(Statement::from_string(
            DatabaseBackend::Sqlite,
            "CREATE TABLE IF NOT EXISTS invites (
                 email      TEXT NOT NULL,
                 org_slug   TEXT NOT NULL,
                 role       TEXT,
                 created_at INTEGER NOT NULL,
                 PRIMARY KEY (email, org_slug)
             )"
            .to_owned(),
        ))
        .await
        .wrap_err("create invites table")?;
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
        let now = now_unix();
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

    /// Every membership row on this server, `(user_id, org_slug)` order.
    ///
    /// Reads the table and nothing else. The older listing walked the
    /// home org's auth store and printed a row per LOCAL account, which
    /// made a principal that has no local account — now the ordinary
    /// case — invisible to the operator inspecting their own server.
    pub async fn all(&self) -> Result<Vec<(Uuid, Membership)>> {
        let rows = self
            .conn
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT user_id, org_slug, role FROM memberships
                 ORDER BY user_id, org_slug"
                    .to_owned(),
            ))
            .await
            .wrap_err("list all memberships")?;
        rows.into_iter()
            .map(|r| {
                let raw: String = r.try_get("", "user_id")?;
                let user_id = raw
                    .parse::<Uuid>()
                    .wrap_err_with(|| format!("membership row has a non-uuid user_id `{raw}`"))?;
                Ok((
                    user_id,
                    Membership {
                        org_slug: r.try_get("", "org_slug")?,
                        role: r.try_get("", "role")?,
                    },
                ))
            })
            .collect()
    }

    /// Promise an org to an address nobody has resolved yet.
    ///
    /// The whole point of the invites table: an operator naming a person
    /// knows their EMAIL, and the fence is keyed to a PRINCIPAL — a uuid
    /// only the issuer can mint and only that person's first sign-in
    /// reveals to this server. Requiring the operator to carry the uuid
    /// across by hand is what forced a local shadow account per org
    /// (something for `adopt-principal` to read an id out of). An invite
    /// closes that gap without the shadow: write the address now, and
    /// the principal binds itself on arrival.
    ///
    /// Idempotent on `(email, org_slug)`, so re-inviting changes the
    /// pending role rather than erroring.
    pub async fn invite(&self, email: &str, org_slug: &str, role: Option<&str>) -> Result<()> {
        let email = normalize_email(email);
        let now = now_unix();
        let role_sql = role.map_or_else(|| "NULL".to_owned(), |r| format!("'{}'", esc(r)));
        self.conn
            .execute(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!(
                    "INSERT INTO invites (email, org_slug, role, created_at)
                     VALUES ('{}', '{}', {role_sql}, {now})
                     ON CONFLICT(email, org_slug) DO UPDATE SET role = excluded.role",
                    esc(&email),
                    esc(org_slug)
                ),
            ))
            .await
            .wrap_err_with(|| format!("invite `{email}` to `{org_slug}`"))?;
        Ok(())
    }

    /// Withdraw a pending invite. Does nothing to a membership already
    /// claimed from it — revoking that is [`Self::revoke`].
    pub async fn withdraw(&self, email: &str, org_slug: &str) -> Result<()> {
        self.conn
            .execute(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!(
                    "DELETE FROM invites WHERE email = '{}' AND org_slug = '{}'",
                    esc(&normalize_email(email)),
                    esc(org_slug)
                ),
            ))
            .await
            .wrap_err("withdraw invite")?;
        Ok(())
    }

    /// Every invite still waiting to be claimed.
    pub async fn pending(&self) -> Result<Vec<(String, Membership)>> {
        let rows = self
            .conn
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                "SELECT email, org_slug, role FROM invites ORDER BY email, org_slug".to_owned(),
            ))
            .await
            .wrap_err("list invites")?;
        rows.into_iter()
            .map(|r| {
                Ok((
                    r.try_get("", "email")?,
                    Membership {
                        org_slug: r.try_get("", "org_slug")?,
                        role: r.try_get("", "role")?,
                    },
                ))
            })
            .collect()
    }

    /// Turn every invite held for `email` into a membership for
    /// `user_id`, and return the orgs that changed hands.
    ///
    /// This is the binding moment, and the reason invites are safe: the
    /// address comes from the ISSUER's answer about a token it just
    /// validated, never from anything the client said. A caller that
    /// passed a client-supplied address here would have built an
    /// open door — see the call site in
    /// [`crate::central_auth::CentralFallbackResolver`].
    ///
    /// An invite for an org where the principal is already a member is
    /// consumed without changing the existing role: the invite is an
    /// offer, and a role already held on the server is the newer fact.
    ///
    /// Idempotent, and empty is the overwhelmingly common answer — every
    /// sign-in by an established member finds nothing here.
    pub async fn claim_for_email(&self, email: &str, user_id: Uuid) -> Result<Vec<String>> {
        let email = normalize_email(email);
        let rows = self
            .conn
            .query_all(Statement::from_string(
                DatabaseBackend::Sqlite,
                format!(
                    "SELECT org_slug, role FROM invites WHERE email = '{}' ORDER BY org_slug",
                    esc(&email)
                ),
            ))
            .await
            .wrap_err("read invites to claim")?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }

        let mut claimed = Vec::new();
        for r in rows {
            let org_slug: String = r.try_get("", "org_slug")?;
            let role: Option<String> = r.try_get("", "role")?;
            if self.role_for(user_id, &org_slug).await?.is_none() {
                self.upsert(user_id, &org_slug, role.as_deref()).await?;
            }
            // Consume either way — a claimed invite that stays pending
            // would re-apply its role on every sign-in and quietly undo
            // a later role change.
            self.withdraw(&email, &org_slug).await?;
            claimed.push(org_slug);
        }
        Ok(claimed)
    }
}

/// Addresses are compared case-insensitively, because an invite written
/// by an operator and an address reported by the issuer are typed by
/// different people and `Cody@` must claim an invite for `cody@`.
fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(0))
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

    /// The whole point: one account, several orgs, and nothing created
    /// locally in any of them beforehand.
    #[tokio::test]
    async fn an_invite_becomes_a_membership_for_whoever_claims_it() {
        let (_d, m) = store().await;
        m.invite("cody@example.app", "cbu", Some("admin"))
            .await
            .unwrap();
        m.invite("cody@example.app", "days-to-praise", None)
            .await
            .unwrap();

        // Before the claim the invite grants nothing at all.
        let cody = Uuid::new_v4();
        assert!(m.role_for(cody, "cbu").await.unwrap().is_none());

        let claimed = m.claim_for_email("cody@example.app", cody).await.unwrap();
        assert_eq!(claimed, vec!["cbu", "days-to-praise"]);
        assert_eq!(
            m.role_for(cody, "cbu").await.unwrap().unwrap().role,
            Some("admin".into())
        );
        assert!(m.role_for(cody, "days-to-praise").await.unwrap().is_some());
        assert!(
            m.pending().await.unwrap().is_empty(),
            "a claimed invite is consumed"
        );
    }

    /// An invite is for one address, and claiming is how the fence gets
    /// its key — so a different principal arriving at a different
    /// address must get nothing. This is the test that would fail if
    /// `claim_for_email` ever stopped filtering by email.
    #[tokio::test]
    async fn a_different_address_claims_nothing() {
        let (_d, m) = store().await;
        m.invite("cody@example.app", "cbu", Some("admin"))
            .await
            .unwrap();

        let someone_else = Uuid::new_v4();
        let claimed = m
            .claim_for_email("mallory@example.app", someone_else)
            .await
            .unwrap();
        assert!(claimed.is_empty());
        assert!(m.role_for(someone_else, "cbu").await.unwrap().is_none());
        assert_eq!(m.pending().await.unwrap().len(), 1, "still waiting");
    }

    /// Operators and issuers disagree about capitalisation constantly.
    #[tokio::test]
    async fn addresses_match_regardless_of_case() {
        let (_d, m) = store().await;
        m.invite("Cody@Example.app", "cbu", None).await.unwrap();
        let cody = Uuid::new_v4();
        assert_eq!(
            m.claim_for_email("cody@example.APP", cody).await.unwrap(),
            vec!["cbu"]
        );
    }

    /// A second sign-in must not re-apply the invited role over a role
    /// changed since — the invite was an offer, the current row is the
    /// newer fact.
    #[tokio::test]
    async fn claiming_never_overwrites_a_role_held_now() {
        let (_d, m) = store().await;
        let cody = Uuid::new_v4();
        m.upsert(cody, "cbu", Some("admin")).await.unwrap();
        m.invite("cody@example.app", "cbu", Some("reader"))
            .await
            .unwrap();

        m.claim_for_email("cody@example.app", cody).await.unwrap();
        assert_eq!(
            m.role_for(cody, "cbu").await.unwrap().unwrap().role,
            Some("admin".into()),
            "the invite is consumed, the standing role survives"
        );
        assert!(m.pending().await.unwrap().is_empty());
    }

    /// Withdrawing is for the promise, not for access already granted.
    #[tokio::test]
    async fn withdrawing_an_invite_leaves_a_claimed_membership_alone() {
        let (_d, m) = store().await;
        let cody = Uuid::new_v4();
        m.invite("cody@example.app", "cbu", None).await.unwrap();
        m.claim_for_email("cody@example.app", cody).await.unwrap();

        m.withdraw("cody@example.app", "cbu").await.unwrap();
        assert!(
            m.role_for(cody, "cbu").await.unwrap().is_some(),
            "revoking a granted membership is `revoke`, not `withdraw`"
        );
    }

    /// `all()` must see a principal that owns no local account
    /// anywhere — that is now the ordinary case, and the listing it
    /// replaced could not.
    #[tokio::test]
    async fn listing_reports_every_principal_not_just_local_accounts() {
        let (_d, m) = store().await;
        let a = Uuid::new_v4();
        let b = Uuid::new_v4();
        m.upsert(a, "cbu", Some("admin")).await.unwrap();
        m.upsert(b, "cbu", None).await.unwrap();
        m.upsert(b, "codywright", None).await.unwrap();
        assert_eq!(m.all().await.unwrap().len(), 3);
    }
}
