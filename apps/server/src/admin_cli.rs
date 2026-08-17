//! Operator subcommands, run ON the server against its own data root.
//!
//! ## Why these exist separately from the vox surface
//!
//! The vox `AuthService::migrate_user_email` authorizes with a SESSION —
//! the caller proves who they are, and is recorded as `changed_by`. That
//! is right for a person acting in the app, and wrong for an operator:
//! the usual reason to migrate an address is that its owner can no longer
//! sign in with it, so requiring their session to fix their login is
//! circular. Routing an operator through it also means impersonating the
//! user, which is worse than not authenticating at all — it puts their
//! password somewhere it needn't be and records the change as if they
//! made it.
//!
//! So authorization here is FILESYSTEM OWNERSHIP of the data root, the
//! same argument `OrgManagementImpl::new_local_trusted` and
//! `SnapshotImpl::new_local_trusted` already make: something that can
//! open `auth.sqlite` can do anything to it anyway, and a session check
//! on top would be theatre. In the cluster that means `kubectl exec` into
//! the pod, which is already the highest privilege available.
//!
//! Changes are recorded as `changed_by: None` (nobody signed in did it)
//! with an explicit reason, so the trail distinguishes an operator
//! migration from a user's own.

use eyre::{Context as _, bail};

/// Handle an `admin …` invocation. Returns `Ok(false)` when argv is not
/// an admin subcommand, so `main` falls through to booting the server.
pub async fn dispatch() -> eyre::Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("admin") {
        return Ok(false);
    }
    match args.get(1).map(String::as_str) {
        Some("migrate-email") => migrate_email(&args[2..]).await.map(|()| true),
        Some("email-history") => email_history(&args[2..]).await.map(|()| true),
        Some("set-password") => set_password(&args[2..]).await.map(|()| true),
        Some("list-users") => list_users(&args[2..]).await.map(|()| true),
        Some("delete-user") => delete_user(&args[2..]).await.map(|()| true),
        Some("set-role") => set_role(&args[2..]).await.map(|()| true),
        Some("create-user") => create_user(&args[2..]).await.map(|()| true),
        Some("merge-principals") => merge_principals(&args[2..]).await.map(|()| true),
        Some("adopt-principal") => adopt_principal(&args[2..]).await.map(|()| true),
        Some("memberships") => show_memberships(&args[2..]).await.map(|()| true),
        // Dev-only: compiled OUT of release builds entirely, so the
        // deployed (release) server can never seed a known-password
        // admin (PR #295 review). In a release binary `seed` is just an
        // unknown subcommand.
        #[cfg(debug_assertions)]
        Some("seed") => seed(&args[2..]).await.map(|()| true),
        // Also dev-only, and for the same reason.
        #[cfg(debug_assertions)]
        Some("demo") => crate::demo_cli::demo(&args[2..]).await.map(|()| true),
        Some("webdav") => webdav(&args[2..]).map(|()| true),
        other => {
            eprintln!(
                "usage:\n  \
                 task-server admin migrate-email --org <slug> --from <email> --to <email> \\\n    \
                 [--reason <text>] [--dry-run]\n  \
                 task-server admin email-history --org <slug> --email <address>\n  \
                 task-server admin set-password --org <slug> --email <address>\n    \
                 (reads the new password from STDIN)\n  \
                 task-server admin list-users --org <slug>\n  \
                 task-server admin delete-user --org <slug> --email <address> --yes\n  \
                 task-server admin set-role --org <slug> --email <address> [--role admin|--clear]\n  \
                 task-server admin create-user --org <slug> --email <address> \\\n    \
                 [--name <display>] [--username <handle>] (reads the password from STDIN)\n  \
                 task-server admin seed [--orgs <a,b,c>] [--email <address>] \\\n    \
                 [--password <pw>] [--no-divergence] (stands up a local multi-org dev vault)\n  \
                 task-server admin demo --org <acme-audio|vnt-video>\n    \
                 (plants `examples/studio` as one org on this data root — `just demo`\n     \
                 runs it twice, on two data roots, for two servers that federate)\n  \
                 task-server admin webdav --org <slug> [--hide <root-id>|--show <root-id>]\n    \
                 (no flag lists the org's File Roots and their WebDAV visibility)\n"
            );
            bail!("unknown admin subcommand: {}", other.unwrap_or("(none)"));
        }
    }
}

/// Minimal `--flag value` parsing. Deliberately not clap: this is two
/// operator verbs on a server binary, and adding an arg parser to it for
/// them would be the larger change.
pub(crate) fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

/// Open one org's auth store directly from the data root.
async fn open_org_auth(slug: &str) -> eyre::Result<crate::AuthState> {
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let org_root = data_root.org(slug);
    let db = org_root.auth_db();
    if !db.exists() {
        bail!("org `{slug}` has no auth store at {}", db.display());
    }
    let url = format!("sqlite://{}?mode=rwc", db.display());
    // The running server holds this same file open. Both connections use
    // WAL (`AuthState::open` enables it), which is exactly the
    // multi-process case WAL exists for, so a short write here is safe
    // alongside a live server.
    crate::AuthState::open(&url, &crate::auth_secret())
        .await
        .wrap_err_with(|| format!("open auth store for `{slug}`"))
}

async fn migrate_email(args: &[String]) -> eyre::Result<()> {
    let (Some(slug), Some(from), Some(to)) = (
        flag(args, "--org"),
        flag(args, "--from"),
        flag(args, "--to"),
    ) else {
        bail!("--org, --from and --to are all required");
    };
    let reason = flag(args, "--reason")
        .unwrap_or_else(|| "operator migration (task-server admin)".to_owned());
    let dry = has(args, "--dry-run");

    let auth = open_org_auth(&slug).await?;
    let user = auth
        .auth
        .find_user_by_email(&from)
        .await
        .map_err(|e| eyre::eyre!("look up `{from}` in `{slug}`: {e:?}"))?
        .ok_or_else(|| eyre::eyre!("no account with email `{from}` in org `{slug}`"))?;

    println!("{slug}: {} is {}", user.id, from);
    if dry {
        println!("  dry run — would migrate to {to}");
        return Ok(());
    }

    let moved = auth
        .auth
        .migrate_user_email(architect_auth::MigrateUserEmail {
            user_id: user.id,
            new_email: to.clone(),
            // Nobody signed in did this; the trail should say so rather
            // than name a user who wasn't involved.
            changed_by: None,
            reason: Some(reason),
        })
        .await
        .map_err(|e| eyre::eyre!("migrate `{from}` -> `{to}` in `{slug}`: {e:?}"))?;

    println!(
        "  now {} (user id {} — unchanged)",
        moved.email.as_deref().unwrap_or("(none)"),
        moved.id
    );
    // The id holding is the property that matters: everything keyed on it
    // (tasks, timers, sessions, authorship) stays attached.
    if moved.id != user.id {
        bail!("user id changed — this should be impossible; investigate before continuing");
    }
    Ok(())
}

/// Grant or clear architect-auth's `admin` role.
///
/// This is the bootstrap for admin itself: `require_admin` needs an
/// existing admin, so the FIRST one cannot be made through the admin
/// flows. Possession of the auth store is the only authority that
/// predates any account.
///
/// Scope note: this sets `auth_users.role`, which gates architect-auth's
/// `admin_*` flows. It is NOT the permission gate's role — that comes
/// from architect-permissions, which currently gives every validated
/// user the same default and never consults this column. Granting admin
/// therefore does not (yet) widen what the gate allows.
async fn set_role(args: &[String]) -> eyre::Result<()> {
    let (Some(slug), Some(email)) = (flag(args, "--org"), flag(args, "--email")) else {
        bail!("--org and --email are required");
    };
    let role = if has(args, "--clear") {
        None
    } else {
        Some(flag(args, "--role").unwrap_or_else(|| "admin".to_owned()))
    };
    let auth = open_org_auth(&slug).await?;
    let user = auth
        .auth
        .find_user_by_email(&email)
        .await
        .map_err(|e| eyre::eyre!("look up `{email}`: {e:?}"))?
        .ok_or_else(|| eyre::eyre!("no account with email `{email}` in org `{slug}`"))?;
    let updated = auth
        .auth
        .set_user_role_local_trusted(user.id, role.clone())
        .await
        .map_err(|e| eyre::eyre!("set role for `{email}` in `{slug}`: {e:?}"))?;
    println!(
        "{slug}: {} ({email}) role = {}",
        updated.id,
        updated.role.as_deref().unwrap_or("(none)")
    );
    Ok(())
}

/// Every account in one org's store.
async fn list_users(args: &[String]) -> eyre::Result<()> {
    let Some(slug) = flag(args, "--org") else {
        bail!("--org is required");
    };
    let auth = open_org_auth(&slug).await?;
    let users = auth
        .auth
        .list_users_local_trusted()
        .await
        .map_err(|e| eyre::eyre!("list users in `{slug}`: {e:?}"))?;
    if users.is_empty() {
        println!("{slug}: no accounts");
        return Ok(());
    }
    println!("{slug}:");
    for u in users {
        println!("  {}  {}", u.id, u.email.as_deref().unwrap_or("(no email)"));
    }
    Ok(())
}

