//! `task auth …` — architect-auth sign-in / session / org selection.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

use clap::Subcommand;

use crate::establish_for_url;
use crate::global_org;
use crate::resolve_org_vox_url;
use crate::resolve_server_base;

#[derive(Subcommand)]
pub(crate) enum AuthCmd {
    /// Sign in over the server's per-org `AuthService`
    /// (`<server>/org/<slug>/vox`) — works against a remote
    /// server with NO local org dir. Persists the session
    /// (token + user + org + server URL) to
    /// `$XDG_DATA_HOME/task/session.json`, so subsequent
    /// commands need nothing else: the stored server URL is
    /// used whenever `--server` / `TASK_VOX_URL` is absent.
    Login {
        /// Prompted for when omitted, if stdin is a terminal.
        #[arg(long)]
        email: Option<String>,
        /// Reads `TASK_PASSWORD` when the flag is omitted; prompted
        /// for (with echo off) when neither is set and stdin is a
        /// terminal — the best option of the three, since it reaches
        /// neither `ps` nor the environment.
        ///
        /// Prefer the env var over the flag in scripts: anything
        /// passed as an argument is visible to `ps` for the lifetime
        /// of the process, and lands in shell history. The env var is
        /// readable only by processes that can already read this
        /// one's environment.
        #[arg(long, env = "TASK_PASSWORD", hide_env_values = true)]
        password: Option<String>,
    },
    /// Create a new email/password user over the org's
    /// `AuthService` and persist the resulting session — like
    /// `login`, purely remote. The first user signed up in a
    /// fresh org is its de-facto owner — architect-auth has no
    /// separate ownership concept yet. Use `--org <slug>` /
    /// `--server <url>` to target a specific org.
    Signup {
        /// Prompted for when omitted, if stdin is a terminal.
        #[arg(long)]
        email: Option<String>,
        /// Prompted for (twice, with echo off) when omitted and
        /// stdin is a terminal — there's no password reset flow, so
        /// a typo here is a locked-out account.
        #[arg(long)]
        password: Option<String>,
        /// Optional username — needed if you want
        /// `SignInUsername` to work later. Free-form, but the
        /// architect-auth username uniqueness constraint
        /// applies per `auth.sqlite`.
        #[arg(long)]
        username: Option<String>,
        /// Optional display name. Falls back to the email
        /// localpart in the UI when empty.
        #[arg(long)]
        name: Option<String>,
    },
    /// Link another (server, org) to your home identity.
    ///
    /// Signs you in to the target once, then stores the resulting
    /// session token — encrypted at rest — in your HOME org's identity
    /// locker, keyed by `(home_user, remote_url, remote_slug)`. The
    /// home server can then act on your behalf there, and every client
    /// signed into home learns which orgs you have without being told.
    ///
    /// The local `session.json` entry is written too, so `auth use`
    /// switches to it immediately.
    ///
    /// Same-server orgs are a legitimate target: the locker keys on
    /// (url, slug), so six orgs on one host are six links.
    Link {
        /// Server base URL. Defaults to the home server.
        #[arg(long)]
        server: Option<String>,
        /// Org slug to link. Omitted = pick from the server's
        /// well-known org list.
        #[arg(long)]
        org: Option<String>,
        /// Sign-in email for the target. Prompted when omitted.
        #[arg(long)]
        email: Option<String>,
        /// Prompted (echo off) when omitted; reads `TASK_PASSWORD`.
        #[arg(long, env = "TASK_PASSWORD", hide_env_values = true)]
        password: Option<String>,
        /// Human label for the link. Defaults to the org's display name.
        #[arg(long)]
        label: Option<String>,
    },
    /// Every org linked to your home identity, from the locker —
    /// including ones this machine has never signed into.
    Links {
        #[arg(long)]
        server: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Drop a link from your home identity's locker by id (see `links`).
    Unlink {
        id: String,
        #[arg(long)]
        server: Option<String>,
    },
    /// Show the authoritative profile your home org holds.
    Profile {
        #[arg(long)]
        server: Option<String>,
    },
    /// Set your display name / avatar once, on home, and push it to
    /// every linked org.
    ///
    /// The home copy is authoritative; each linked org keeps a cache so
    /// it keeps working when home is unreachable. Passing neither field
    /// re-pushes the current profile — the repair path for a link that
    /// was down during an earlier edit.
    ///
    /// An empty string clears a field; omitting it leaves it alone.
    SetProfile {
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        image: Option<String>,
        #[arg(long)]
        server: Option<String>,
    },
    /// Print the active session (email, user id, org id).
    Whoami,
    /// Switch the active session entry (server profile) without
    /// re-authenticating. `task auth whoami` lists the stored
    /// entries; reference one by key (`slug@host`), bare slug, or
    /// any unique prefix. Subsequent commands talk to that
    /// entry's server unless `--server` / `TASK_VOX_URL` says
    /// otherwise.
    Use {
        /// Session reference — key, slug, or unique prefix.
        session: String,
    },
    /// Invalidate the active session server-side AND remove
    /// the local session file.
    Logout,
    /// Org membership + selection.
    #[command(subcommand)]
    Org(AuthOrgCmd),
    /// List every user in the active org's `auth.sqlite`.
    /// Useful when you need a user_id to pass to
    /// `timer reassign-user --to`.
    Users,
    /// Move an account onto a different email, keeping its user id and
    /// recording the change. The id is what tasks, timers and authorship
    /// are keyed on, so this is a rename — NOT a new account.
    ///
    /// Identify the account by `--user <uuid>` or `--email <current>`.
    /// Each org has its own auth store, so run this once per org
    /// (`--org <slug>`).
    MigrateEmail {
        /// The account's user id in THIS org (`task auth users`).
        #[arg(long, conflicts_with = "email")]
        user: Option<uuid::Uuid>,
        /// The account's current email, as an alternative to `--user`.
        #[arg(long, conflicts_with = "user")]
        email: Option<String>,
        /// The address to move to.
        #[arg(long)]
        to: String,
        /// Recorded on the history row — worth setting for bulk moves.
        #[arg(long)]
        reason: Option<String>,
    },
    /// Show every email an account has held, oldest first.
    EmailHistory {
        #[arg(long, conflicts_with = "email")]
        user: Option<uuid::Uuid>,
        #[arg(long, conflicts_with = "user")]
        email: Option<String>,
    },
}

#[derive(Subcommand)]
pub(crate) enum AuthOrgCmd {
    /// List orgs the signed-in user is a member of.
    List,
    /// Set the active org for subsequent commands. Updates
    /// both the local session file and the server-side
    /// `auth_session.active_organization_id`.
    Use {
        /// Org reference — UUID, slug, or name (exact / unique
        /// prefix), matched against your memberships.
        org_id: String,
    },
}

/// Open `ArchitectAuth` against a specific org's
/// `auth.sqlite` — same DB the server uses for that org.
/// CLI ↔ server interop hinges on matching the
/// `<data_root>/orgs/<slug>/auth.sqlite` resolver plus
/// `DEFAULT_AUTH_SECRET`.
async fn open_local_auth(
    auth_db_path: &std::path::Path,
) -> eyre::Result<architect_auth::ArchitectAuth<architect_auth::db::AuthSeaOrmStorage>> {
    use architect_auth::db::{AuthSeaOrmStorage, Migrator as AuthMigrator};
    use sea_orm::Database;
    use sea_orm_migration::MigratorTrait;
    let db_url = format!("sqlite://{}?mode=rwc", auth_db_path.display());
    let db = Database::connect(&db_url)
        .await
        .map_err(|e| eyre::eyre!("connect auth db `{db_url}`: {e}"))?;
    AuthMigrator::up(&db, None)
        .await
        .map_err(|e| eyre::eyre!("auth migrations: {e}"))?;
    let storage = AuthSeaOrmStorage::new(db);
    architect_auth::ArchitectAuth::builder()
        .secret(crate::session_store::DEFAULT_AUTH_SECRET)
        .storage(storage)
        .build()
        .map_err(|e| eyre::eyre!("build ArchitectAuth: {e}"))
}

/// One hosted org row from a server's discovery document.
#[derive(Debug, serde::Deserialize)]
struct HostedOrg {
    slug: String,
    #[serde(default)]
    is_home: bool,
}

/// `ws(s)://` vox base → `http(s)://` origin+path for the same
/// server (the well-known + health endpoints live on plain HTTP).
pub(crate) fn ws_base_to_http(base: &str) -> String {
    base.replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1)
}