/// Remove an account outright.
///
/// Requires `--yes`. This is not recoverable from here — the row is gone
/// and anything keyed on the user id is orphaned, so an operator should
/// have to say so deliberately rather than discover it from a typo in an
/// `--email` flag.
async fn delete_user(args: &[String]) -> eyre::Result<()> {
    let (Some(slug), Some(email)) = (flag(args, "--org"), flag(args, "--email")) else {
        bail!("--org and --email are required");
    };
    let auth = open_org_auth(&slug).await?;
    let user = auth
        .auth
        .find_user_by_email(&email)
        .await
        .map_err(|e| eyre::eyre!("look up `{email}`: {e:?}"))?
        .ok_or_else(|| eyre::eyre!("no account with email `{email}` in org `{slug}`"))?;

    if !has(args, "--yes") {
        println!("{slug}: would delete {} ({email})", user.id);
        println!("  re-run with --yes to actually delete — this cannot be undone");
        return Ok(());
    }
    auth.auth
        .delete_user_local_trusted(user.id)
        .await
        .map_err(|e| eyre::eyre!("delete `{email}` in `{slug}`: {e:?}"))?;
    println!("{slug}: deleted {} ({email})", user.id);
    Ok(())
}

/// Create an account in one org's auth store.
///
/// **This is the bootstrap for org membership**, and it exists because
/// there was no way in. `AuthService::sign_up_email_password` is
/// deliberately not public — open self-registration plus the org lane's
/// default `member` role made enforcement bypassable in one call — so
/// only an existing member can provision an account. An org with zero
/// accounts therefore has nobody who could create the first one, and is
/// unreachable by every client: CLI, GUI and agent alike. Five of the
/// six orgs on production are in exactly that state.
///
/// Possession of the auth store is the only authority that predates any
/// account, which is why this lives on the server binary next to
/// [`set_role`] rather than behind an RPC.
///
/// The password is read from STDIN for the same reason as
/// [`set_password`]: arguments are visible to every user on the box via
/// `ps` and land in shell history.
///
/// Creating the same email in several orgs makes several *distinct*
/// accounts with distinct user ids — auth stores are per-org and there
/// is no cross-org identity yet (federated-platform phase 3). They
/// share a login, not a principal.
async fn create_user(args: &[String]) -> eyre::Result<()> {
    use std::io::Read as _;

    let (Some(slug), Some(email)) = (flag(args, "--org"), flag(args, "--email")) else {
        bail!("--org and --email are required");
    };
    let mut password = String::new();
    std::io::stdin()
        .read_to_string(&mut password)
        .wrap_err("read the password from stdin")?;
    let password = password.trim_end_matches(['\n', '\r']).to_owned();
    if password.is_empty() {
        bail!(
            "no password on stdin — pipe it in, e.g. `kubectl exec -i … -- task-server admin create-user …`"
        );
    }

    let auth = open_org_auth(&slug).await?;
    // Idempotence: re-running after a partial sweep across several orgs
    // should report the existing account, not fail halfway with a
    // uniqueness error that leaves the operator guessing which orgs got
    // done.
    if let Some(existing) = auth
        .auth
        .find_user_by_email(&email)
        .await
        .map_err(|e| eyre::eyre!("look up `{email}` in `{slug}`: {e:?}"))?
    {
        println!(
            "{slug}: {email} already exists ({}) — nothing to do",
            existing.id
        );
        println!("  use `set-password` to change its credential");
        return Ok(());
    }

    let bundle = auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: email.clone(),
            password,
            name: flag(args, "--name"),
            username: flag(args, "--username"),
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: Some("task-server admin create-user".into()),
        })
        .await
        .map_err(|e| eyre::eyre!("create `{email}` in `{slug}`: {e:?}"))?;

    println!("{slug}: created {} ({email})", bundle.user.id);
    println!("  sign in with: task auth login --org {slug} --email {email}");
    Ok(())
}

/// Set an account's password without knowing the old one.
///
/// The operator counterpart to `AuthService::change_password`, which is
/// self-service and requires the current password. This one exists for
/// the case that flow cannot serve: the owner cannot sign in, so there
/// is no session and no known credential.
///
/// The new password is read from STDIN, never from an argument.
/// Arguments are visible to every user on the box via `ps` for the life
/// of the process, and land in shell history; stdin does neither. That
/// also means whoever runs this supplies the secret directly — it is not
/// something the command can be handed by a third party.
async fn set_password(args: &[String]) -> eyre::Result<()> {
    use std::io::Read as _;

    let (Some(slug), Some(email)) = (flag(args, "--org"), flag(args, "--email")) else {
        bail!("--org and --email are required");
    };
    let mut new_password = String::new();
    std::io::stdin()
        .read_to_string(&mut new_password)
        .wrap_err("read the new password from stdin")?;
    let new_password = new_password.trim_end_matches(['\n', '\r']).to_owned();
    if new_password.is_empty() {
        bail!(
            "no password on stdin — pipe it in, e.g. `kubectl exec -i … -- task-server admin set-password …`"
        );
    }

    let auth = open_org_auth(&slug).await?;
    let user = auth
        .auth
        .find_user_by_email(&email)
        .await
        .map_err(|e| eyre::eyre!("look up `{email}`: {e:?}"))?
        .ok_or_else(|| eyre::eyre!("no account with email `{email}` in org `{slug}`"))?;

    auth.auth
        .set_user_password_local_trusted(user.id, &new_password)
        .await
        .map_err(|e| eyre::eyre!("set password for `{email}` in `{slug}`: {e:?}"))?;

    println!("{slug}: password set for {} ({email})", user.id);
    println!("  existing sessions are NOT revoked — sign out elsewhere if that matters");
    Ok(())
}

async fn email_history(args: &[String]) -> eyre::Result<()> {
    let (Some(slug), Some(email)) = (flag(args, "--org"), flag(args, "--email")) else {
        bail!("--org and --email are required");
    };
    let auth = open_org_auth(&slug).await?;
    let user = auth
        .auth
        .find_user_by_email(&email)
        .await
        .map_err(|e| eyre::eyre!("look up `{email}`: {e:?}"))?
        .ok_or_else(|| eyre::eyre!("no account with email `{email}` in org `{slug}`"))?;
    let history = auth
        .auth
        .list_email_history(user.id)
        .await
        .map_err(|e| eyre::eyre!("history for `{email}`: {e:?}"))?;
    if history.is_empty() {
        println!("{slug}: {} has never changed email", user.id);
        return Ok(());
    }
    println!("{slug}: {}", user.id);
    for row in history {
        println!(
            "  {}  {} -> {}  by {}",
            row.created_at.to_rfc3339(),
            row.previous_email.as_deref().unwrap_or("(none)"),
            row.new_email,
            row.changed_by
                .map_or_else(|| "operator".to_owned(), |id| id.to_string()),
        );
        if let Some(reason) = row.reason {
            println!("      {reason}");
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────
//  merge-principals — one account per server (S2)
// ─────────────────────────────────────────────────────────────────────

/// One org's account for a given email, as it exists today.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OrgAccount {
    slug: String,
    user_id: uuid::Uuid,
}

/// Every account sharing one email, and which of them becomes the
/// principal.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Principal {
    email: String,
    /// The id every other account's rows get remapped onto.
    canonical: OrgAccount,
    /// The rest — `(slug, old_id)` that need a `user_id_map` row.
    absorbed: Vec<OrgAccount>,
}

impl Principal {
    /// Orgs this principal ends up a member of, canonical included.
    fn org_slugs(&self) -> Vec<String> {
        std::iter::once(self.canonical.slug.clone())
            .chain(self.absorbed.iter().map(|a| a.slug.clone()))
            .collect()
    }
}

/// Decide the principals from the per-org account lists.
///
/// The canonical account is the HOME org's when the email has one there,
/// because the home org is the only org on a server guaranteed to
/// outlive the others — an org can be exported and moved away, and a
/// principal whose canonical id left with it would strand every
/// remapped row. Otherwise the first org in slug order, so the choice is
/// deterministic across runs rather than dependent on directory
/// iteration order.
///
/// Emails are compared case-insensitively (`Cody@…` and `cody@…` are one
/// login everywhere else in this stack) but reported in the casing the
/// home — or first — org stored, since that is what the person typed.
///
/// Pure so the rule can be tested without six databases.
fn plan_principals(
    orgs: &[(String, bool, Vec<(uuid::Uuid, Option<String>)>)],
) -> (Vec<Principal>, Vec<String>) {
    use std::collections::BTreeMap;

    let mut by_email: BTreeMap<String, Vec<(String, bool, uuid::Uuid, String)>> = BTreeMap::new();
    let mut warnings = Vec::new();

    let mut sorted: Vec<&(String, bool, Vec<(uuid::Uuid, Option<String>)>)> = orgs.iter().collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));

    for (slug, is_home, users) in sorted {
        for (id, email) in users {
            let Some(email) = email else {
                // An account with no address cannot be matched to
                // anything; naming it is the whole point of a dry run.
                warnings.push(format!(
                    "{slug}: user {id} has no email — cannot merge, left alone"
                ));
                continue;
            };
            by_email.entry(email.to_lowercase()).or_default().push((
                slug.clone(),
                *is_home,
                *id,
                email.clone(),
            ));
        }
    }

    let mut principals = Vec::new();
    for (_key, mut rows) in by_email {
        // Home first, then slug order — `plan` reads the head as canonical.
        rows.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let (slug, _, id, display_email) = rows.remove(0);
        let canonical = OrgAccount { slug, user_id: id };
        let absorbed: Vec<OrgAccount> = rows
            .iter()
            .map(|(slug, _, id, _)| OrgAccount {
                slug: slug.clone(),
                user_id: *id,
            })
            .collect();
        principals.push(Principal {
            email: display_email,
            canonical,
            absorbed,
        });
    }

    (principals, warnings)
}

/// Columns that hold a user id. Matched by NAME across every table in
/// every per-org database, rather than from a list of tables this
/// command knows about — a store added later would otherwise be silently
/// left holding dead ids, and the dry run exists precisely to be
/// exhaustive about what a merge touches.
fn is_user_id_column(name: &str) -> bool {
    matches!(
        name,
        "user_id" | "home_user_id" | "owner_id" | "assignee_id" | "created_by" | "changed_by"
    )
}

/// `admin merge-principals` — the S2 dry run of one-account-per-server.
///
/// Reports, and by default changes NOTHING: which accounts share an
/// email across this server's orgs, which id each group would collapse
/// onto, the membership rows that would exist afterwards, and every row
/// in every per-org database that carries one of the absorbed ids.
///
/// Databases are opened READ-ONLY (`mode=ro`) so this is safe to run
/// against a live server — it holds these same files open.
///
/// `--apply` is deliberately NOT implemented yet: the server-level auth
/// store (S1) has to exist before there is anywhere to write the merged
/// principals, and a half-applied merge is the one outcome with no clean
/// rollback.
async fn merge_principals(args: &[String]) -> eyre::Result<()> {
    use sea_orm::{ConnectionTrait as _, Database, DatabaseBackend, Statement};

    if has(args, "--apply") {
        bail!(
            "--apply is not implemented yet — S1 (the server-level auth store) must land first.\n\
             This command is read-only today."
        );
    }

    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let orgs = data_root
        .scan_orgs()
        .map_err(|e| eyre::eyre!("scan orgs: {e}"))?;

    println!(
        "merge-principals (DRY RUN) — data root {}",
        data_root.path().display()
    );
    println!();

    // ── every org's accounts ────────────────────────────────────────
    let mut per_org: Vec<(String, bool, Vec<(uuid::Uuid, Option<String>)>)> = Vec::new();
    for (root, manifest) in &orgs {
        let slug = root.slug().to_owned();
        if !root.auth_db().exists() {
            println!("  {slug}: no auth store — skipped");
            continue;
        }
        let auth = open_org_auth(&slug).await?;
        let users = auth
            .auth
            .list_users_local_trusted()
            .await
            .map_err(|e| eyre::eyre!("list users in `{slug}`: {e:?}"))?;
        per_org.push((
            slug,
            manifest.is_home,
            users.into_iter().map(|u| (u.id, u.email)).collect(),
        ));
    }

    let (principals, warnings) = plan_principals(&per_org);

    // ── principals ──────────────────────────────────────────────────
    println!("principals ({})", principals.len());
    for p in &principals {
        let home_note = if p.absorbed.is_empty() {
            "  (single org — nothing to merge)"
        } else {
            ""
        };
        println!(
            "  {}{home_note}\n    canonical  {}  ({})",
            p.email, p.canonical.user_id, p.canonical.slug
        );
        for a in &p.absorbed {
            println!("    absorbed   {}  ({})", a.user_id, a.slug);
        }
    }
    println!();

    // ── memberships ─────────────────────────────────────────────────
    let membership_rows: usize = principals.iter().map(|p| p.org_slugs().len()).sum();
    println!("memberships to create ({membership_rows})");
    for p in &principals {
        println!("  {}: {}", p.email, p.org_slugs().join(", "));
    }
    println!();

    // ── rows carrying an absorbed id ────────────────────────────────
    // Absorbed ids only: the canonical id keeps its rows untouched.
    let mut absorbed_by_slug: std::collections::BTreeMap<String, Vec<uuid::Uuid>> =
        std::collections::BTreeMap::new();
    for p in &principals {
        for a in &p.absorbed {
            absorbed_by_slug
                .entry(a.slug.clone())
                .or_default()
                .push(a.user_id);
        }
    }

    println!("rows to rewrite");
    let mut total = 0usize;
    for (root, _) in &orgs {
        let slug = root.slug().to_owned();
        let Some(ids) = absorbed_by_slug.get(&slug) else {
            continue;
        };
        for (label, path) in [
            ("auth", root.auth_db()),
            ("identity", root.identity_db()),
            ("timer", root.timer_db()),
            ("finance", root.finance_db()),
            ("threads", root.threads_db()),
            ("prefs", root.prefs_db()),
        ] {
            if !path.exists() {
                continue;
            }
            // Read-only: a live server holds these open.
            let url = format!("sqlite://{}?mode=ro", path.display());
            let conn = Database::connect(&url)
                .await
                .wrap_err_with(|| format!("open {label} db for `{slug}`"))?;
            let tables = conn
                .query_all(Statement::from_string(
                    DatabaseBackend::Sqlite,
                    "SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'"
                        .to_owned(),
                ))
                .await?;
            for t in tables {
                let table: String = t.try_get("", "name")?;
                let cols = conn
                    .query_all(Statement::from_string(
                        DatabaseBackend::Sqlite,
                        format!("PRAGMA table_info('{table}')"),
                    ))
                    .await?;
                for c in cols {
                    let column: String = c.try_get("", "name")?;
                    if !is_user_id_column(&column) {
                        continue;
                    }
                    // Ids are stored as text or as blobs depending on the
                    // store; compare against both spellings rather than
                    // guessing, so a zero here means zero rows and not a
                    // type mismatch quietly hiding them.
                    let list = ids
                        .iter()
                        .map(|id| format!("'{id}'"))
                        .collect::<Vec<_>>()
                        .join(",");
                    let sql = format!(
                        "SELECT COUNT(*) AS n FROM \"{table}\" WHERE CAST(\"{column}\" AS TEXT) IN ({list})"
                    );
                    let row = conn
                        .query_one(Statement::from_string(DatabaseBackend::Sqlite, sql))
                        .await?;
                    let n: i64 = row.map(|r| r.try_get("", "n")).transpose()?.unwrap_or(0);
                    if n > 0 {
                        println!("  {slug:<18} {label:<9} {table}.{column}  {n}");
                        total += usize::try_from(n).unwrap_or(0);
                    }
                }
            }
        }
    }
    if total == 0 {
        println!("  (none — absorbed accounts own no rows in any per-org store)");
    } else {
        println!("  total {total}");
    }

    if !warnings.is_empty() {
        println!();
        println!("warnings");
        for w in &warnings {
            println!("  {w}");
        }
    }

    println!();
    println!("nothing was written. `--apply` lands with S1.");
    Ok(())
}

/// Resolve the home org — the server's identity authority.
fn home_org(
    orgs: &[(org_proto::OrgRoot, org_proto::OrgManifest)],
) -> eyre::Result<&org_proto::OrgRoot> {
    orgs.iter()
        .find(|(_, m)| m.is_home)
        .map(|(r, _)| r)
        .ok_or_else(|| {
            eyre::eyre!(
                "no org is marked `is_home` in its org.toml — the home org is this server's \
                 identity authority, so memberships have nowhere to live"
            )
        })
}

/// `admin adopt-principal --email <addr>` — S1 of one-account-per-server.
///
/// Give one principal a membership row in every org on this server that
/// already holds an account with that address, carrying THAT org's role
/// so an admin in one org and a reader in another stay exactly that.
/// The principal is the home org's user id.
///
/// It adopts orgs that already provisioned the person; it never invents
/// membership. An org with no account for the address is reported and
/// skipped — `admin create-user --org <slug> --email <addr>` first if
/// that org should have them, then re-run.
///
/// Idempotent: re-running updates roles in place, which is the ONLY way
/// a role changes once the lane stops reading each org's own role
/// column. `--dry-run` prints the rows and writes nothing.
async fn adopt_principal(args: &[String]) -> eyre::Result<()> {
    let Some(email) = flag(args, "--email") else {
        bail!("--email is required");
    };
    let dry = has(args, "--dry-run");

    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let orgs = data_root
        .scan_orgs()
        .map_err(|e| eyre::eyre!("scan orgs: {e}"))?;
    let home = home_org(&orgs)?;
    let home_slug = home.slug().to_owned();

    // The principal id is the home org's user id, so nothing that
    // already references it in the home org — where nearly all the data
    // is — has to be rewritten.
    let home_auth = open_org_auth(&home_slug).await?;
    let principal = home_auth
        .auth
        .find_user_by_email(&email)
        .await
        .map_err(|e| eyre::eyre!("look up `{email}` in home org `{home_slug}`: {e:?}"))?
        .ok_or_else(|| {
            eyre::eyre!(
                "no account with email `{email}` in the home org `{home_slug}` — the principal \
                 must exist there first (`admin create-user --org {home_slug} --email {email}`)"
            )
        })?;

    println!(
        "principal {} ({email}) — home org `{home_slug}`{}",
        principal.id,
        if dry { "  [DRY RUN]" } else { "" }
    );

    let mut adopted: Vec<(String, Option<String>)> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    for (root, _) in &orgs {
        let slug = root.slug().to_owned();
        if !root.auth_db().exists() {
            skipped.push(format!("{slug}: no auth store"));
            continue;
        }
        let auth = open_org_auth(&slug).await?;
        let account = auth
            .auth
            .find_user_by_email(&email)
            .await
            .map_err(|e| eyre::eyre!("look up `{email}` in `{slug}`: {e:?}"))?;
        match account {
            Some(user) => adopted.push((slug, user.role)),
            None => skipped.push(format!("{slug}: no account for `{email}`")),
        }
    }

    if !dry {
        let store = crate::memberships::Memberships::open(&home.memberships_db()).await?;
        for (slug, role) in &adopted {
            store.upsert(principal.id, slug, role.as_deref()).await?;
        }
    }

    println!();
    println!("memberships ({})", adopted.len());
    for (slug, role) in &adopted {
        println!(
            "  {slug:<20} role = {}",
            role.as_deref().unwrap_or("(member)")
        );
    }
    if !skipped.is_empty() {
        println!();
        println!("skipped");
        for s in &skipped {
            println!("  {s}");
        }
    }
    println!();
    if dry {
        println!("nothing written (--dry-run).");
    } else {
        println!("written to {}", home.memberships_db().display());
    }
    Ok(())
}