/// Fetch the server's `/.well-known/task-server.json` org list —
/// the remote replacement for scanning `<data_root>/orgs/`.
async fn fetch_hosted_orgs(base: &str) -> eyre::Result<Vec<HostedOrg>> {
    let url = format!(
        "{}/.well-known/task-server.json",
        ws_base_to_http(&crate::session_store::normalize_server_base(base))
    );
    let doc: serde_json::Value = reqwest::get(&url)
        .await
        .map_err(|e| eyre::eyre!("fetch {url}: {e}"))?
        .json()
        .await
        .map_err(|e| eyre::eyre!("parse {url}: {e}"))?;
    let orgs = doc
        .get("orgs")
        .cloned()
        .ok_or_else(|| eyre::eyre!("{url}: no `orgs` field"))?;
    serde_json::from_value(orgs).map_err(|e| eyre::eyre!("{url}: bad `orgs` shape: {e}"))
}

/// Resolve which (org slug, server base) the remote auth verbs
/// operate on. Purely remote — requires NO local org dir:
///
/// - server: [`resolve_server_base`] precedence (flag > env >
///   session > localhost).
/// - slug: `--org` > the session entry for that server > the
///   server's well-known org list (its home org, or the single
///   hosted org).
///
/// When discovery works, an unknown slug fails here with the
/// hosted list — clearer than a raw vox connect error. When the
/// well-known endpoint is unreachable the vox connect downstream
/// reports the connection failure with its own taxonomy.
/// A client for the home server's identity locker, plus the home
/// session entry it authenticates with. The locker lives on HOME, so
/// every locker verb needs both.
async fn home_identity_client(
    server: Option<String>,
) -> eyre::Result<(
    identity_proto::IdentityServiceClient,
    crate::session_store::ServerEntry,
)> {
    let home = crate::session_store::load()?
        .and_then(|s| s.home_entry().cloned())
        .ok_or_else(|| {
            eyre::eyre!("no home session — run `task auth login` against your home server first")
        })?;
    let base = server.unwrap_or_else(|| home.url.clone());
    let (client, _endpoint) = crate::establish_server_client(Some(&base)).await?;
    Ok((client, home))
}