/// `admin memberships [--email <addr>]` — read the table back.
///
/// The counterpart to `adopt-principal`: what the server will actually
/// believe about who belongs where, read from the rows rather than
/// inferred from six auth stores.
async fn show_memberships(args: &[String]) -> eyre::Result<()> {
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let orgs = data_root
        .scan_orgs()
        .map_err(|e| eyre::eyre!("scan orgs: {e}"))?;
    let home = home_org(&orgs)?;
    let db = home.memberships_db();
    if !db.exists() {
        println!(
            "no memberships store yet at {} — run `admin adopt-principal --email <addr>`",
            db.display()
        );
        return Ok(());
    }

    let store = crate::memberships::Memberships::open_ro(&db).await?;
    let home_auth = open_org_auth(home.slug()).await?;

    let users = match flag(args, "--email") {
        Some(email) => {
            let u = home_auth
                .auth
                .find_user_by_email(&email)
                .await
                .map_err(|e| eyre::eyre!("look up `{email}`: {e:?}"))?
                .ok_or_else(|| eyre::eyre!("no account with email `{email}` in the home org"))?;
            vec![u]
        }
        None => home_auth
            .auth
            .list_users_local_trusted()
            .await
            .map_err(|e| eyre::eyre!("list users: {e:?}"))?,
    };

    for u in users {
        let rows = store.for_user(u.id).await?;
        println!("{}  {}", u.id, u.email.as_deref().unwrap_or("(no email)"));
        if rows.is_empty() {
            println!("  (no memberships — not a member of any org on this server)");
        }
        for r in rows {
            println!(
                "  {:<20} role = {}",
                r.org_slug,
                r.role.as_deref().unwrap_or("(member)")
            );
        }
    }
    Ok(())
}

/// `admin seed` — stand up (or top up) a LOCAL multi-org dev vault with
/// demo data so a fresh `task-server` has something to sign into and
/// exercise: an owner account with known credentials in every org, a
/// couple of vault notes, and a Files root with real version history
/// (checkpoints + a Named Version) plus a divergence for the resolution
/// UI (issue #267). DEV ONLY.
///
/// Authorization is filesystem ownership of the data root, like every
/// other verb here. It writes into `$TASK_DATA_ROOT` (default
/// `~/.task`) — point that at a throwaway dir for local dev (the
/// `dev-seed` wrapper does). Idempotent: an existing owner account or
/// `Demo Project` root is left as-is, so re-running tops up what's
/// missing rather than duplicating.
#[cfg(debug_assertions)]
async fn seed(args: &[String]) -> eyre::Result<()> {
    // This verb plants a KNOWN-PASSWORD admin and mints default orgs, so
    // it must never run against a real data root. Require TASK_DATA_ROOT
    // to be set explicitly (the `dev-seed` wrapper points it at a
    // throwaway dir) — refuse the `~/.task` default (PR #295 review).
    match std::env::var("TASK_DATA_ROOT") {
        Ok(v) if !v.trim().is_empty() => {}
        _ => bail!(
            "refusing to seed: set TASK_DATA_ROOT to a throwaway dir first \
             (this plants a known-password admin — never point it at real data). \
             `just dev-seed` does this for you."
        ),
    }
    let email = flag(args, "--email").unwrap_or_else(|| "dev@fasttrackstudio.dev".to_owned());
    let password = flag(args, "--password").unwrap_or_else(|| "password".to_owned());
    // A Files root with a divergence, so the #267 resolution UI has
    // something to resolve. On by default (`--no-divergence` opts out);
    // browsing/resolving a divergent root works fine — the earlier
    // belief that it wedged a cold server's browse was a misdiagnosis
    // (see cold_open_browse_on_a_divergent_root_completes).
    let with_divergence = !has(args, "--no-divergence");
    let orgs: Vec<(String, String)> = match flag(args, "--orgs") {
        Some(list) => list
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| (s.to_owned(), title_case(s)))
            .collect(),
        None => vec![
            ("fasttrackstudio".to_owned(), "FastTrackStudio".to_owned()),
            ("acme-films".to_owned(), "Acme Films".to_owned()),
            ("northwind".to_owned(), "Northwind".to_owned()),
        ],
    };
    if orgs.is_empty() {
        bail!("--orgs was empty");
    }

    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .ensure()
        .map_err(|e| eyre::eyre!("ensure data root: {e}"))?;
    println!(
        "seeding {} org(s) into {}",
        orgs.len(),
        data_root.path().display()
    );

    for (i, (slug, display)) in orgs.iter().enumerate() {
        let is_home = i == 0;
        println!(
            "\n== {slug} ({display}){} ==",
            if is_home { " [home]" } else { "" }
        );
        match data_root.init_org(slug, display, is_home) {
            Ok(_) => println!("  org: created on disk"),
            Err(e) => println!("  org: already present ({e})"),
        }
        let org = data_root.org(slug);
        seed_owner(&org, &email, &password, display).await?;
        seed_vault_notes(&org, display)?;
        seed_files_demo(&org, with_divergence).await?;
        // The full studio dataset — 50 projects, albums with songs,
        // the song library / setlists / events, and media-bearing
        // Files roots — only in the home org: the other orgs stay
        // light so multi-org flows stay fast to eyeball.
        if is_home {
            seed_studio_vault(&org)?;
            seed_studio_files(&org, with_divergence).await?;
        }
    }

    println!("\ndone. sign in at the web app with:");
    println!("  email:    {email}");
    println!("  password: {password}");
    println!("home org:   {}", orgs[0].0);
    Ok(())
}

/// `north-west-films` -> `North West Films`.
#[cfg(debug_assertions)]
fn title_case(slug: &str) -> String {
    slug.split(['-', '_'])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            match c.next() {
                Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Create (idempotently) the org's owner account with the admin role.
#[cfg(debug_assertions)]
async fn seed_owner(
    org: &org_proto::OrgRoot,
    email: &str,
    password: &str,
    display: &str,
) -> eyre::Result<()> {
    let db = org.auth_db();
    let url = format!("sqlite://{}?mode=rwc", db.display());
    let auth = crate::AuthState::open(&url, &crate::auth_secret())
        .await
        .wrap_err_with(|| format!("open/create auth store for `{}`", org.slug()))?;
    seed_account(
        &auth,
        email,
        password,
        &format!("Dev ({display})"),
        None,
        Some("admin"),
        "owner",
    )
    .await?;
    // The web shell's debug-build roster (`ui::auth::DEV_ACCOUNTS` —
    // keep the two lists in lockstep): a fresh debug browser boots
    // straight into Guest with no login, and the account dropdown
    // switches between the others. Debug-only on both sides, so the
    // known passwords never exist in a release binary.
    const DEV_ACCOUNTS: [(&str, &str, &str, &str, Option<&str>); 4] = [
        (
            "cody@fasttrackstudios.com",
            "dev-cody-2026",
            "Cody Wright",
            "cody",
            Some("admin"),
        ),
        (
            "carter@fasttrackstudios.com",
            "dev-carter-2026",
            "Carter Whitlock",
            "carter",
            None,
        ),
        (
            "tom@fasttrackstudios.com",
            "dev-tom-2026",
            "Tom Brooks",
            "tom",
            None,
        ),
        (
            "guest@fasttrackstudios.com",
            "dev-guest-2026",
            "Guest",
            "guest",
            None,
        ),
    ];
    for (mail, pass, name, username, role) in DEV_ACCOUNTS {
        seed_account(&auth, mail, pass, name, Some(username), role, "dev account").await?;
    }
    Ok(())
}

/// Create one auth account if it doesn't exist yet (idempotent seed).
#[cfg(debug_assertions)]
pub(crate) async fn seed_account(
    auth: &crate::AuthState,
    email: &str,
    password: &str,
    name: &str,
    username: Option<&str>,
    role: Option<&str>,
    label: &str,
) -> eyre::Result<()> {
    if let Some(existing) = auth
        .auth
        .find_user_by_email(email)
        .await
        .map_err(|e| eyre::eyre!("look up `{email}`: {e:?}"))?
    {
        println!("  {label}: {email} already exists ({})", existing.id);
        return Ok(());
    }
    let bundle = auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: email.to_owned(),
            password: password.to_owned(),
            name: Some(name.to_owned()),
            username: username.map(str::to_owned),
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: Some("task-server admin seed".into()),
        })
        .await
        .map_err(|e| eyre::eyre!("create `{email}`: {e:?}"))?;
    if let Some(role) = role {
        auth.auth
            .set_user_role_local_trusted(bundle.user.id, Some(role.to_owned()))
            .await
            .map_err(|e| eyre::eyre!("set {role} role: {e:?}"))?;
    }
    println!(
        "  {label}: {email} created ({}){}",
        bundle.user.id,
        role.map(|r| format!(" with {r} role")).unwrap_or_default()
    );
    Ok(())
}

/// A couple of vault markdown notes so the org isn't empty.
#[cfg(debug_assertions)]
fn seed_vault_notes(org: &org_proto::OrgRoot, display: &str) -> eyre::Result<()> {
    let vault = org.vault_dir();
    std::fs::create_dir_all(&vault).wrap_err("create vault dir")?;
    let welcome = format!(
        "---\ntitle: Welcome to {display}\ntype: note\n---\n\n# Welcome to {display}\n\n\
         This is a **seeded dev org** for local task-server development. Everything \
         here is demo data — safe to change or delete.\n"
    );
    write_if_absent(&vault.join("Welcome.md"), &welcome)?;
    let project = format!(
        "---\ntitle: Q1 Launch\ntype: project\nstatus: active\n---\n\n# Q1 Launch\n\n\
         A seeded demo project in {display}.\n\n- [ ] Kickoff\n- [ ] Draft\n- [ ] Ship\n"
    );
    write_if_absent(&vault.join("Q1 Launch.md"), &project)?;
    println!("  vault: Welcome + Q1 Launch notes");
    Ok(())
}

#[cfg(debug_assertions)]
fn write_if_absent(path: &std::path::Path, content: &str) -> eyre::Result<()> {
    if !path.exists() {
        std::fs::write(path, content).wrap_err_with(|| format!("write {}", path.display()))?;
    }
    Ok(())
}

/// A Files root with real version history (three checkpoints + a Named
/// Version) and a divergence — the demo the version-history / divergence
/// UI (issue #267) is built to show.
#[cfg(debug_assertions)]
async fn seed_files_demo(org: &org_proto::OrgRoot, with_divergence: bool) -> eyre::Result<()> {
    let files_dir = org.path().join("files");
    let backend = files::FilesBackend::new(&files_dir, org.vault_dir())
        .map_err(|e| eyre::eyre!("open files backend: {e}"))?;
    // Always shut the backend down — it holds jj repo handles + cadence
    // tasks — whether the body succeeds or errors out (PR #295 review).
    let result = seed_files_demo_inner(&backend, &files_dir, with_divergence).await;
    backend.shutdown().await;
    result
}

#[cfg(debug_assertions)]
async fn seed_files_demo_inner(
    backend: &files::FilesBackend,
    files_dir: &std::path::Path,
    with_divergence: bool,
) -> eyre::Result<()> {
    let existing = files::FilesService::list_roots(backend)
        .await
        .map_err(|e| eyre::eyre!("list roots: {e:?}"))?;
    if let Some(root) = existing.iter().find(|r| r.name == "Demo Project") {
        // Already seeded. Top up the divergence if it's wanted and not
        // present yet (e.g. a prior `--no-divergence` run), so a re-run
        // never reports success with nothing for the #267 UI to resolve.
        if with_divergence {
            let has_divergence = files::FilesService::divergences(backend, root.id)
                .await
                .map_err(|e| eyre::eyre!("divergences: {e:?}"))?
                .iter()
                .any(|d| d.path == "edit.mov");
            if !has_divergence {
                backend
                    .seed_divergent_file(
                        root.id,
                        "edit.mov",
                        b"reel v4 - warm grade",
                        b"reel v4 - cool grade",
                    )
                    .await
                    .map_err(|e| eyre::eyre!("seed divergence: {e:?}"))?;
                println!("  files: Demo Project present — added a divergence");
                return Ok(());
            }
        }
        println!("  files: Demo Project already present");
        return Ok(());
    }

    // create_root adopts an existing on-disk dir; checkpoint_now captures
    // whatever is on disk at call time, so we write, then checkpoint. The
    // root must live inside the Files area (roots are confined to it).
    let root_dir = files_dir.join("demo-project");
    std::fs::create_dir_all(&root_dir).wrap_err("create demo root dir")?;
    std::fs::write(root_dir.join("edit.mov"), b"reel v1 - assembly cut")?;
    std::fs::write(
        root_dir.join("graphics.png"),
        b"\x89PNG\r\n\x1a\n seeded-demo",
    )?;
    std::fs::write(
        root_dir.join("notes.md"),
        b"# Editorial notes\n\n- assembly cut\n",
    )?;

    let root = files::FilesService::create_root(
        backend,
        root_dir.to_string_lossy().into_owned(),
        "Demo Project".to_owned(),
        files::RootFlavor::Media,
    )
    .await
    .map_err(|e| eyre::eyre!("create root: {e:?}"))?;
    files::FilesService::checkpoint_now(backend, root.id, Some("Initial import".to_owned()))
        .await
        .map_err(|e| eyre::eyre!("checkpoint 1: {e:?}"))?;

    // A revision, checkpointed and given a Named Version.
    std::fs::write(root_dir.join("edit.mov"), b"reel v2 - rough cut")?;
    std::fs::write(
        root_dir.join("notes.md"),
        b"# Editorial notes\n\n- assembly cut\n- rough cut: tightened the intro\n",
    )?;
    let cp2 = files::FilesService::checkpoint_now(backend, root.id, Some("Rough cut".to_owned()))
        .await
        .map_err(|e| eyre::eyre!("checkpoint 2: {e:?}"))?;
    files::FilesService::name_version(
        backend,
        root.id,
        cp2.commit_id.clone(),
        "Rough Cut v1".to_owned(),
    )
    .await
    .map_err(|e| eyre::eyre!("name version: {e:?}"))?;

    // A third revision.
    std::fs::write(root_dir.join("edit.mov"), b"reel v3 - color pass")?;
    files::FilesService::checkpoint_now(backend, root.id, Some("Color pass".to_owned()))
        .await
        .map_err(|e| eyre::eyre!("checkpoint 3: {e:?}"))?;

    // A divergence on edit.mov, for the #267 resolution UI (on by
    // default; `--no-divergence` skips it).
    if with_divergence {
        backend
            .seed_divergent_file(
                root.id,
                "edit.mov",
                b"reel v4 - warm grade",
                b"reel v4 - cool grade",
            )
            .await
            .map_err(|e| eyre::eyre!("seed divergence: {e:?}"))?;
    }

    println!(
        "  files: Demo Project (3 checkpoints, 1 named version{})",
        if with_divergence {
            ", 1 divergence"
        } else {
            ""
        }
    );
    Ok(())
}

// ── The studio dataset (home org only) ────────────────────────────
//
// A dataset big enough to actually exercise the UI: 50 projects (10
// video productions, 3 albums whose songs are sub-projects, 37 other
// engagements), each with its own folder of working notes — the
// project's "vault within the vault". Plus the performance stack: a
// song library, setlists, and events that reference them. And
// media-bearing Files roots (real tiny renditions when ffmpeg is on
// PATH) so browsing, review, and version history all have teeth.
//
// Everything is deterministic and idempotent: names are fixed,
// numbers derive from indices, notes are write-if-absent, roots are
// topped up by name. Playwright/dioxus tests and screenshots can
// rely on exact titles.

/// The ten video productions. Also seeded as Files roots.
#[cfg(debug_assertions)]
const VIDEO_PROJECTS: [&str; 10] = [
    "Aurora Sneaker Spot",
    "Hayes Wedding Film",
    "Skyline Doc — Ep 1",
    "Product Launch Teaser",
    "Cafe Brand Story",
    "Drone Reel 2026",
    "Iron & Oak Gym Promo",
    "Charity Gala Recap",
    "Neon Run Music Video",
    "Color 101 Tutorial Series",
];

/// The three albums and their songs (sub-projects of the album).
/// 6 + 4 + 8 songs — inside the brief's 4–8 per album. Song titles
/// are unique across the whole vault ON PURPOSE: wikilink resolution
/// is basename-keyed, so a song sharing its album's name (or any
/// other page's) would shadow one of the two.
#[cfg(debug_assertions)]
const ALBUMS: [(&str, &str, &[&str]); 3] = [
    (
        "Midnight Static",
        "Neon Palms",
        &[
            "Static Heart",
            "Neon Run",
            "Afterglow",
            "Low Tide",
            "Signal Fade",
            "City Sleeps",
        ],
    ),
    (
        "Golden Hour",
        "Ada June",
        &[
            "First Light",
            "Amber Sky",
            "Slow Burn",
            "Golden Hour (Reprise)",
        ],
    ),
    (
        "Roots & Wires",
        "Timber & Rye",
        &[
            "Copper Strings",
            "Dust & Diesel",
            "Backroad Hymn",
            "Porch Light",
            "Wire & Wood",
            "River Stone",
            "Old Growth",
            "Homestead",
        ],
    ),
];