/// Choose an org from a server's well-known list. Non-interactive
/// callers must pass `--org`; a picker with nobody watching would hang.
async fn pick_org_interactively(base: &str) -> eyre::Result<String> {
    let hosted = fetch_hosted_orgs(base).await?;
    if hosted.is_empty() {
        return Err(eyre::eyre!("{base} hosts no orgs"));
    }
    if !crate::shared::stdin_is_tty() {
        return Err(eyre::eyre!(
            "pass `--org <slug>` (stdin is not a terminal, so there's nobody to pick): {}",
            hosted
                .iter()
                .map(|o| o.slug.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    println!("Orgs on {base}:");
    for (i, o) in hosted.iter().enumerate() {
        println!(
            "  {}) {}{}",
            i + 1,
            o.slug,
            if o.is_home { "  (home)" } else { "" }
        );
    }
    let pick = crate::shared::prompt_line("Number or slug")?;
    if let Ok(n) = pick.parse::<usize>() {
        return hosted
            .get(n.wrapping_sub(1))
            .map(|o| o.slug.clone())
            .ok_or_else(|| eyre::eyre!("no such choice: {n}"));
    }
    hosted
        .iter()
        .find(|o| o.slug == pick)
        .map(|o| o.slug.clone())
        .ok_or_else(|| eyre::eyre!("`{pick}` is not an org on {base}"))
}

/// The email to sign in/up with: the flag, else a prompt when a
/// person is watching. Piped stdin gets the flag error instead of a
/// read that would hang a script forever.
fn resolve_email(flag: Option<String>) -> eyre::Result<String> {
    if let Some(email) = flag.map(|e| e.trim().to_owned()).filter(|e| !e.is_empty()) {
        return Ok(email);
    }
    if !crate::shared::stdin_is_tty() {
        return Err(eyre::eyre!(
            "no email — pass `--email <address>` (stdin is not a terminal, so there's nobody to prompt)"
        ));
    }
    let email = crate::shared::prompt_line("Email")?;
    if email.is_empty() {
        return Err(eyre::eyre!("no email entered"));
    }
    Ok(email)
}

/// The password: the flag or `TASK_PASSWORD`, else a hidden prompt.
/// `confirm` asks twice and compares — used on signup, where there is
/// no reset flow, so a typo is a locked-out account. Sign-in doesn't
/// need it: a wrong password just fails to authenticate.
fn resolve_password(flag: Option<String>, confirm: bool) -> eyre::Result<String> {
    if let Some(password) = flag.filter(|p| !p.is_empty()) {
        return Ok(password);
    }
    if !crate::shared::stdin_is_tty() {
        return Err(eyre::eyre!(
            "no password — pass `--password` or set `TASK_PASSWORD` (stdin is not a terminal, so there's nobody to prompt)"
        ));
    }
    let password = crate::shared::prompt_secret("Password")?;
    if password.is_empty() {
        return Err(eyre::eyre!("no password entered"));
    }
    if confirm && crate::shared::prompt_secret("Confirm password")? != password {
        return Err(eyre::eyre!("passwords didn't match"));
    }
    Ok(password)
}

async fn resolve_auth_target(org_override: Option<&str>) -> eyre::Result<(String, String)> {
    let base = resolve_server_base(None);
    let hosted = fetch_hosted_orgs(&base).await.ok();
    let chosen = if let Some(s) = org_override.map(str::to_owned).or_else(global_org) {
        Some(s)
    } else if let Some((_, entry)) = crate::session_store::load()?
        .as_ref()
        .and_then(|s| s.entry_for_server(&base))
    {
        Some(entry.slug.clone())
    } else {
        // No flag, no session: let the server disambiguate — its
        // home org, or the only org it hosts.
        hosted.as_ref().and_then(|orgs| {
            orgs.iter()
                .find(|o| o.is_home)
                .or_else(|| (orgs.len() == 1).then(|| &orgs[0]))
                .map(|o| o.slug.clone())
        })
    };
    let Some(slug) = chosen else {
        return Err(crate::errors::usage("resolve org for auth")
            .cause(format!(
                "no `--org` given and nothing to infer it from ({base})"
            ))
            .hint("pass --org <slug> (see `task org list --server …` for what the server hosts)")
            .report());
    };
    if let Some(orgs) = &hosted {
        if !orgs.iter().any(|o| o.slug == slug) {
            let names: Vec<&str> = orgs.iter().map(|o| o.slug.as_str()).collect();
            return Err(crate::errors::not_found("resolve org on server", &slug)
                .cause(format!("{base} hosts: {}", names.join(", ")))
                .hint("pass --org <slug> from that list, or `task org create` it first")
                .report());
        }
    }
    Ok((slug, base))
}

pub(crate) async fn run_auth(cmd: AuthCmd, org_override: Option<&str>) -> eyre::Result<()> {
    use architect_auth::commands::CurrentSession;
    use architect_auth::proto::{AuthServiceClient, SignInEmailPassword, SignUpEmailPassword};
    match cmd {
        AuthCmd::Signup {
            email,
            password,
            username,
            name,
        } => {
            let email = resolve_email(email)?;
            let password = resolve_password(password, true)?;
            // Remote-first: sign up over the org's AuthService —
            // the same per-org vox endpoint every other service
            // rides. No local org dir required.
            let (slug, base) = resolve_auth_target(org_override).await?;
            let url = resolve_org_vox_url(Some(base.clone()), &slug);
            let client: AuthServiceClient = establish_for_url(&url).await?;
            let bundle = client
                .sign_up_email_password(SignUpEmailPassword {
                    email: email.clone(),
                    password,
                    name: name.clone(),
                    username: username.clone(),
                    image: None,
                    metadata_json: None,
                    ip_address: None,
                    user_agent: Some("task-cli".into()),
                })
                .await
                .map_err(|e| eyre::eyre!("sign up: {e}"))?;
            let resolved_email = bundle.user.email.clone().unwrap_or_else(|| email.clone());
            // Persist the session keyed by (server, org) — same
            // shape as `Login` so subsequent commands work
            // without a follow-up `task auth login`.
            let mut sess = crate::session_store::load()?
                .unwrap_or_else(crate::session_store::CliSession::empty);
            let key = sess.record_login(
                &slug,
                &base,
                bundle.user.id,
                resolved_email.clone(),
                bundle.token.clone(),
            );
            crate::session_store::save(&sess)?;
            println!(
                "Created user {} ({}) in org `{slug}`",
                resolved_email, bundle.user.id,
            );
            if let Some(u) = username {
                println!("  username: {u}");
            }
            if let Some(n) = name {
                println!("  name:     {n}");
            }
            println!("  server:   {base}");
            println!("  session:  {key}");
        }
        AuthCmd::Link {
            server,
            org,
            email,
            password,
            label,
        } => {
            // The locker lives on HOME. Linking without a home session
            // has nowhere to write, so say that rather than failing
            // deeper with an auth error.
            let home = crate::session_store::load()?
                .and_then(|s| s.home_entry().cloned())
                .ok_or_else(|| {
                    eyre::eyre!(
                        "no home session — run `task auth login` against your home server first"
                    )
                })?;

            let base = crate::session_store::normalize_server_base(
                &server.unwrap_or_else(|| home.url.clone()),
            );
            let slug = match org {
                Some(s) => s,
                None => pick_org_interactively(&base).await?,
            };

            let email = resolve_email(email)?;
            let password = resolve_password(password, false)?;

            // One sign-in against the target, exactly as `login` does.
            let url = resolve_org_vox_url(Some(base.clone()), &slug);
            let client: AuthServiceClient = establish_for_url(&url).await?;
            let bundle = client
                .sign_in_email_password(SignInEmailPassword {
                    email: email.clone(),
                    password,
                    ip_address: None,
                    user_agent: Some("task-cli link".into()),
                })
                .await
                .map_err(|e| eyre::eyre!("sign in to `{slug}` on {base}: {e}"))?;
            let resolved_email = bundle.user.email.clone().unwrap_or_else(|| email.clone());

            // Stash it in the home locker, encrypted at rest there.
            let (identity, _endpoint): (identity_proto::IdentityServiceClient, String) =
                crate::establish_server_client(Some(&home.url)).await?;
            let view = identity
                .link_server(identity_proto::LinkServerRequest {
                    session_token: home.token.clone(),
                    label: label.unwrap_or_else(|| slug.clone()),
                    remote_url: base.clone(),
                    remote_slug: slug.clone(),
                    remote_user_id: Some(bundle.user.id),
                    remote_email: Some(resolved_email.clone()),
                    token: Some(bundle.token.clone()),
                    expires_at: None,
                })
                .await
                .map_err(|e| eyre::eyre!("store the link on home ({}): {e:?}", home.url))?;

            // And locally, so `auth use` reaches it without a round trip.
            let mut sess = crate::session_store::load()?
                .unwrap_or_else(crate::session_store::CliSession::empty);
            let key = sess.record_login(
                &slug,
                &base,
                bundle.user.id,
                resolved_email.clone(),
                bundle.token,
            );
            crate::session_store::save(&sess)?;

            println!("Linked `{slug}` on {base} to your home identity");
            println!("  as:      {resolved_email} ({})", bundle.user.id);
            println!("  link id: {}", view.id);
            println!("  session: {key}   (switch with `task auth use {key}`)");
        }
        AuthCmd::Links { server, json } => {
            let (identity, home) = home_identity_client(server).await?;
            let base = home.url.clone();
            let links = identity
                .list_links(home.token.clone())
                .await
                .map_err(|e| eyre::eyre!("list links on {base}: {e:?}"))?;

            if json {
                // Tokens are the whole point of the locker being
                // encrypted; don't print them just because a caller
                // asked for machine-readable output.
                let redacted: Vec<serde_json::Value> = links
                    .iter()
                    .map(|l| {
                        serde_json::json!({
                            "id": l.id, "label": l.label,
                            "remote_url": l.remote_url, "remote_slug": l.remote_slug,
                            "remote_user_id": l.remote_user_id,
                            "remote_email": l.remote_email,
                            "has_token": l.token.is_some(),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&redacted)?);
                return Ok(());
            }
            if links.is_empty() {
                println!("no linked orgs — add one with `task auth link --org <slug>`");
                return Ok(());
            }
            println!("{} linked org(s) on {base}:", links.len());
            for l in &links {
                println!(
                    "  {}  {:<20} {}  {}",
                    crate::shared::short_uuid(&l.id),
                    l.remote_slug,
                    l.remote_email.as_deref().unwrap_or("(no email)"),
                    l.remote_url,
                );
            }
        }
        AuthCmd::Unlink { id, server } => {
            let uuid = id
                .parse::<uuid::Uuid>()
                .map_err(|_| eyre::eyre!("`{id}` is not a link id — see `task auth links`"))?;
            let (identity, home) = home_identity_client(server).await?;
            let base = home.url.clone();
            identity
                .unlink_server(home.token.clone(), uuid)
                .await
                .map_err(|e| eyre::eyre!("unlink on {base}: {e:?}"))?;
            println!("unlinked {id}");
            println!(
                "  the local session entry (if any) is untouched — `task auth logout` to drop it"
            );
        }
        AuthCmd::Profile { server } => {
            let (identity, home) = home_identity_client(server).await?;
            let p = identity
                .get_profile(home.token.clone())
                .await
                .map_err(|e| eyre::eyre!("get profile: {e:?}"))?;
            println!("Profile (authoritative, on {}):", home.url);
            println!("  user:  {}", p.user_id);
            println!("  email: {}", p.email.as_deref().unwrap_or("(none)"));
            println!("  name:  {}", p.name.as_deref().unwrap_or("(none)"));
            println!("  image: {}", p.image.as_deref().unwrap_or("(none)"));
        }
        AuthCmd::SetProfile {
            name,
            image,
            server,
        } => {
            let (identity, home) = home_identity_client(server).await?;
            let report = identity
                .sync_profile(identity_proto::SyncProfileRequest {
                    session_token: home.token.clone(),
                    name,
                    image,
                })
                .await
                .map_err(|e| eyre::eyre!("sync profile: {e:?}"))?;
            let p = &report.profile;
            println!("Profile set on home ({}):", home.url);
            println!("  name:  {}", p.name.as_deref().unwrap_or("(none)"));
            println!("  image: {}", p.image.as_deref().unwrap_or("(none)"));
            if !report.updated.is_empty() {
                println!("  pushed to: {}", report.updated.join(", "));
            }
            // Anything short of "everywhere" gets said out loud — a
            // partial fan-out that reads as success is how caches rot.
            if !report.pending.is_empty() {
                println!(
                    "  pending (other servers — needs federated push): {}",
                    report.pending.join(", ")
                );
            }
            if !report.failed.is_empty() {
                println!("  FAILED:");
                for f in &report.failed {
                    println!("    {f}");
                }
                println!("  re-run `task auth set-profile` to retry those");
            }
        }
        AuthCmd::Login { email, password } => {
            let email = resolve_email(email)?;
            let password = resolve_password(password, false)?;
            // Remote-first sign-in over the org's AuthService.
            // The org is resolved via the server's well-known
            // document — no `task org init` needed on this box.
            let (slug, base) = resolve_auth_target(org_override).await?;
            let url = resolve_org_vox_url(Some(base.clone()), &slug);
            let client: AuthServiceClient = establish_for_url(&url).await?;
            let bundle = client
                .sign_in_email_password(SignInEmailPassword {
                    email: email.clone(),
                    password,
                    ip_address: None,
                    user_agent: Some("task-cli".into()),
                })
                .await
                .map_err(|e| eyre::eyre!("sign in: {e}"))?;
            let resolved_email = bundle.user.email.clone().unwrap_or_else(|| email.clone());
            // Multi-server session: insert/update the entry keyed
            // by (server, org) and make it active. The stored
            // server URL is what later invocations resolve when
            // neither `--server` nor `TASK_VOX_URL` is set.
            let mut sess = crate::session_store::load()?
                .unwrap_or_else(crate::session_store::CliSession::empty);
            let key = sess.record_login(
                &slug,
                &base,
                bundle.user.id,
                resolved_email.clone(),
                bundle.token.clone(),
            );
            crate::session_store::save(&sess)?;
            println!(
                "Signed in as {} ({}) on org `{slug}`",
                resolved_email, bundle.user.id,
            );
            println!("  server:   {base}");
            println!("  session:  {key}");
            if let Some(member_org) = bundle.session.active_organization_id {
                println!("Architect-auth active membership: {member_org}");
            }
        }
        AuthCmd::Whoami => match crate::session_store::load()? {
            Some(s) => {
                println!(
                    "home:   {}",
                    if s.home.is_empty() {
                        "(none)"
                    } else {
                        s.home.as_str()
                    }
                );
                println!("active: {}", s.active);
                for (key, entry) in &s.servers {
                    let marker = if *key == s.active { "*" } else { " " };
                    println!(
                        "{marker} {key:<28}  org={}  {}  {}  server={}",
                        entry.slug, entry.email, entry.user_id, entry.url
                    );
                }
                // Where the NEXT command will go, after the full
                // precedence fold (flag > env > session > default).
                println!("server: {} (this invocation)", resolve_server_base(None));
                println!(
                    "session: {}",
                    crate::session_store::session_path()?.display()
                );
            }
            None => {
                println!("Not signed in. Run `task auth login --email … --password …`.");
            }
        },
        AuthCmd::Use { session } => {
            let Some(mut sess) = crate::session_store::load()? else {
                return Err(crate::errors::usage("auth use")
                    .cause("no stored session")
                    .hint("run `task auth login` first")
                    .report());
            };
            let key = match_session_entry(&sess, &session)?;
            sess.active = key.clone();
            crate::session_store::save(&sess)?;
            let entry = &sess.servers[&key];
            println!(
                "Active session: {key} — org `{}` on {} ({})",
                entry.slug, entry.url, entry.email
            );
        }
        AuthCmd::Logout => {
            let Some(mut sess) = crate::session_store::load()? else {
                println!("Not signed in — nothing to do.");
                return Ok(());
            };
            // Which entry? `--org` picks by slug (preferring the
            // entry on the currently-resolved server); default is
            // the active entry. Other servers stay linked.
            let key = match org_override.map(str::to_owned).or_else(global_org) {
                Some(slug) => {
                    let base = resolve_server_base(None);
                    sess.servers
                        .iter()
                        .find(|(_, e)| {
                            e.slug == slug && crate::session_store::same_server(&e.url, &base)
                        })
                        .or_else(|| sess.servers.iter().find(|(_, e)| e.slug == slug))
                        .map(|(k, _)| k.clone())
                        .ok_or_else(|| {
                            crate::errors::not_found("logout", &slug)
                                .cause("no stored session for that org")
                                .hint("`task auth whoami` lists the signed-in sessions")
                                .report()
                        })?
                }
                None => sess.active.clone(),
            };
            if let Some(entry) = sess.servers.remove(&key) {
                // Server-side revoke, best effort — over the
                // entry's OWN server (remote logout). Legacy
                // `"local"` entries ride the same per-org vox
                // route: the localhost default resolves to the
                // embedded backend when no server is running,
                // which reaches the org's own auth store — no
                // direct auth.sqlite open.
                let revoked: eyre::Result<()> = {
                    let base =
                        (entry.url != crate::session_store::LOCAL_URL).then(|| entry.url.clone());
                    let url = resolve_org_vox_url(base, &entry.slug);
                    match Box::pin(establish_for_url::<AuthServiceClient>(&url)).await {
                        Ok(client) => client
                            .sign_out(entry.token.clone())
                            .await
                            .map_err(|e| eyre::eyre!("{e}")),
                        Err(e) => Err(e),
                    }
                };
                if let Err(e) = revoked {
                    eprintln!("warning: server-side sign out failed: {e:#}");
                }
                println!("Signed out of `{}` ({}).", entry.slug, entry.url);
            } else {
                println!("No stored session under `{key}`.");
            }
            // If no servers left, clear the file entirely; else
            // write the shrunken session back.
            if sess.servers.is_empty() {
                crate::session_store::clear()?;
            } else {
                // Active falls back to home if home is still
                // present, otherwise pick the first remaining
                // server.
                if !sess.servers.contains_key(&sess.active) {
                    sess.active = if sess.servers.contains_key(&sess.home) {
                        sess.home.clone()
                    } else {
                        sess.servers.keys().next().cloned().unwrap_or_default()
                    };
                }
                crate::session_store::save(&sess)?;
            }
        }
        AuthCmd::Org(AuthOrgCmd::List) => {
            // Local-only (documented on `local_org_ctx`): no
            // membership-listing RPC on AuthService yet.
            let ctx = local_org_ctx(org_override, "auth org list")?;
            let auth_db_path = ctx.root.auth_db();
            let Some(sess) = crate::session_store::load()? else {
                return Err(eyre::eyre!("not signed in — run `task auth login` first"));
            };
            let Some(active_entry) = sess.active_server() else {
                return Err(eyre::eyre!(
                    "no active server entry in session — run `task auth login --org {} …` first",
                    ctx.root.slug()
                ));
            };
            let auth = open_local_auth(&auth_db_path).await?;
            // Verify session still valid + refresh user_id.
            let bundle = auth
                .current_session(CurrentSession {
                    token: active_entry.token.clone(),
                })
                .await
                .map_err(|e| eyre::eyre!("session: {e}"))?;
            let memberships = list_user_memberships(bundle.user.id, &auth_db_path).await?;
            if memberships.is_empty() {
                println!("(no org memberships)");
            }
            for (member, org) in memberships {
                println!(
                    "  {}  {}  ({})",
                    member.organization_id, org.name, member.role
                );
            }
        }
        AuthCmd::Org(AuthOrgCmd::Use { org_id }) => {
            // Local-only (documented on `local_org_ctx`): no
            // set-active-organization RPC on AuthService yet.
            let ctx = local_org_ctx(org_override, "auth org use")?;
            let auth_db_path = ctx.root.auth_db();
            let Some(sess) = crate::session_store::load()? else {
                return Err(eyre::eyre!("not signed in — run `task auth login` first"));
            };
            let Some(active_entry) = sess.active_server() else {
                return Err(eyre::eyre!("no active server in session"));
            };
            // Resolve the reference against the user's memberships:
            // uuid / id prefix, slug, or name (exact / unique prefix)
            // — same matcher as every other entity flag. Doubles as
            // the membership check (non-members never match).
            let memberships = list_user_memberships(active_entry.user_id, &auth_db_path).await?;
            let cands: Vec<crate::json_out::Candidate> = memberships
                .iter()
                .map(|(m, o)| (m.organization_id, o.name.clone(), o.slug.clone()))
                .collect();
            let resolved = match crate::json_out::match_entity(&cands, &org_id, "organization") {
                Ok(i) => cands[i].0,
                Err(fail) => {
                    return Err(fail.into_report("organization", &org_id));
                }
            };
            update_session_active_org(&active_entry.token, Some(resolved), &auth_db_path).await?;
            println!("Architect-auth active membership set to {resolved}");
        }
        AuthCmd::Users => {
            // Over vox via `AuthService::list_org_members` — works
            // against a remote org now, and (via the embedded
            // fallback) against a local org with no server running.
            // The stored session token is attached when present; the
            // service's tokenless fallback enumerates the org
            // store's users, which is exactly what the old direct
            // auth.sqlite read produced.
            let slug = match crate::resolve_active_org(org_override.map(str::to_owned)) {
                Ok(s) => s,
                Err(_) => crate::org_ctx::resolve_active(None)?.root.slug().to_owned(),
            };
            let url = resolve_org_vox_url(None, &slug);
            let client: AuthServiceClient = establish_for_url(&url).await?;
            let token = crate::session_store::load()?
                .as_ref()
                .and_then(|s| s.active_server().map(|e| e.token.clone()))
                .unwrap_or_default();
            let users = client
                .list_org_members(token)
                .await
                .map_err(|e| eyre::eyre!("query auth_users: {e}"))?;
            if users.is_empty() {
                println!("(no users)");
            }
            println!("{:<38}  {:<24}  email", "user_id", "name");
            for u in users {
                println!("{:<38}  {:<24}  {}", u.user_id, u.name, u.email);
            }
        }
        AuthCmd::MigrateEmail {
            user,
            email,
            to,
            reason,
        } => {
            let (client, slug, token) = auth_client_with_session(org_override).await?;
            let user_id = resolve_target_user(&client, &token, user, email.as_deref()).await?;
            let moved = client
                .migrate_user_email(architect_auth::proto::service::MigrateUserEmailRequest {
                    session_token: token,
                    user_id,
                    new_email: to.clone(),
                    reason,
                })
                .await
                .map_err(|e| eyre::eyre!("migrate email: {e}"))?;
            println!(
                "{slug}: {user_id} is now {}",
                moved.email.as_deref().unwrap_or("(no email)")
            );
            // The id not changing IS the feature; say so, because the
            // whole risk of an "email migration" is that it quietly
            // created a new account and orphaned everything.
            println!("  user id unchanged — tasks, timers and authorship stay attached");
            println!("  email is now unverified (the new address hasn't been proven)");
        }
        AuthCmd::EmailHistory { user, email } => {
            let (client, slug, token) = auth_client_with_session(org_override).await?;
            let user_id = resolve_target_user(&client, &token, user, email.as_deref()).await?;
            let history = client
                .list_email_history(architect_auth::proto::service::EmailHistoryRequest {
                    session_token: token,
                    user_id,
                })
                .await
                .map_err(|e| eyre::eyre!("email history: {e}"))?;
            if history.is_empty() {
                println!("{slug}: {user_id} has never changed email");
                return Ok(());
            }
            println!("{:<26}  {:<30}  {:<30}  by", "when", "from", "to");
            for row in history {
                println!(
                    "{:<26}  {:<30}  {:<30}  {}",
                    row.created_at.to_rfc3339(),
                    row.previous_email.as_deref().unwrap_or("(none)"),
                    row.new_email,
                    row.changed_by
                        .map_or_else(|| "self".to_owned(), |id| id.to_string()),
                );
                if let Some(reason) = row.reason {
                    println!("{:<26}  ↳ {reason}", "");
                }
            }
        }
    }
    Ok(())
}

/// An `AuthServiceClient` for the active org plus the stored session
/// token. Both migration verbs need all three, and both fail early and
/// clearly when there is no session — these are operator actions, and the
/// server refuses them without one.
async fn auth_client_with_session(
    org_override: Option<&str>,
) -> eyre::Result<(architect_auth::proto::AuthServiceClient, String, String)> {
    use architect_auth::proto::AuthServiceClient;
    let slug = match crate::resolve_active_org(org_override.map(str::to_owned)) {
        Ok(s) => s,
        Err(_) => crate::org_ctx::resolve_active(None)?.root.slug().to_owned(),
    };
    let url = resolve_org_vox_url(None, &slug);
    let client: AuthServiceClient = establish_for_url(&url).await?;
    let token = crate::session_store::load()?
        .as_ref()
        .and_then(|s| s.active_server().map(|e| e.token.clone()))
        .filter(|t| !t.is_empty())
        .ok_or_else(|| {
            eyre::eyre!(
                "no session for this server — run `task auth login --org {slug}` first \
                 (changing an account's email is an operator action and the server \
                 refuses it without a session)"
            )
        })?;
    Ok((client, slug, token))
}

/// Resolve `--user <uuid>` or `--email <current>` to a user id in THIS
/// org. Emails are per-org, so the lookup has to go through this org's
/// member list rather than assuming an id carries across servers.
async fn resolve_target_user(
    client: &architect_auth::proto::AuthServiceClient,
    token: &str,
    user: Option<uuid::Uuid>,
    email: Option<&str>,
) -> eyre::Result<uuid::Uuid> {
    if let Some(id) = user {
        return Ok(id);
    }
    let Some(email) = email else {
        return Err(eyre::eyre!(
            "pass --user <uuid> or --email <current address>"
        ));
    };
    let members = client
        .list_org_members(token.to_owned())
        .await
        .map_err(|e| eyre::eyre!("look up `{email}`: {e}"))?;
    members
        .iter()
        .find(|m| m.email.eq_ignore_ascii_case(email))
        .map(|m| m.user_id)
        .ok_or_else(|| {
            eyre::eyre!(
                "no account with email `{email}` in this org — `task auth users` lists them"
            )
        })
}

/// Resolve a `task auth use` reference against the stored session
/// entries: exact key, exact slug (unique), then unique prefix of
/// either. Ambiguity and misses list what IS stored.
fn match_session_entry(
    sess: &crate::session_store::CliSession,
    reference: &str,
) -> eyre::Result<String> {
    if sess.servers.contains_key(reference) {
        return Ok(reference.to_owned());
    }
    let slug_hits: Vec<&String> = sess
        .servers
        .iter()
        .filter(|(_, e)| e.slug == reference)
        .map(|(k, _)| k)
        .collect();
    if let [one] = slug_hits.as_slice() {
        return Ok((*one).clone());
    }
    let prefix_hits: Vec<&String> = if slug_hits.is_empty() {
        sess.servers
            .iter()
            .filter(|(k, e)| k.starts_with(reference) || e.slug.starts_with(reference))
            .map(|(k, _)| k)
            .collect()
    } else {
        slug_hits
    };
    let stored = || {
        sess.servers
            .keys()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    match prefix_hits.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(crate::errors::not_found("auth use", reference)
            .cause(format!("stored sessions: {}", stored()))
            .hint("`task auth whoami` lists the stored entries")
            .report()),
        many => Err(crate::errors::conflict("auth use", reference)
            .cause(format!(
                "matches {} entries: {}",
                many.len(),
                many.iter()
                    .map(|k| k.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .hint("disambiguate with the full key (`slug@host`)")
            .report()),
    }
}

/// Resolve a LOCAL on-disk org for the two remaining auth verbs
/// that read the org's `auth.sqlite` directly: `auth org list` and
/// `auth org use`. These stay local because `AuthService` has no
/// membership-listing or set-active-organization RPC yet (the
/// per-user membership walk and the `auth_session
/// .active_organization_id` write have no wire surface — a known
/// gap, to be closed alongside the vox client-middleware auth
/// work). Everything else in `task auth` is remote-capable.
fn local_org_ctx(
    org_override: Option<&str>,
    what: &str,
) -> eyre::Result<crate::org_ctx::ActiveOrg> {
    crate::org_ctx::resolve_active(org_override).map_err(|e| {
        crate::errors::usage(format!(
            "{what} is a local-only command (it reads the org's on-disk auth.sqlite)"
        ))
        .cause(format!("{e:#}"))
        .hint(
            "run `task org init <slug>` to create a local org dir; a remote session \
             (`task auth login --server …`) cannot serve this command",
        )
        .report()
    })
}

async fn open_auth_db(auth_db_path: &std::path::Path) -> eyre::Result<sea_orm::DatabaseConnection> {
    use sea_orm::Database;
    use sea_orm_migration::MigratorTrait;
    let db = Database::connect(format!("sqlite://{}?mode=rwc", auth_db_path.display()))
        .await
        .map_err(|e| eyre::eyre!("connect auth db: {e}"))?;
    architect_auth::db::Migrator::up(&db, None)
        .await
        .map_err(|e| eyre::eyre!("auth migrations: {e}"))?;
    Ok(db)
}

async fn list_user_memberships(
    user_id: uuid::Uuid,
    auth_db_path: &std::path::Path,
) -> eyre::Result<
    Vec<(
        architect_auth::db::AuthMemberModel,
        architect_auth::db::AuthOrganizationModel,
    )>,
> {
    use architect_auth::db::{AuthMemberColumn, AuthMemberEntity, AuthOrganizationEntity};
    use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};
    let db = open_auth_db(auth_db_path).await?;
    let members = AuthMemberEntity::find()
        .filter(AuthMemberColumn::UserId.eq(user_id))
        .all(&db)
        .await
        .map_err(|e| eyre::eyre!("list members: {e}"))?;
    let mut out = Vec::with_capacity(members.len());
    for m in members {
        let Some(org) = AuthOrganizationEntity::find_by_id(m.organization_id)
            .one(&db)
            .await
            .map_err(|e| eyre::eyre!("find org {}: {e}", m.organization_id))?
        else {
            continue;
        };
        out.push((m, org));
    }
    Ok(out)
}

async fn update_session_active_org(
    token: &str,
    org_id: Option<uuid::Uuid>,
    auth_db_path: &std::path::Path,
) -> eyre::Result<()> {
    use architect_auth::db::{AuthSessionActiveModel, AuthSessionColumn, AuthSessionEntity};
    use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};
    let token_hash = hash_session_token(crate::session_store::DEFAULT_AUTH_SECRET, token);
    let db = open_auth_db(auth_db_path).await?;
    let row = AuthSessionEntity::find()
        .filter(AuthSessionColumn::TokenHash.eq(token_hash))
        .one(&db)
        .await
        .map_err(|e| eyre::eyre!("find session: {e}"))?
        .ok_or_else(|| eyre::eyre!("session not found — session file may be stale"))?;
    let mut am: AuthSessionActiveModel = row.into_active_model();
    am.active_organization_id = Set(org_id);
    am.update(&db)
        .await
        .map_err(|e| eyre::eyre!("update session: {e}"))?;
    Ok(())
}

/// Reproduce `architect-auth::crypto::hash_token`. The auth
/// crate keeps the helper crate-private; we re-implement the
/// exact same recipe so the CLI can look up its own session
/// row by token hash without depending on auth internals.
///
/// **Recipe (must match `architect-auth/crypto.rs`):**
/// `base64url-no-pad(SHA256(secret || ":" || token))`.
fn hash_session_token(secret: &str, token: &str) -> String {
    use base64::Engine;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    h.update(b":");
    h.update(token.as_bytes());
    URL_SAFE_NO_PAD.encode(h.finalize())
}