/// The other 37 engagements (10 video + 3 albums + these = 50).
#[cfg(debug_assertions)]
const OTHER_PROJECTS: [&str; 37] = [
    "Acme Brand Refresh",
    "Acme Sizzle Reel",
    "Northwind Onboarding Videos",
    "Northwind Trade Show Loop",
    "Bluebird Podcast Launch",
    "Bluebird Season 2",
    "Fall Tour Prep",
    "Acoustic Sessions — Vol 1",
    "Acoustic Sessions — Vol 2",
    "Sunday Service Production",
    "Christmas Special 2026",
    "Easter Live Album",
    "Studio Website Redesign",
    "Gear Inventory Overhaul",
    "Control Room B Buildout",
    "Patch Bay Relabel",
    "Sample Library Cleanup",
    "Archive Migration",
    "Client Portal Rollout",
    "Merch Store Photos",
    "EPK — Neon Palms",
    "EPK — Ada June",
    "Single — Glass River",
    "Single — Paper Kites",
    "Remix Pack — Static Heart",
    "Live at the Fox Theater",
    "Festival Recap — Summerline",
    "Voiceover Sessions — Q3",
    "Audiobook — The Long Field",
    "Jingle Package — Fresh Mart",
    "Podcast Edit Retainer",
    "Wedding Highlights — Ortiz",
    "Corporate Training — Delta Freight",
    "Real Estate Walkthroughs",
    "Youth Camp Recap",
    "Studio Open House",
    "Intern Program 2026",
];

/// Rotating project statuses — every one a REAL `ProjectStatus`
/// variant (the proto's `from_str`), so each lands in a genuine
/// filter bucket rather than the unknown-status fallback.
#[cfg(debug_assertions)]
fn project_status(i: usize) -> &'static str {
    ["active", "on-hold", "done", "stale"][i % 4]
}

/// Deterministic musical facts from an index — believable, stable.
#[cfg(debug_assertions)]
fn song_facts(i: usize) -> (&'static str, u32, f64) {
    const KEYS: [&str; 8] = ["C", "G", "D", "A", "E", "B", "F", "Bb"];
    let key = KEYS[i % KEYS.len()];
    let bpm = 72 + ((i * 13) % 76) as u32; // 72..148
    let duration = 180.0 + ((i * 29) % 150) as f64; // 3:00..5:30
    (key, bpm, duration)
}

/// One `type: song` note — the ONE emitter of the song frontmatter
/// shape (`SongFront`), so a schema change lands in every seeded
/// song at once.
#[cfg(debug_assertions)]
fn song_note(title: &str, artist: &str, album: Option<&str>, facts_index: usize) -> String {
    let (key, bpm, duration) = song_facts(facts_index);
    let album_line = album.map(|a| format!("album: {a}\n")).unwrap_or_default();
    let verse_end = duration * 0.35;
    let chorus_end = duration * 0.6;
    let bridge_end = duration * 0.8;
    format!(
        "---\ntitle: {title}\ntype: song\nartist: {artist}\n{album_line}key: {key}\nbpm: {bpm}\ntime_signature: \"4/4\"\nduration_sec: {duration}\nsections:\n  - name: Intro\n    start_sec: 0\n    end_sec: 12\n  - name: Verse 1\n    start_sec: 12\n    end_sec: {verse_end:.0}\n  - name: Chorus\n    start_sec: {verse_end:.0}\n    end_sec: {chorus_end:.0}\n  - name: Bridge\n    start_sec: {chorus_end:.0}\n    end_sec: {bridge_end:.0}\n  - name: Outro\n    start_sec: {bridge_end:.0}\n    end_sec: {duration:.0}\n---\n\n\
         # {title}\n\n\
         Verse one keeps it low and close,\nthe city hums a distant note.\n\n\
         **Chorus** — we don't fade, we amplify,\nsignal strong against the night.\n\n\
         *(seeded demo lyric — original text)*\n"
    )
}

/// The studio vault: projects with their working notes, albums with
/// song sub-projects, the song library, setlists, and events.
#[cfg(debug_assertions)]
fn seed_studio_vault(org: &org_proto::OrgRoot) -> eyre::Result<()> {
    let vault = org.vault_dir();
    std::fs::create_dir_all(&vault).wrap_err("create vault dir")?;

    // ── 10 video projects + 37 other engagements ──────────────
    let mut planted = 0usize;
    for (i, name) in VIDEO_PROJECTS
        .iter()
        .chain(OTHER_PROJECTS.iter())
        .enumerate()
    {
        let dir = vault.join("Projects").join(name);
        std::fs::create_dir_all(&dir)?;
        let status = project_status(i);
        let is_video = i < VIDEO_PROJECTS.len();
        let kind = if is_video { "video" } else { "engagement" };
        let note = format!(
            "---\ntitle: {name}\ntype: project\nstatus: {status}\nkind: {kind}\n---\n\n\
             # {name}\n\n\
             Working home for **{name}** — notes, decisions, and links live here, \
             beside the project instead of in a shared pile.\n\n\
             ## Next\n\n- [ ] Kickoff notes\n- [ ] First cut / draft\n- [ ] Client review\n- [ ] Deliver\n"
        );
        write_if_absent(&dir.join(format!("{name}.md")), &note)?;
        std::fs::create_dir_all(dir.join("Notes"))?;
        let brief = format!(
            "---\ntitle: Brief\ntype: note\n---\n\n# Brief — {name}\n\n\
             Audience, tone, references, and the one-line promise. Seeded demo copy: \
             deliver something the client screenshots and sends to their team.\n"
        );
        write_if_absent(&dir.join("Notes").join("Brief.md"), &brief)?;
        let log = format!(
            "---\ntitle: Session Log\ntype: note\n---\n\n# Session Log — {name}\n\n\
             - Kickoff: scoped the deliverables.\n- Session 2: first pass reviewed in the room.\n"
        );
        write_if_absent(&dir.join("Notes").join("Session Log.md"), &log)?;
        planted += 1;
    }

    // ── 3 albums, songs as sub-projects with their own notes ──
    let mut song_index = 0usize;
    for (album, artist, songs) in ALBUMS {
        let album_dir = vault.join("Albums").join(album);
        std::fs::create_dir_all(&album_dir)?;
        let tracklist = songs
            .iter()
            .map(|s| format!("1. [[{s}]]"))
            .collect::<Vec<_>>()
            .join("\n");
        let album_note = format!(
            "---\ntitle: {album}\ntype: project\nstatus: active\nkind: album\nartist: {artist}\n---\n\n\
             # {album}\n\n*{artist}* — {n} songs, each its own sub-project below.\n\n\
             ## Tracklist\n\n{tracklist}\n\n\
             ## Next\n\n- [ ] Tracking\n- [ ] Mixing\n- [ ] Master\n- [ ] Artwork\n- [ ] Release\n",
            n = songs.len(),
        );
        write_if_absent(&album_dir.join(format!("{album}.md")), &album_note)?;

        for song in songs {
            let song_dir = album_dir.join(song);
            std::fs::create_dir_all(&song_dir)?;
            write_if_absent(
                &song_dir.join(format!("{song}.md")),
                &song_note(song, artist, Some(album), song_index),
            )?;
            song_index += 1;
            let prod = format!(
                "---\ntitle: Production Notes\ntype: note\n---\n\n# Production Notes — {song}\n\n\
                 - Drums tracked in Room A.\n- [ ] Comp vocals\n- [ ] Print stems\n"
            );
            write_if_absent(&song_dir.join("Production Notes.md"), &prod)?;
        }
    }

    // ── the live song library ──────────────────────────────────
    const LIVE_SONGS: [&str; 8] = [
        "Rise and Shine",
        "Every Morning New",
        "Anchor Line",
        "Wildfire Praise",
        "Steady Ground",
        "Open Doors",
        "Brighter Still",
        "Carry Me Home",
    ];
    let songs_dir = vault.join("Songs");
    std::fs::create_dir_all(&songs_dir)?;
    for (i, song) in LIVE_SONGS.iter().enumerate() {
        write_if_absent(
            &songs_dir.join(format!("{song}.md")),
            &song_note(song, "FTS Collective", None, 50 + i),
        )?;
    }

    // ── setlists ───────────────────────────────────────────────
    let setlists = vault.join("Setlists");
    std::fs::create_dir_all(&setlists)?;
    write_if_absent(
        &setlists.join("Sunday Setlist.md"),
        "---\ntitle: Sunday Setlist\ntype: setlist\n---\n\n# Sunday Setlist\n\n\
         1. [[Rise and Shine]]\n2. [[Every Morning New]]\n3. [[Anchor Line]]\n4. [[Steady Ground]]\n5. [[Carry Me Home]]\n",
    )?;
    write_if_absent(
        &setlists.join("Album Release Show.md"),
        "---\ntitle: Album Release Show\ntype: setlist\n---\n\n# Album Release Show\n\n\
         1. [[Static Heart]]\n2. [[Neon Run]]\n3. [[Afterglow]]\n4. [[Signal Fade]]\n5. [[City Sleeps]]\n6. [[Brighter Still]]\n",
    )?;

    // ── events (Records/events, setlist experience) ────────────
    // Basenames stay unique vault-wide (wikilink resolution is
    // basename-keyed): the event is "… — Fox Theater", never a twin
    // of the "Album Release Show" setlist above.
    let events = vault.join("Records").join("events");
    std::fs::create_dir_all(&events)?;
    write_if_absent(
        &events.join("Sunday Service.md"),
        "---\ntitle: Sunday Service\ntype: event\nexperience: setlist\ntabs: true\nstart: 2026-08-16T09:00:00Z\nend: 2026-08-16T11:00:00Z\n---\n\n\
         # Sunday Service\n\n## Walk-in\n\n- [[Brighter Still]]\n\n## Set\n\n\
         1. [[Rise and Shine]]\n2. [[Every Morning New]]\n3. [[Anchor Line]]\n4. [[Steady Ground]]\n5. [[Carry Me Home]]\n\n\
         ## Notes\n\nSeeded demo event — the set mirrors [[Sunday Setlist]].\n",
    )?;
    write_if_absent(
        &events.join("Album Release Show — Fox Theater.md"),
        "---\ntitle: Album Release Show — Fox Theater\ntype: event\nexperience: setlist\ntabs: true\nstart: 2026-08-22T19:30:00Z\nend: 2026-08-22T22:00:00Z\n---\n\n\
         # Album Release Show — Fox Theater\n\n## Doors\n\n- Playlist: label picks\n\n## Set\n\n\
         1. [[Static Heart]]\n2. [[Neon Run]]\n3. [[Afterglow]]\n4. [[Signal Fade]]\n5. [[City Sleeps]]\n6. [[Brighter Still]]\n\n\
         ## Production\n\nSeeded demo event for [[Midnight Static]].\n",
    )?;

    println!(
        "  vault: {planted} projects, {albums} albums ({songs} songs), 8 library songs, 2 setlists, 2 events",
        albums = ALBUMS.len(),
        songs = ALBUMS.iter().map(|(_, _, s)| s.len()).sum::<usize>(),
    );
    Ok(())
}

/// One slug for every demo root dir: lowercase, every non-alnum run
/// collapsed to a single '-'. Two ad-hoc spellings of this drifted
/// once already ("Roots & Wires" kept its literal '&' on disk).
#[cfg(debug_assertions)]
fn demo_slug(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_owned()
}

/// Whether ffmpeg is runnable — real tiny media when it is, labeled
/// placeholder bytes when not (everything still browses; only
/// playback needs the real thing).
#[cfg(debug_assertions)]
fn ffmpeg_available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Write a small real MP4 (testsrc + tone) at `path`; placeholder
/// bytes when ffmpeg is missing. `variant` changes the picture so
/// successive checkpoints have genuinely different content.
#[cfg(debug_assertions)]
fn seed_video_file(path: &std::path::Path, ffmpeg: bool, variant: u32) -> eyre::Result<()> {
    if ffmpeg {
        let duration = 3 + (variant % 3);
        let hue = (variant * 47) % 360;
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi"])
            .arg("-i")
            .arg(format!("testsrc2=duration={duration}:size=640x360:rate=24"))
            .args(["-f", "lavfi"])
            .arg("-i")
            .arg(format!("sine=frequency={}:duration={duration}", 220 + hue))
            .args(["-vf", &format!("hue=h={hue}")])
            .args(["-pix_fmt", "yuv420p", "-shortest"])
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .wrap_err("run ffmpeg (video)")?;
        if status.success() {
            return Ok(());
        }
    }
    std::fs::write(path, format!("seeded placeholder video v{variant}"))?;
    Ok(())
}

/// Write a short real WAV (sine) at `path`; placeholder bytes when
/// ffmpeg is missing.
#[cfg(debug_assertions)]
fn seed_audio_file(path: &std::path::Path, ffmpeg: bool, freq: u32) -> eyre::Result<()> {
    if ffmpeg {
        let status = std::process::Command::new("ffmpeg")
            .args(["-y", "-f", "lavfi"])
            .arg("-i")
            .arg(format!("sine=frequency={freq}:duration=2"))
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .wrap_err("run ffmpeg (audio)")?;
        if status.success() {
            return Ok(());
        }
    }
    std::fs::write(path, format!("seeded placeholder audio {freq}hz"))?;
    Ok(())
}

/// A batch of independent media writes, run on the blocking pool in
/// parallel. A fresh seed is ~97 encodes; strictly sequential they
/// were the whole wall time.
#[cfg(debug_assertions)]
async fn run_media_jobs(
    jobs: Vec<Box<dyn FnOnce() -> eyre::Result<()> + Send>>,
) -> eyre::Result<()> {
    let mut set = tokio::task::JoinSet::new();
    for job in jobs {
        set.spawn_blocking(job);
    }
    while let Some(joined) = set.join_next().await {
        joined.map_err(|e| eyre::eyre!("media job: {e}"))??;
    }
    Ok(())
}

/// Media-bearing Files roots for the studio: one per video project
/// (2 checkpoints, Named Versions on half, divergences on two) and
/// one per album (per-song folders with mixes + stems). Existing
/// roots are TOPPED UP, not skipped — an interrupted prior run (the
/// encode window is seconds long) must not leave a root permanently
/// half-dressed.
#[cfg(debug_assertions)]
async fn seed_studio_files(org: &org_proto::OrgRoot, with_divergence: bool) -> eyre::Result<()> {
    let files_dir = org.path().join("files");
    let backend = files::FilesBackend::new(&files_dir, org.vault_dir())
        .map_err(|e| eyre::eyre!("open files backend: {e}"))?;
    let result = seed_studio_files_inner(&backend, &files_dir, with_divergence).await;
    backend.shutdown().await;
    result
}

#[cfg(debug_assertions)]
async fn seed_studio_files_inner(
    backend: &files::FilesBackend,
    files_dir: &std::path::Path,
    with_divergence: bool,
) -> eyre::Result<()> {
    let existing: std::collections::HashMap<String, files::FileRootInfo> =
        files::FilesService::list_roots(backend)
            .await
            .map_err(|e| eyre::eyre!("list roots: {e:?}"))?
            .into_iter()
            .map(|r| (r.name.clone(), r))
            .collect();
    let ffmpeg = ffmpeg_available();
    if !ffmpeg {
        println!(
            "  files: ffmpeg not on PATH — placeholder media bytes (browsing works, playback won't)"
        );
    }

    // "Dressed" = the second-revision checkpoint landed: the probe
    // file's chain has two entries. Chain length is the marker (not
    // Named Versions — half the roots deliberately never get one,
    // and re-dressing those every run would grow their chains).
    async fn dressed(
        backend: &files::FilesBackend,
        root_id: uuid::Uuid,
        probe: &str,
    ) -> eyre::Result<bool> {
        Ok(
            files::FilesService::chain(backend, root_id, probe.to_owned())
                .await
                .map_err(|e| eyre::eyre!("chain {probe}: {e:?}"))?
                .len()
                >= 2,
        )
    }

    let mut planted = 0usize;
    let mut topped_up = 0usize;
    for (i, name) in VIDEO_PROJECTS.iter().enumerate() {
        let root_dir = files_dir.join(format!("video-{}", demo_slug(name)));
        let variant = u32::try_from(i).unwrap_or(0);
        let wants_name = i % 2 == 0;
        let wants_divergence = with_divergence && i < 2;

        let root = match existing.get(*name) {
            Some(root) => root.clone(),
            None => {
                std::fs::create_dir_all(root_dir.join("broll"))?;
                let mut jobs: Vec<Box<dyn FnOnce() -> eyre::Result<()> + Send>> = Vec::new();
                let (cut, aerial, vo) = (
                    root_dir.join("cut.mp4"),
                    root_dir.join("broll").join("aerial.mp4"),
                    root_dir.join("vo-scratch.wav"),
                );
                jobs.push(Box::new(move || seed_video_file(&cut, ffmpeg, variant)));
                jobs.push(Box::new(move || {
                    seed_video_file(&aerial, ffmpeg, variant + 40)
                }));
                jobs.push(Box::new(move || {
                    seed_audio_file(&vo, ffmpeg, 200 + variant * 20)
                }));
                run_media_jobs(jobs).await?;
                std::fs::write(
                    root_dir.join("notes.md"),
                    format!("# {name}\n\n- assembly cut\n"),
                )?;

                let root = files::FilesService::create_root(
                    backend,
                    root_dir.to_string_lossy().into_owned(),
                    (*name).to_owned(),
                    files::RootFlavor::Media,
                )
                .await
                .map_err(|e| eyre::eyre!("create root {name}: {e:?}"))?;
                files::FilesService::checkpoint_now(
                    backend,
                    root.id,
                    Some("Initial import".to_owned()),
                )
                .await
                .map_err(|e| eyre::eyre!("checkpoint {name}: {e:?}"))?;
                planted += 1;
                root
            }
        };

        // Second-revision dressing, keyed on the probe chain so an
        // interrupted run completes here and a complete one is a
        // no-op.
        if !dressed(backend, root.id, "cut.mp4").await? {
            if existing.contains_key(*name) {
                topped_up += 1;
            }
            seed_video_file(&root_dir.join("cut.mp4"), ffmpeg, variant + 100)?;
            std::fs::write(
                root_dir.join("notes.md"),
                format!("# {name}\n\n- assembly cut\n- rough cut: tightened intro\n"),
            )?;
            let cp2 =
                files::FilesService::checkpoint_now(backend, root.id, Some("Rough cut".to_owned()))
                    .await
                    .map_err(|e| eyre::eyre!("checkpoint 2 {name}: {e:?}"))?;
            if wants_name {
                files::FilesService::name_version(
                    backend,
                    root.id,
                    cp2.commit_id.clone(),
                    "Rough Cut v1".to_owned(),
                )
                .await
                .map_err(|e| eyre::eyre!("name version {name}: {e:?}"))?;
            }
        }

        // Divergence top-up (the #267 UI's fixture) — on the NOTES
        // file, so the playable cut stays playable.
        if wants_divergence {
            let has = files::FilesService::divergences(backend, root.id)
                .await
                .map_err(|e| eyre::eyre!("divergences {name}: {e:?}"))?
                .iter()
                .any(|d| d.path == "notes.md");
            if !has {
                backend
                    .seed_divergent_file(
                        root.id,
                        "notes.md",
                        format!("# {name}\n\n- rough cut (producer pass)\n").as_bytes(),
                        format!("# {name}\n\n- rough cut (director pass)\n").as_bytes(),
                    )
                    .await
                    .map_err(|e| eyre::eyre!("seed divergence {name}: {e:?}"))?;
            }
        }
    }

    for (album, _artist, songs) in ALBUMS {
        let root_name = format!("Album — {album}");
        let root_dir = files_dir.join(format!("album-{}", demo_slug(album)));

        let root = match existing.get(&root_name) {
            Some(root) => root.clone(),
            None => {
                let mut jobs: Vec<Box<dyn FnOnce() -> eyre::Result<()> + Send>> = Vec::new();
                for (j, song) in songs.iter().enumerate() {
                    let song_dir = root_dir.join(song);
                    std::fs::create_dir_all(song_dir.join("stems"))?;
                    let base = 180 + u32::try_from(j).unwrap_or(0) * 30;
                    let (mix, drums, bass) = (
                        song_dir.join("mix.wav"),
                        song_dir.join("stems").join("drums.wav"),
                        song_dir.join("stems").join("bass.wav"),
                    );
                    jobs.push(Box::new(move || seed_audio_file(&mix, ffmpeg, base)));
                    jobs.push(Box::new(move || seed_audio_file(&drums, ffmpeg, base + 5)));
                    jobs.push(Box::new(move || seed_audio_file(&bass, ffmpeg, base / 2)));
                }
                run_media_jobs(jobs).await?;
                std::fs::write(
                    root_dir.join("album-notes.md"),
                    format!("# {album}\n\nTracking + mix files, one folder per song.\n"),
                )?;

                let root = files::FilesService::create_root(
                    backend,
                    root_dir.to_string_lossy().into_owned(),
                    root_name.clone(),
                    files::RootFlavor::Media,
                )
                .await
                .map_err(|e| eyre::eyre!("create root {root_name}: {e:?}"))?;
                files::FilesService::checkpoint_now(backend, root.id, Some("Tracking".to_owned()))
                    .await
                    .map_err(|e| eyre::eyre!("checkpoint {root_name}: {e:?}"))?;
                planted += 1;
                root
            }
        };

        // Mix revision on the first song, named — keyed on the probe
        // chain like the video roots, so interruptions heal.
        if let Some(first) = songs.first() {
            if !dressed(backend, root.id, &format!("{first}/mix.wav")).await? {
                if existing.contains_key(&root_name) {
                    topped_up += 1;
                }
                seed_audio_file(&root_dir.join(first).join("mix.wav"), ffmpeg, 445)?;
                let cp = files::FilesService::checkpoint_now(
                    backend,
                    root.id,
                    Some("Mix v1".to_owned()),
                )
                .await
                .map_err(|e| eyre::eyre!("checkpoint 2 {root_name}: {e:?}"))?;
                files::FilesService::name_version(
                    backend,
                    root.id,
                    cp.commit_id,
                    "Mix v1".to_owned(),
                )
                .await
                .map_err(|e| eyre::eyre!("name version {root_name}: {e:?}"))?;
            }
        }
    }

    println!("  files: {planted} studio roots planted, {topped_up} topped up");
    Ok(())
}

/// `admin webdav --org <slug> [--hide <root-id>|--show <root-id>]` —
/// the operator surface for "a per-root policy can hide a root from
/// WebDAV" (issue #274).
///
/// The policy lives in a JSON file beside the org's Files registry, and
/// that file is deliberately the source of truth (see
/// `files_webdav::WebdavPolicy`): hiding a root is an operator decision
/// on a compat surface, not something the RPC contract should carry. But
/// "edit this JSON by hand" is not an operator surface, so this verb is
/// the one that writes it correctly — and, with no flag, answers the
/// question an operator actually has, which is "what is exposed right
/// now?". The running server re-reads the file on its next request; no
/// restart, no signal.
///
/// Authorization is filesystem ownership of the data root, exactly like
/// every other verb in this module.
fn webdav(args: &[String]) -> eyre::Result<()> {
    let Some(slug) = flag(args, "--org") else {
        bail!("--org <slug> is required");
    };
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    let files_dir = data_root.org(&slug).path().join("files");
    if !files_dir.is_dir() {
        bail!("org `{slug}` has no Files area at {}", files_dir.display());
    }
    let backend = files::FilesBackend::new(&files_dir, data_root.org(&slug).path().join("vault"))
        .map_err(|e| eyre::eyre!("open files backend for `{slug}`: {e}"))?;
    let policy = files_webdav::WebdavPolicy::open(&files_dir);

    for (name, hide) in [("--hide", true), ("--show", false)] {
        if let Some(raw) = flag(args, name) {
            let id = raw
                .parse::<uuid::Uuid>()
                .wrap_err_with(|| format!("{name} takes a root id, got `{raw}`"))?;
            // Refuse an id this org does not have — a typo'd uuid would
            // otherwise be accepted silently and hide nothing.
            let roots = pollster::block_on(files::FilesService::list_roots(&backend))
                .map_err(|e| eyre::eyre!("list roots: {e}"))?;
            if !roots.iter().any(|r| r.id == id) {
                bail!("org `{slug}` has no File Root {id}");
            }
            policy
                .set_hidden(id, hide)
                .wrap_err_with(|| format!("write webdav policy for `{slug}`"))?;
            println!(
                "{slug}: root {id} is now {} on WebDAV",
                if hide { "hidden" } else { "visible" }
            );
            return Ok(());
        }
    }

    let roots = pollster::block_on(files::FilesService::list_roots(&backend))
        .map_err(|e| eyre::eyre!("list roots: {e}"))?;
    if roots.is_empty() {
        println!("{slug}: no File Roots");
        return Ok(());
    }
    let hidden = policy
        .hidden_set()
        .wrap_err_with(|| format!("read webdav policy for `{slug}`"))?;
    println!("{slug}: WebDAV policy at {}", policy.path().display());
    for root in roots {
        println!(
            "  {}  {:<8}  {}",
            root.id,
            if hidden.contains(&root.id) {
                "hidden"
            } else {
                "visible"
            },
            root.name,
        );
    }
    Ok(())
}

#[cfg(test)]
mod merge_principals_tests {
    use super::*;

    fn u(n: u128) -> uuid::Uuid {
        uuid::Uuid::from_u128(n)
    }

    #[test]
    fn the_home_org_supplies_the_canonical_id() {
        let orgs = vec![
            ("cbu".into(), false, vec![(u(2), Some("a@b.com".into()))]),
            (
                "codywright".into(),
                true,
                vec![(u(1), Some("a@b.com".into()))],
            ),
        ];
        let (principals, _) = plan_principals(&orgs);
        assert_eq!(principals.len(), 1);
        assert_eq!(principals[0].canonical.user_id, u(1));
        assert_eq!(principals[0].canonical.slug, "codywright");
        assert_eq!(principals[0].absorbed.len(), 1);
        assert_eq!(principals[0].absorbed[0].slug, "cbu");
    }

    #[test]
    fn without_a_home_account_the_first_slug_wins_deterministically() {
        // Directory iteration order must not decide which id survives.
        let orgs = vec![
            ("zeta".into(), false, vec![(u(9), Some("a@b.com".into()))]),
            ("alpha".into(), false, vec![(u(3), Some("a@b.com".into()))]),
        ];
        let (principals, _) = plan_principals(&orgs);
        assert_eq!(principals[0].canonical.slug, "alpha");
        assert_eq!(principals[0].absorbed[0].slug, "zeta");
    }

    #[test]
    fn email_case_does_not_split_a_principal() {
        let orgs = vec![
            (
                "codywright".into(),
                true,
                vec![(u(1), Some("Cody@Example.com".into()))],
            ),
            (
                "cbu".into(),
                false,
                vec![(u(2), Some("cody@example.com".into()))],
            ),
        ];
        let (principals, _) = plan_principals(&orgs);
        assert_eq!(principals.len(), 1, "one login, one principal");
        // Reported in the casing the home org stored — what was typed.
        assert_eq!(principals[0].email, "Cody@Example.com");
    }

    #[test]
    fn a_single_org_account_is_a_principal_with_nothing_absorbed() {
        let orgs = vec![(
            "tombrooksmusic".into(),
            false,
            vec![(u(7), Some("carter@x.invalid".into()))],
        )];
        let (principals, _) = plan_principals(&orgs);
        assert_eq!(principals[0].absorbed, vec![]);
        assert_eq!(principals[0].org_slugs(), vec!["tombrooksmusic".to_owned()]);
    }

    #[test]
    fn an_account_without_an_email_is_reported_not_merged() {
        let orgs = vec![("cbu".into(), false, vec![(u(5), None)])];
        let (principals, warnings) = plan_principals(&orgs);
        assert!(principals.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("no email"));
    }
}
