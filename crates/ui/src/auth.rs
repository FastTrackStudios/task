//! Web auth — the active account context + the bottom-left switcher.
//!
//! The server mounts architect-auth's `AuthService` per org
//! (`AuthServerMiddleware` wraps the dispatcher), so the UI signs in
//! over the same per-org vox socket every other service rides
//! ([`crate::vox_clients::establish_for`] against the home org).
//!
//! Flow: [`provide_auth`] runs at the app root and provides
//! `Signal<Option<ActiveAccount>>` + [`AuthCtx`]. On boot it restores
//! the persisted account (localStorage `task.auth.active`), defaulting
//! to the Guest account — Guest is the account we use for anonymous
//! sessions. [`AuthCtx::switch_account`] first tries the cached token
//! (`task.auth.token.<email>` → `whoami` validates), and only on a
//! miss performs a real `sign_in_email_password`. Switching never
//! signs the previous account out — its token stays cached so
//! switching back is instant; [`AuthCtx::sign_out`] is the only
//! explicit revocation.
//!
//! The [`AccountSwitcher`] (sidebar footer) replaces the old
//! free-text presence name input: identity now comes from the
//! account, and the presence status picker folded into the same
//! popover as a "Status" section.

use architect_ui::prelude::*;
use dioxus::prelude::*;
use uuid::Uuid;

use auth_proto::{AuthServiceClient, AuthUser, SignInEmailPassword, SignUpEmailPassword};
use identity_proto::{IdentityServiceClient, LinkServerRequest};

use crate::orgs::{OrgMeta, home_slug};
use crate::presence::{ManualStatus, PresenceLocal, PresenceStatus};
use crate::vox_clients::establish_for;

// ── dev accounts (DEBUG BUILDS ONLY) ────────────────────────────────
//
// The switcher performs the REAL sign-in flow — real session tokens
// issued by the org's auth engine — it just has these credentials
// pre-filled so switching is one click.
//
// The whole section is `#[cfg(debug_assertions)]`, so the passwords are
// not merely hidden in a release build: they are not compiled into the
// binary at all. Release takes the path this module always planned for
// — [`LoginForm`], which is already the sign-in surface — and keeps
// everything else (token cache, whoami validation, context,
// presence/claims integration) unchanged.

/// One pre-seeded dev account in the home org's auth DB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevAccount {
    pub email: &'static str,
    pub password: &'static str,
    pub name: &'static str,
    pub username: &'static str,
}

/// The dev roster — **empty in release**, so no password string is
/// compiled into a shipped binary. Every consumer iterates or takes
/// `.len()`, so both builds work without a `cfg` at each use site: the
/// pickers simply render nothing and the dropdown index offsets are 0.
#[cfg(not(debug_assertions))]
pub const DEV_ACCOUNTS: [DevAccount; 0] = [];

/// The four dev accounts seeded into the home org's `auth.sqlite` —
/// `task-server admin seed` plants exactly this roster (keep the two
/// lists in lockstep), so a debug web build against a seeded server
/// boots straight into Guest with no login.
#[cfg(debug_assertions)]
pub const DEV_ACCOUNTS: [DevAccount; 4] = [
    DevAccount {
        email: "cody@fasttrackstudios.com",
        password: "dev-cody-2026",
        name: "Cody Wright",
        username: "cody",
    },
    DevAccount {
        email: "carter@fasttrackstudios.com",
        password: "dev-carter-2026",
        name: "Carter Whitlock",
        username: "carter",
    },
    DevAccount {
        email: "tom@fasttrackstudios.com",
        password: "dev-tom-2026",
        name: "Tom Brooks",
        username: "tom",
    },
    DevAccount {
        email: "guest@fasttrackstudios.com",
        password: "dev-guest-2026",
        name: "Guest",
        username: "guest",
    },
];

/// The account a fresh browser lands on — anonymous-ish shared
/// identity ("Guest is the account we use for stuff like that").
pub const GUEST_EMAIL: &str = "guest@fasttrackstudios.com";

// ── the demo cast (DEBUG BUILDS ONLY) ───────────────────────────────
//
// `just demo` serves the example studio, whose people are Alice, Sam
// and Casey — not the dev-seed roster above. Rather than compile a
// second roster in, the demo script hands the cast to the app through
// `TASK_DEMO_CAST` (`email:password:Name:username`, comma-separated):
// read at runtime on native, baked at build for wasm (the same split as
// `TASK_VOX_URL` / `TASK_VOX_URL_WEB`). When it is set, it *replaces*
// the dev roster — boot lands on its first member, the switcher lists
// it, and signing out stays signed out so the login form's cast picker
// gets its turn.
//
// Debug builds only, same argument as `DEV_ACCOUNTS`: release keeps the
// property that no build shipped to anyone can be talked into a
// password sign-in nobody typed.

/// The roster every picker, switcher and auto-sign-in draws from: the
/// demo cast when `TASK_DEMO_CAST` is set (debug builds), the compiled
/// dev roster otherwise (empty in release).
pub fn dev_accounts() -> &'static [DevAccount] {
    #[cfg(debug_assertions)]
    {
        static CAST: std::sync::OnceLock<Option<Vec<DevAccount>>> = std::sync::OnceLock::new();
        if let Some(cast) = CAST.get_or_init(|| {
            #[cfg(target_arch = "wasm32")]
            let raw: Option<String> = option_env!("TASK_DEMO_CAST").map(str::to_owned);
            #[cfg(not(target_arch = "wasm32"))]
            let raw: Option<String> = std::env::var("TASK_DEMO_CAST").ok();
            let cast = parse_cast(&raw?);
            (!cast.is_empty()).then_some(cast)
        }) {
            return cast;
        }
    }
    &DEV_ACCOUNTS
}

/// `email:password:Name:username`, comma-separated. Malformed entries
/// are skipped rather than failing the roster — a demo that half-works
/// beats a login screen with no explanation.
#[cfg(debug_assertions)]
fn parse_cast(raw: &str) -> Vec<DevAccount> {
    fn hold(s: &str) -> &'static str {
        Box::leak(s.trim().to_owned().into_boxed_str())
    }
    raw.split(',')
        .filter_map(|entry| {
            let mut parts = entry.splitn(4, ':');
            let (email, password) = (parts.next()?.trim(), parts.next()?.trim());
            if email.is_empty() || password.is_empty() {
                return None;
            }
            let name = parts.next().map(str::trim).filter(|s| !s.is_empty());
            let username = parts.next().map(str::trim).filter(|s| !s.is_empty());
            Some(DevAccount {
                email: hold(email),
                password: hold(password),
                name: hold(name.unwrap_or(email)),
                username: hold(username.unwrap_or_default()),
            })
        })
        .collect()
}

/// Whether the roster came from `TASK_DEMO_CAST` rather than the
/// compiled dev list. The two differ in what signing out means: the dev
/// roster re-lands on Guest, the demo cast lands on the login form so
/// the cast picker gets used.
fn demo_cast_active() -> bool {
    !dev_accounts().is_empty() && !std::ptr::eq(dev_accounts().as_ptr(), DEV_ACCOUNTS.as_ptr())
}

/// Where boot lands with nothing stored: the demo cast's first member
/// (Alice, for the example studio), else Guest.
fn auto_land_email() -> &'static str {
    if demo_cast_active() {
        dev_accounts()[0].email
    } else {
        GUEST_EMAIL
    }
}

// ── active account context ──────────────────────────────────────────

/// The signed-in identity, derived from the server's session bundle.
#[derive(Clone, Debug, PartialEq)]
pub struct ActiveAccount {
    /// `AuthUser::id` — the auth system's user uuid (claims key).
    pub user_id: Uuid,
    pub email: String,
    pub name: String,
    /// The raw session token (also cached in localStorage under
    /// `task.auth.token.<email>` for instant switch-back).
    pub token: String,
}

/// Messages for the root auth service coroutine — the ONLY way auth
/// state changes. Every surface (desktop dropdown, mobile sheet, boot
/// restore) sends these; the single sequential consumer makes
/// concurrent-switch races and unmount-cancelled tasks structurally
/// impossible.
pub enum AuthAction {
    Switch(String),
    /// Real credential sign-in from the login form.
    SignIn {
        email: String,
        password: String,
    },
    /// Self-serve account creation (architect-auth `sign_up_email_password`).
    SignUp {
        email: String,
        password: String,
        name: String,
    },
    /// A token the central issuer already vouched for, from the redirect
    /// flow. The only action carrying no credentials: the password was
    /// typed at the issuer and Task never saw it, which is the point of
    /// going the long way round.
    ///
    /// Carries the issuer rather than looking it up, because this
    /// arrives on a fresh page load where discovery may not have
    /// resolved yet — see `central_login::ISSUER_KEY`.
    AdoptCentralToken {
        token: String,
        issuer: String,
    },
    SignOut,
}

/// Copyable auth handle — provided at the app root next to the plain
/// `Signal<Option<ActiveAccount>>` context (consumers that only read
/// identity take the signal; the switcher takes this).
#[derive(Clone, Copy)]
pub struct AuthCtx {
    pub active: Signal<Option<ActiveAccount>>,
    /// Last auth error, surfaced as a small text line under the
    /// switcher (never panics, never blocks the app).
    pub error: Signal<Option<String>>,
    /// True while a switch/sign-in is in flight.
    pub busy: Signal<bool>,
    /// Has boot restore finished? Distinguishes "no account" from "we
    /// haven't looked yet" — without it [`SignInGate`] flashes the login
    /// screen on every load before the cached token validates.
    pub booted: Signal<bool>,
    /// The root auth service (see [`provide_auth`]).
    actions: Coroutine<AuthAction>,
}

impl AuthCtx {
    /// Request a switch to `email`. Sync fire-and-forget — safe from
    /// UI that closes/unmounts on selection (sheets, dropdowns): the
    /// work runs in the root coroutine, not the caller's scope.
    pub fn switch_account(self, email: impl Into<String>) {
        self.actions.send(AuthAction::Switch(email.into()));
    }

    /// Sign in with real credentials (the login form). Fire-and-forget.
    pub fn sign_in(self, email: impl Into<String>, password: impl Into<String>) {
        self.actions.send(AuthAction::SignIn {
            email: email.into(),
            password: password.into(),
        });
    }

    /// Create a new account, then sign into it (the sign-up form).
    pub fn sign_up(
        self,
        email: impl Into<String>,
        password: impl Into<String>,
        name: impl Into<String>,
    ) {
        self.actions.send(AuthAction::SignUp {
            email: email.into(),
            password: password.into(),
            name: name.into(),
        });
    }

    /// Adopt a token from the central redirect flow.
    ///
    /// Called by the `/auth/callback` page once it has redeemed an
    /// authorization code. Fire-and-forget like the rest: the work runs
    /// in the root coroutine, so the callback page is free to navigate
    /// away immediately.
    pub fn adopt_central_token(self, token: impl Into<String>, issuer: impl Into<String>) {
        self.actions.send(AuthAction::AdoptCentralToken {
            token: token.into(),
            issuer: issuer.into(),
        });
    }

    /// Request sign-out (revoke server-side, drop cached token, fall
    /// back to Guest). Same fire-and-forget contract as
    /// [`Self::switch_account`].
    pub fn sign_out(self) {
        self.actions.send(AuthAction::SignOut);
    }
}

/// The server whose identity locker we last pulled — the anchor we push
/// new links UP to. A server is a locker host iff its `list_links`
/// answered (see [`pull_locker`]); non-locker servers leave this at the
/// previously-anchored home so [`push_link`] still targets it.
#[derive(Clone, PartialEq)]
struct HomeLocker {
    /// The home server's vox base URL (matches [`crate::vox_session::ActiveServer::url`]).
    url: String,
    /// The session token that authenticates us against the locker.
    token: String,
}

/// The signals the auth service mutates — plumbed as one bundle so
/// the coroutine closure and the async workers stay readable.
#[derive(Clone, Copy)]
struct AuthState {
    active: Signal<Option<ActiveAccount>>,
    error: Signal<Option<String>>,
    busy: Signal<bool>,
    orgs: Signal<Vec<OrgMeta>>,
    /// The multi-server registry — the resolved session is mirrored into
    /// the active [`crate::server_registry::ServerEntry`] so each server
    /// remembers who you signed in as (see [`sync_active_server_entry`]).
    registry: crate::server_registry::ServerRegistry,
    /// The server whose identity locker anchors multi-server sync — set
    /// by [`pull_locker`] when a signed-into server turns out to host a
    /// locker; read by [`push_link`] to decide where to push new links.
    home_locker: Signal<Option<HomeLocker>>,
    /// Mirror of the active session token, provided at the app root
    /// BEFORE org discovery. Writing it re-runs discovery so the org list
    /// gets re-tagged with membership (#109 criterion 6) — the boot fetch
    /// is necessarily anonymous, since sign-in needs the home slug that
    /// discovery resolves.
    session_token: Signal<Option<String>>,
}

/// Publish the resolved session token as the vox dial identity, and tear
/// down connections established under the previous one.
///
/// This is the piece that was missing: the token was stored (localStorage
/// and the registry entry) and passed as an ARGUMENT to the handful of
/// auth methods that take one, but nothing ever attached it to the vox
/// transport — so every other RPC arrived at the server as
/// `principal=anonymous`, and the permission gate computed the right
/// answer (`would_deny`) on all of them and threw it away.
///
/// A connection presents its identity ONCE, at the WebSocket upgrade, so
/// publishing the token is only half the job: sockets already open from
/// before sign-in stay anonymous forever. `set_session_token` reports
/// whether the value actually changed, and on a change every cached
/// connection is dropped so the next call re-dials as the new identity.
/// The app root's supervised `Connection` re-establishes on its own (its
/// connect closure re-runs, and `caller_for` finds no cached root).
///
/// Guest is NOT excluded here, unlike [`sync_active_server_entry`]: Guest
/// is a real seeded account holding a real session token, and a debug
/// build boots straight into it. Presenting that token is what it is — a
/// validated session — and withholding it would make every dev boot
/// anonymous, i.e. denied the moment enforcement goes on.
fn publish_session_token(mut st: AuthState, account: Option<&ActiveAccount>) {
    let token = account.map(|a| a.token.clone());
    if crate::vox_session::set_session_token(token.clone()) {
        crate::vox_clients::drop_cached_connections();
    }
    // Reactive mirror: re-runs org discovery so the well-known doc is
    // re-fetched WITH the token and the org list gets its membership
    // tags. Guarded so an unchanged token can't loop the resource.
    if *st.session_token.peek() != token {
        st.session_token.set(token);
    }
}

/// Mirror a freshly-resolved session into the active server's registry
/// entry: on real sign-in the entry gains `session_token` / `my_user_id`
/// / `my_email`; a Guest/anonymous session (or sign-out) clears them.
///
/// This is what makes multi-server auth stick: the app root's effect
/// forwards the active entry's `session_token` into the
/// [`crate::vox_session::ActiveServer`] holder, so switching servers
/// re-points the connection at the right identity. No-op when no server
/// is selected (the env/same-origin default) or the entry is unchanged.
fn sync_active_server_entry(
    mut registry: crate::server_registry::ServerRegistry,
    account: Option<&ActiveAccount>,
) {
    let Some(id) = registry.active_id() else {
        return;
    };
    let Some(mut entry) = registry.get(id) else {
        return;
    };
    let (token, uid, mail) = match account {
        Some(a) if a.email != GUEST_EMAIL => (
            Some(a.token.clone()),
            Some(a.user_id),
            Some(a.email.clone()),
        ),
        _ => (None, None, None),
    };
    if entry.session_token == token && entry.my_user_id == uid && entry.my_email == mail {
        return;
    }
    entry.session_token = token;
    entry.my_user_id = uid;
    entry.my_email = mail;
    registry.upsert(entry);
}

/// Pull the identity locker from the server we just signed into.
///
/// If that server hosts a locker (its `list_links` answers), it becomes
/// the [`HomeLocker`] anchor and every linked server it knows about is
/// upserted into the multi-server [`crate::server_registry::ServerRegistry`]
/// — token, user id, email, label all mirrored so switching to any of
/// them is instant. A server WITHOUT a locker (`list_links` errors) is a
/// plain remote: no-op, the previous anchor stands. Never touches the
/// active selection.
async fn pull_locker(mut st: AuthState, account: &ActiveAccount) {
    let Some(server) = crate::vox_session::active_server() else {
        return;
    };
    let client = match crate::vox_clients::establish_server::<IdentityServiceClient>(Some(
        &server.url,
    ))
    .await
    {
        Ok(c) => c,
        Err(_) => return,
    };
    let links = match client.list_links(account.token.clone()).await {
        // Err = this server has no locker / isn't a home server. No-op —
        // NOT fatal: it's just a plain remote we've signed into.
        Ok(l) => l,
        Err(_) => return,
    };
    // This server IS a locker host → it's our identity anchor.
    st.home_locker.set(Some(HomeLocker {
        url: server.url.clone(),
        token: account.token.clone(),
    }));
    for link in links {
        let mut entry = st
            .registry
            .list()
            .into_iter()
            .find(|e| e.server_url == link.remote_url)
            .unwrap_or_else(|| {
                crate::server_registry::ServerEntry::new(
                    link.label.clone(),
                    link.remote_url.clone(),
                )
            });
        entry.session_token = link.token;
        entry.my_user_id = link.remote_user_id;
        entry.my_email = link.remote_email;
        entry.label = link.label;
        st.registry.upsert(entry);
    }
}

/// Push the server we just signed into UP to the home locker as a linked
/// server, so the locker (and every other client of it) learns about it.
///
/// Best-effort: no anchor, no active server, or a link RPC error is a
/// silent no-op. Skips when the active server IS the home locker (you
/// don't link a server to itself) — which is exactly the case right
/// after [`pull_locker`] anchored THIS server.
async fn push_link(st: AuthState, account: &ActiveAccount) {
    let Some(home) = st.home_locker.peek().clone() else {
        return;
    };
    let Some(server) = crate::vox_session::active_server() else {
        return;
    };
    if server.url == home.url {
        return;
    }
    let label = st
        .registry
        .active_entry()
        .map(|e| e.label)
        .filter(|l| !l.trim().is_empty())
        .unwrap_or_else(|| {
            server
                .url
                .split("://")
                .nth(1)
                .unwrap_or(&server.url)
                .split('/')
                .next()
                .unwrap_or(&server.url)
                .to_owned()
        });
    let remote_slug = home_slug(&st.orgs.peek());
    let client = match crate::vox_clients::establish_server::<IdentityServiceClient>(Some(
        &home.url,
    ))
    .await
    {
        Ok(c) => c,
        Err(_) => return,
    };
    // Best-effort — tolerate errors (the locker is a convenience mirror).
    let _ = client
        .link_server(LinkServerRequest {
            session_token: home.token,
            label,
            remote_url: server.url,
            remote_slug,
            remote_user_id: Some(account.user_id),
            remote_email: Some(account.email.clone()),
            token: Some(account.token.clone()),
            expires_at: None,
        })
        .await;
}

/// Switch to `email`: cached token → `whoami` validates; on miss or
/// invalid → real `sign_in_email_password` → cache the fresh token.
/// Sets the context + persists `task.auth.active`. The previous
/// account is NOT signed out — its token stays cached so switching
/// back is instant.
async fn run_switch(mut st: AuthState, email: &str) {
    let slug = home_slug(&st.orgs.peek());
    if slug.is_empty() {
        st.error
            .set(Some("org discovery hasn't resolved yet".to_owned()));
        // MUST clear `busy`. Boot restore raises it synchronously before
        // queueing this action (so `SignInGate` doesn't flash the login
        // form over a session that's about to restore), which means this
        // early return owns it too — leaving it set strands the app on
        // "Restoring your session…" forever, with no way to sign in and
        // no way to reach the server picker.
        st.busy.set(false);
        return;
    }
    st.busy.set(true);
    st.error.set(None);
    match resolve_session(&slug, email).await {
        Ok(account) => {
            save_active_email(&account.email);
            sync_active_server_entry(st.registry, Some(&account));
            // Before any further RPC: the locker/link calls below should
            // already ride the new identity.
            publish_session_token(st, Some(&account));
            // Multi-server sync: pull this server's locker (if it hosts
            // one, it becomes the anchor), then push it up to whatever
            // anchor stands. Both borrow `&account` before the move.
            pull_locker(st, &account).await;
            push_link(st, &account).await;
            st.active.set(Some(account));
        }
        Err(e) => st.error.set(Some(e)),
    }
    st.busy.set(false);
}

/// Real credential sign-in (login form). When `name` is `Some`, first
/// creates the account via `sign_up_email_password`, then the returned
/// bundle IS the session (sign-up logs you in). Caches the token under
/// the email so later `switch_account`/boot restores it via `whoami`.
async fn run_credential_sign_in(
    mut st: AuthState,
    email: &str,
    password: &str,
    name: Option<&str>,
) {
    let slug = home_slug(&st.orgs.peek());
    if slug.is_empty() {
        st.error
            .set(Some("org discovery hasn't resolved yet".to_owned()));
        st.busy.set(false);
        return;
    }
    st.busy.set(true);
    st.error.set(None);
    let result = async {
        let client = auth_client(&slug).await?;
        let bundle = if let Some(name) = name {
            client
                .sign_up_email_password(SignUpEmailPassword {
                    email: email.to_owned(),
                    password: password.to_owned(),
                    name: Some(name.to_owned()),
                    username: None,
                    image: None,
                    metadata_json: None,
                    ip_address: None,
                    user_agent: Some("task-web".to_owned()),
                })
                .await
                .map_err(|e| format!("create account: {e}"))?
        } else {
            client
                .sign_in_email_password(SignInEmailPassword {
                    email: email.to_owned(),
                    password: password.to_owned(),
                    ip_address: None,
                    user_agent: Some("task-web".to_owned()),
                })
                .await
                .map_err(|e| format!("sign in: {e}"))?
        };
        save_cached_token(email, &bundle.token);
        Ok::<ActiveAccount, String>(account_from(bundle.user, email, bundle.token))
    }
    .await;
    match result {
        Ok(account) => {
            save_active_email(&account.email);
            sync_active_server_entry(st.registry, Some(&account));
            publish_session_token(st, Some(&account));
            // Same multi-server sync as run_switch (borrow before move).
            pull_locker(st, &account).await;
            push_link(st, &account).await;
            st.active.set(Some(account));
        }
        Err(e) => st.error.set(Some(e)),
    }
    st.busy.set(false);
}

/// Sign in with a token the central issuer already vouched for.
///
/// The redirect flow hands back a token and nothing else, so unlike the
/// credential path there is no bundle to read a user out of — we ask the
/// issuer who it belongs to.
///
/// The token is an OAuth access token, not a session token. Task's
/// server accepts both (see `central_auth` there, which tries
/// `/auth/session` then `/oauth2/userinfo`), so everything downstream —
/// vox dialing, the locker, cached-token switch-back — is identical to a
/// credential sign-in and none of it needs to know which door was used.
async fn run_central_token_sign_in(mut st: AuthState, token: String, issuer: String) {
    st.busy.set(true);
    st.error.set(None);

    let result = async {
        let user = crate::central_login::user_info(&issuer, &token)
            .await
            .map_err(|e| e.to_string())?;
        let account = central_account(user, token)?;
        save_cached_token(&account.email, &account.token);
        Ok::<ActiveAccount, String>(account)
    }
    .await;

    match result {
        Ok(account) => {
            save_active_email(&account.email);
            sync_active_server_entry(st.registry, Some(&account));
            publish_session_token(st, Some(&account));
            pull_locker(st, &account).await;
            push_link(st, &account).await;
            st.active.set(Some(account));
        }
        Err(e) => st.error.set(Some(e)),
    }
    st.busy.set(false);
}

/// Explicit sign-out: revoke the session server-side, drop the cached
/// token + active marker, then fall back to Guest (the anonymous
/// default — auto sign-in).
/// The `AuthService` this server wants sign-in to go through.
///
/// The central issuer when discovery advertised one, the home org
/// otherwise. Both mount the *same* service, so this is only a choice
/// of endpoint — the sign-in, sign-up and sign-out calls below are
/// identical either way, and nothing downstream of the returned bundle
/// knows or needs to know which answered.
///
/// Anonymous on purpose: a sign-in has no session yet, and presenting a
/// stale one to the issuer would dial a connection keyed to the wrong
/// identity (see `shared_caller_with` on why identity is part of the
/// cache key).
///
/// Falls back to the home org if the issuer cannot be reached. That is
/// deliberate and it is the *client's* call to make, not a security
/// decision: a server that trusts an issuer still validates whatever
/// token arrives, so the worst case here is a sign-in that fails twice
/// instead of once. Silently failing when a self-hosted org could have
/// answered would be worse.
async fn auth_client(slug: &str) -> Result<AuthServiceClient, String> {
    if let Some(url) = task_ui_core::central_auth::issuer_vox() {
        match task_ui_core::vox_clients::establish_shared_at::<AuthServiceClient>(&url, None).await
        {
            Ok(client) => return Ok(client),
            Err(e) => {
                tracing::warn!(%e, "central issuer unreachable — trying this server's own accounts");
            }
        }
    }
    establish_for::<AuthServiceClient>(slug).await
}

async fn run_sign_out(mut st: AuthState) {
    let Some(account) = st.active.peek().clone() else {
        return;
    };
    clear_cached_token(&account.email);
    clear_active_email();
    sync_active_server_entry(st.registry, None);
    // Drop the authenticated sockets NOW, before the revoke round trip —
    // `sign_out` passes the token as an argument, so it doesn't need the
    // connection to carry the identity, and re-dialing anonymously for it
    // is the correct end state anyway.
    publish_session_token(st, None);
    st.active.set(None);
    let slug = home_slug(&st.orgs.peek());
    if !slug.is_empty() {
        if let Ok(client) = establish_for::<AuthServiceClient>(&slug).await {
            // Best-effort revocation — sign_out is idempotent.
            let _ = client.sign_out(account.token.clone()).await;
        }
    }
    // Debug lands back on Guest; release has no password to do that
    // with, so signing out leaves no active account and `LoginForm`
    // takes over — which is what signing out should mean. The demo cast
    // takes the release path on purpose: its login form offers the cast
    // one click each, and "log out, come back as Casey" is the demo.
    if cfg!(debug_assertions) && !demo_cast_active() {
        run_switch(st, GUEST_EMAIL).await;
    }
}

/// Provide the auth contexts and kick off boot restore. Call once at
/// the app root, after the org-list provider (`fetch_orgs` discovery)
/// and before the router.
pub fn provide_auth() -> AuthCtx {
    use futures_util::StreamExt as _;

    let orgs = use_context::<Signal<Vec<OrgMeta>>>();
    let registry = use_context::<crate::server_registry::ServerRegistry>();
    let active = use_signal(|| None::<ActiveAccount>);
    let error = use_signal(|| None::<String>);
    let busy = use_signal(|| false);
    let home_locker = use_signal(|| None::<HomeLocker>);
    // Provided by the app root ahead of discovery (see `app.rs`).
    let session_token = use_context::<Signal<Option<String>>>();
    // Set once boot restore has run (or decided there is nothing to
    // restore) — see `AuthCtx::booted`.
    let mut booted = use_signal(|| false);
    let st = AuthState {
        active,
        error,
        busy,
        orgs,
        registry,
        home_locker,
        session_token,
    };

    // The auth service: one sequential consumer for every auth action
    // in the app. Root-owned (unmount-safe — a sheet closing can't
    // cancel it) and ordered (no concurrent switches to race). A
    // queued burst coalesces to the newest action, so clicking an
    // account while the boot auto-sign-in is still resolving skips
    // straight to the click — no Guest flash, no generation counter.
    let actions = use_coroutine(move |mut rx: UnboundedReceiver<AuthAction>| async move {
        while let Some(mut msg) = rx.next().await {
            while let Ok(newer) = rx.try_recv() {
                msg = newer;
            }
            match msg {
                AuthAction::Switch(email) => run_switch(st, &email).await,
                AuthAction::SignIn { email, password } => {
                    run_credential_sign_in(st, &email, &password, None).await;
                }
                AuthAction::SignUp {
                    email,
                    password,
                    name,
                } => {
                    run_credential_sign_in(st, &email, &password, Some(&name)).await;
                }
                AuthAction::AdoptCentralToken { token, issuer } => {
                    run_central_token_sign_in(st, token, issuer).await;
                }
                AuthAction::SignOut => run_sign_out(st).await,
            }
        }
    });

    let ctx = AuthCtx {
        active,
        error,
        busy,
        booted,
        actions,
    };
    use_context_provider(|| active);
    use_context_provider(|| ctx);
    // The cross-crate identity mirror: feature UIs (the review
    // composer, presence chips) read who is signed in through
    // `task_ui_core::identity` without depending on this crate's auth
    // machinery. Kept in step with `active` by the effect below.
    let mut identity = use_signal(|| Option::<task_ui_core::identity::IdentityInfo>::None);
    use_context_provider(|| task_ui_core::identity::CurrentIdentity(identity));
    use_effect(move || {
        identity.set(
            active
                .read()
                .as_ref()
                .map(|a| task_ui_core::identity::IdentityInfo {
                    user_id: a.user_id,
                    email: a.email.clone(),
                    name: a.name.clone(),
                }),
        );
    });

    // Boot restore: wait for org discovery (home slug resolves), then
    // validate the persisted account — or auto sign-in as Guest when
    // nothing is stored. Runs exactly once.
    use_effect(move || {
        let slug = home_slug(&orgs.read());
        if slug.is_empty() || *booted.peek() {
            return;
        }
        booted.set(true);
        // A stored account is restored from its cached token on any
        // build. With nothing stored, only a debug build can auto-land
        // on Guest — that needs a compiled-in password. Release shows
        // `LoginForm` instead of failing a sign-in nobody asked for.
        //
        // `busy` is raised HERE, synchronously, not left to the
        // coroutine. `switch_account` only queues the action, so between
        // this effect and the coroutine picking it up there is a tick
        // where `booted` is true, `active` is None and `busy` is false —
        // which `SignInGate` would read as "signed out" and flash the
        // login screen over a session that is about to restore.
        let mut busy = busy;
        match load_active_email() {
            Some(email) => {
                busy.set(true);
                ctx.switch_account(email);
            }
            None if cfg!(debug_assertions) => {
                busy.set(true);
                ctx.switch_account(auto_land_email().to_owned());
            }
            // Nothing stored and no compiled-in password: genuinely
            // signed out, and the gate should say so immediately.
            None => {}
        }
    });
    ctx
}

/// Gate the app on a signed-in session (issue #109 criterion 5).
///
/// **This is presentation, not security.** The server still answers
/// anyone who opens a websocket to it directly; what actually refuses
/// data is the permission gate on the org lane
/// (`TASK_ENFORCE_PERMISSIONS`). This exists so an unauthenticated
/// visitor lands on sign-in instead of on a shell that renders empty
/// panels and a wall of failed requests.
///
/// Three states, and the middle one is the whole reason this isn't a
/// one-line `if`:
/// - a session → the app;
/// - restoring (boot hasn't resolved, or a sign-in is in flight) → a
///   neutral placeholder, NOT the login form, so a returning user with a
///   valid cached token never sees a sign-in screen flash;
/// - resolved with no session → sign in.
/// The gate screens' slice of window chrome: a floating drag strip
/// with the window controls, pinned across the top. Renders nothing
/// unless a frameless desktop shell provided
/// [`task_ui_core::window_chrome::WindowChrome`] — a browser tab needs
/// no help being moved or closed.
#[component]
fn GateChrome() -> Element {
    if task_ui_core::window_chrome::window_chrome().is_none() {
        return rsx! {};
    }
    rsx! {
        div { class: "fixed inset-x-0 top-0 z-40 flex h-10 items-center px-2",
            crate::chrome::DragRegion {}
            crate::chrome::WindowControls {}
        }
    }
}

#[component]
pub fn SignInGate(children: Element) -> Element {
    let ctx = use_context::<AuthCtx>();
    let active = ctx.active;
    let booted = ctx.booted;
    let busy = ctx.busy;

    if active.read().is_some() {
        return rsx! { {children} };
    }
    // The one route that MUST render while signed out, because signing in
    // is what it is for. The issuer redirects back here with an
    // authorization code, and this gate sits above the router — so
    // without this the page never mounts, the code is never redeemed,
    // and the round trip ends silently back on the login form with the
    // code sitting unused in the address bar. Which is exactly what it
    // did.
    if at_central_callback() {
        return rsx! { {children} };
    }
    if !booted() || busy() {
        // NEVER a dead end. This branch also covers "org discovery hasn't
        // resolved" — a failed or slow well-known fetch, or a server that
        // isn't answering — and a bug that stranded `busy` once made it
        // permanent: the app sat on this message with no way to sign in
        // and no way to reach the server picker, because both live behind
        // it. So the waiting state carries its own escape hatch, and
        // surfaces WHY discovery hasn't resolved when it knows.
        let discovery = use_context::<crate::orgs::DiscoveryError>();
        let err = discovery.0.read().clone();
        return rsx! {
            // The gate sits ABOVE the router, so the frameless desktop
            // window has no top bar here — without this strip a signed-out
            // window could not be moved or closed.
            GateChrome {}
            div { class: "flex min-h-screen items-center justify-center p-6",
                div { class: "flex w-full max-w-sm flex-col items-center gap-3",
                    p { class: "text-sm text-muted-foreground", "Restoring your session…" }
                    if let Some(msg) = err {
                        p { class: "text-center text-xs text-destructive", "{msg}" }
                    }
                    details { class: "w-full text-sm",
                        summary { class: "cursor-pointer text-center text-muted-foreground",
                            "Taking too long?"
                        }
                        div { class: "flex flex-col gap-4 pt-3",
                            LoginForm {}
                            crate::server_registry::ServersPanel {}
                        }
                    }
                }
            }
        };
    }
    rsx! {
        GateChrome {}
        div { class: "flex min-h-screen items-center justify-center p-6",
            div { class: "flex w-full max-w-sm flex-col gap-4",
                div { class: "flex flex-col gap-1",
                    h1 { class: "text-lg font-semibold", "Sign in to Task" }
                    p { class: "text-sm text-muted-foreground",
                        "This server's data is private to its members."
                    }
                }
                LoginForm {}
                // Without this a user pointed at the wrong server is
                // stuck: they cannot sign in, and the switcher that would
                // let them change servers lives inside the app they can't
                // reach.
                details { class: "text-sm",
                    summary { class: "cursor-pointer text-muted-foreground",
                        "Connect to a different server"
                    }
                    div { class: "pt-2",
                        crate::server_registry::ServersPanel {}
                    }
                }
            }
        }
    }
}

/// Turn what the issuer says about a token into an account.
///
/// Shared by the redirect sign-in and by boot restore, so a session
/// restored from cache is identical to the one that created it — a
/// difference between those two is the kind of bug that only shows up
/// after a reload.
fn central_account(
    user: crate::central_login::UserInfo,
    token: String,
) -> Result<ActiveAccount, String> {
    // `sub` is a string in OIDC but a uuid everywhere in Task, and the
    // membership rows are keyed on the uuid. An issuer that handed back
    // something else would otherwise be discovered much later, as a
    // lookup that silently never matches.
    let user_id = user
        .sub
        .parse::<Uuid>()
        .map_err(|_| format!("the issuer returned a non-uuid subject: {}", user.sub))?;
    // An account with no email still signs in; the id is the part that
    // has to exist, and it is what memberships key on.
    let email = user.email.clone().unwrap_or_else(|| user.sub.clone());
    Ok(ActiveAccount {
        user_id,
        name: user.name.unwrap_or_else(|| email.clone()),
        email,
        token,
    })
}

/// Token-cache-first session resolution against the home org.
async fn resolve_session(slug: &str, email: &str) -> Result<ActiveAccount, String> {
    let client = establish_for::<AuthServiceClient>(slug).await?;

    // 1. Cached token → whoami validates it without a fresh sign-in.
    if let Some(token) = load_cached_token(email) {
        match client.whoami(token.clone()).await {
            Ok(user) => return Ok(account_from(user, email, token)),
            // 1b. The org's auth store cannot validate a token it did
            //     not mint, and a central sign-in leaves exactly that:
            //     an OAuth access token from the issuer. Ask the issuer
            //     before giving up — otherwise every reload throws a
            //     centrally-signed-in person back to the login screen
            //     holding a token that was never actually expired.
            Err(_) => {
                if let Some(issuer) = task_ui_core::central_auth::issuer() {
                    if let Ok(user) = crate::central_login::user_info(&issuer, &token).await {
                        if let Ok(account) = central_account(user, token) {
                            return Ok(account);
                        }
                    }
                }
                clear_cached_token(email); // expired/revoked — fall through
            }
        }
    }

    // 2. No usable cached token. A debug build has a password on file
    //    for the dev roster, so the switch completes in one click. In
    //    release the roster is empty, there is nothing to sign in with,
    //    and the user goes through `LoginForm` — which is already on
    //    screen — hence the message rather than a failed sign-in.
    let Some(dev) = dev_accounts().iter().find(|a| a.email == email) else {
        return Err(format!("sign in as {email} to continue"));
    };
    let bundle = client
        .sign_in_email_password(SignInEmailPassword {
            email: email.to_owned(),
            password: dev.password.to_owned(),
            ip_address: None,
            user_agent: Some("task-web".to_owned()),
        })
        .await
        .map_err(|e| format!("sign in {email}: {e}"))?;
    save_cached_token(email, &bundle.token);
    Ok(account_from(bundle.user, email, bundle.token))
}

/// Is this page load the central issuer redirecting back to us?
///
/// Read off `window.location` rather than the router, because
/// [`SignInGate`] sits above the router and has no route to match on.
#[cfg(target_arch = "wasm32")]
fn at_central_callback() -> bool {
    web_sys::window()
        .and_then(|w| w.location().pathname().ok())
        .is_some_and(|path| path.trim_end_matches('/') == "/auth/callback")
}

/// Native builds never make the round trip — the form signs in without
/// leaving the process.
#[cfg(not(target_arch = "wasm32"))]
fn at_central_callback() -> bool {
    false
}

/// Build the context value from an `AuthUser`, with dev-roster
/// fallbacks for the optional fields.
fn account_from(user: AuthUser, email: &str, token: String) -> ActiveAccount {
    let dev_name = dev_accounts()
        .iter()
        .find(|a| a.email == email)
        .map(|a| a.name.to_owned());
    ActiveAccount {
        user_id: user.id,
        email: user.email.unwrap_or_else(|| email.to_owned()),
        name: user
            .name
            .filter(|n| !n.trim().is_empty())
            .or(dev_name)
            .unwrap_or_else(|| email.to_owned()),
        token,
    }
}

// ── avatars ─────────────────────────────────────────────────────────
// The identity system (gradients + hash + initials) lives in
// task-ui-core::avatar so surfaces outside this crate (the review
// rail in files-ui) render the SAME person the same way.

pub use task_ui_core::avatar::{gradient_index, initials};

/// Round initials avatar with a deterministic per-account gradient.
/// `email` keys the gradient; when it's empty the name keys it
/// (presence rows from peers that predate account identity).
pub use task_ui_core::avatar::Avatar;

// ── account & status, shared content ────────────────────────────────

/// The status-picker rows both presentations render: the manual
/// override value, its label, and the status whose dot previews it.
pub const STATUS_OPTIONS: [(ManualStatus, &str, PresenceStatus); 3] = [
    (ManualStatus::Auto, "Active (auto)", PresenceStatus::Active),
    (
        ManualStatus::Available,
        "Available",
        PresenceStatus::Available,
    ),
    (ManualStatus::Dnd, "Do not disturb", PresenceStatus::Dnd),
];

/// Email/password sign-in (and self-serve sign-up) form. Rides
/// [`AuthCtx`] — `sign_in`/`sign_up` are fire-and-forget; `busy`/`error`
/// come back on the context. Real architect-auth accounts; this is what
/// production shows in place of the dev-account picker.
#[component]
pub fn LoginForm() -> Element {
    let ctx = use_context::<AuthCtx>();
    let busy = ctx.busy;
    let mut error = ctx.error;
    let email = use_signal(String::new);
    let password = use_signal(String::new);
    let name = use_signal(String::new);
    let mut creating = use_signal(|| false);

    let submit = move |_| {
        let e = email.peek().trim().to_owned();
        let p = password.peek().clone();
        if e.is_empty() || p.is_empty() {
            return;
        }
        if *creating.peek() {
            ctx.sign_up(e, p, name.peek().trim().to_owned());
        } else {
            ctx.sign_in(e, p);
        }
    };

    // Read each render rather than cached in a signal: discovery
    // resolves asynchronously at boot, and a value captured once would
    // hide the button from anyone who reached this screen first.
    let central_issuer = task_ui_core::central_auth::issuer();

    let central_sign_in = move |_| {
        error.set(None);
        #[cfg(target_arch = "wasm32")]
        {
            match crate::central_login::redirect_uri() {
                Some(uri) => {
                    // Only returns on failure — success is the page
                    // navigating away to the issuer.
                    if let Err(e) = crate::central_login::begin(&uri) {
                        error.set(Some(e.to_string()));
                    }
                }
                None => error.set(Some("no browser context".to_owned())),
            }
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            // iOS installs a system authentication session at boot
            // (`ios_auth`); desktop has none yet and says so.
            if crate::central_login::native::available() {
                spawn(async move {
                    match crate::central_login::native::sign_in().await {
                        Ok(redeemed) => ctx.adopt_central_token(redeemed.token, redeemed.issuer),
                        Err(e) => error.set(Some(e.to_string())),
                    }
                });
            } else {
                error.set(Some(
                    "Signing in through the browser isn't available here — use your email and \
                     password."
                        .to_owned(),
                ));
            }
        }
    };

    rsx! {
        div { class: "flex flex-col gap-2",
            // The central account, offered first because it is the one
            // that spans every FastTrackStudio app — the form below
            // signs into this server alone. Absent entirely on a
            // self-hosted server, which advertises no issuer and where
            // the button would lead nowhere.
            if central_issuer.is_some() {
                Button {
                    variant: ButtonVariant::Primary,
                    size: ButtonSize::Medium,
                    on_click: central_sign_in,
                    class: "w-full",
                    "Continue with FastTrackStudio"
                }
                div { class: "flex items-center gap-2 py-1",
                    div { class: "h-px flex-1 bg-border" }
                    span { class: "text-xs text-muted-foreground", "or" }
                    div { class: "h-px flex-1 bg-border" }
                }
            }
            if creating() {
                Input {
                    value: name,
                    placeholder: "Name",
                    on_change: move |_| error.set(None),
                }
            }
            Input {
                value: email,
                input_type: "email".to_string(),
                placeholder: "Email",
                on_change: move |_| error.set(None),
            }
            Input {
                value: password,
                input_type: "password".to_string(),
                placeholder: "Password",
                on_change: move |_| error.set(None),
            }
            Button {
                variant: ButtonVariant::Primary,
                size: ButtonSize::Medium,
                loading: busy(),
                on_click: submit,
                class: "w-full",
                if creating() { "Create account" } else { "Sign in" }
            }
            button {
                r#type: "button",
                class: "text-xs text-muted-foreground hover:text-foreground",
                onclick: move |_| {
                    error.set(None);
                    let now = *creating.peek();
                    creating.set(!now);
                },
                if creating() { "Have an account? Sign in" } else { "Create an account" }
            }
            if let Some(msg) = error.read().as_ref() {
                div { class: "px-1 text-xs text-destructive", "{msg}" }
            }
            // One-click sign-in for the compiled dev roster or the demo
            // cast (debug builds; `dev_accounts` is empty in release).
            // This is the "choose who to sign in as" half of the demo:
            // boot lands on the cast's first member, signing out lands
            // here, and each of these is the real credential flow with
            // the password pre-filled.
            if !dev_accounts().is_empty() {
                div { class: "flex flex-col gap-1 pt-3",
                    div { class: "px-1 pb-1 text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                        "Sign in as"
                    }
                    for dev in dev_accounts().iter().copied() {
                        button {
                            key: "{dev.email}",
                            r#type: "button",
                            class: "flex min-h-[40px] w-full items-center gap-3 rounded-lg px-2 py-2 text-left hover:bg-accent",
                            onclick: move |_| {
                                error.set(None);
                                ctx.switch_account(dev.email);
                            },
                            Avatar { name: dev.name.to_string(), email: dev.email.to_string(), size: 24 }
                            span { class: "flex min-w-0 flex-col",
                                span { class: "truncate text-sm", "{dev.name}" }
                                span { class: "truncate text-xs text-muted-foreground", "{dev.email}" }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Change your own password.
///
/// Self-service: the server takes the CURRENT password as well as the
/// session, so a stolen session alone can't take an account over, and
/// knowing the password alone can't either. Strength and known-breach
/// checks live server-side, so their rejections surface here as the
/// error text rather than being re-implemented (and drifting) client-side.
#[component]
pub fn ChangePasswordForm() -> Element {
    let ctx = use_context::<AuthCtx>();
    let active = ctx.active;
    let current = use_signal(String::new);
    let next = use_signal(String::new);
    let confirm = use_signal(String::new);
    let mut status = use_signal(|| None::<Result<String, String>>);
    let mut busy = use_signal(|| false);

    let submit = move |_| {
        let (cur, new, conf) = (
            current.peek().clone(),
            next.peek().clone(),
            confirm.peek().clone(),
        );
        let Some(account) = active.peek().clone() else {
            status.set(Some(Err("sign in first".to_owned())));
            return;
        };
        if cur.is_empty() || new.is_empty() {
            status.set(Some(Err("fill in both passwords".to_owned())));
            return;
        }
        // Caught here rather than server-side: a typo in the confirm box
        // isn't the server's business, and a round trip to say so is a
        // worse experience than an immediate answer.
        if new != conf {
            status.set(Some(Err("the new passwords don't match".to_owned())));
            return;
        }
        busy.set(true);
        status.set(None);
        spawn(async move {
            let slug = crate::orgs::home_slug(&use_context::<Signal<Vec<OrgMeta>>>().peek());
            let outcome = async {
                let client = establish_for::<AuthServiceClient>(&slug).await?;
                client
                    .change_password(auth_proto::service::ChangePasswordRequest {
                        session_token: account.token.clone(),
                        current_password: cur,
                        new_password: new,
                    })
                    .await
                    .map_err(|e| format!("{e}"))
            }
            .await;
            busy.set(false);
            status.set(Some(match outcome {
                Ok(()) => Ok("Password changed.".to_owned()),
                Err(e) => Err(e),
            }));
        });
    };

    rsx! {
        div { class: "flex flex-col gap-2",
            Input {
                value: current,
                input_type: "password".to_string(),
                placeholder: "Current password",
                on_change: move |_| status.set(None),
            }
            Input {
                value: next,
                input_type: "password".to_string(),
                placeholder: "New password",
                on_change: move |_| status.set(None),
            }
            Input {
                value: confirm,
                input_type: "password".to_string(),
                placeholder: "Confirm new password",
                on_change: move |_| status.set(None),
            }
            Button {
                variant: ButtonVariant::Primary,
                size: ButtonSize::Medium,
                loading: busy(),
                on_click: submit,
                class: "w-full",
                "Change password"
            }
            match status.read().as_ref() {
                Some(Ok(msg)) => rsx! { div { class: "px-1 text-xs text-muted-foreground", "{msg}" } },
                Some(Err(msg)) => rsx! { div { class: "px-1 text-xs text-destructive", "{msg}" } },
                None => rsx! {},
            }
        }
    }
}

/// Change your own email.
///
/// Self-service counterpart to the operator migration: the server takes
/// the address from the session's own user, so there is no way to move
/// someone else's. The change appends to the account's email history and
/// the new address starts unverified, both of which the server handles.
#[component]
pub fn ChangeEmailForm() -> Element {
    let ctx = use_context::<AuthCtx>();
    let mut active = ctx.active;
    let orgs = use_context::<Signal<Vec<OrgMeta>>>();
    let next = use_signal(String::new);
    let mut status = use_signal(|| None::<Result<String, String>>);
    let mut busy = use_signal(|| false);

    let submit = move |_| {
        let new_email = next.peek().trim().to_owned();
        let Some(account) = active.peek().clone() else {
            status.set(Some(Err("sign in first".to_owned())));
            return;
        };
        if new_email.is_empty() {
            status.set(Some(Err("enter an email".to_owned())));
            return;
        }
        busy.set(true);
        status.set(None);
        spawn(async move {
            let slug = crate::orgs::home_slug(&orgs.peek());
            let outcome = async {
                let client = establish_for::<AuthServiceClient>(&slug).await?;
                client
                    .change_email(auth_proto::service::ChangeEmailRequest {
                        session_token: account.token.clone(),
                        new_email: new_email.clone(),
                    })
                    .await
                    .map_err(|e| format!("{e}"))
            }
            .await;
            busy.set(false);
            match outcome {
                Ok(user) => {
                    let shown = user.email.clone().unwrap_or(new_email);
                    // Keep the context in step so the switcher and
                    // presence don't keep showing the old address.
                    active.with_mut(|a| {
                        if let Some(a) = a.as_mut() {
                            a.email = shown.clone();
                        }
                    });
                    status.set(Some(Ok(format!("Email is now {shown}."))));
                }
                Err(e) => status.set(Some(Err(e))),
            }
        });
    };

    rsx! {
        div { class: "flex flex-col gap-2",
            Input {
                value: next,
                input_type: "email".to_string(),
                placeholder: "New email",
                on_change: move |_| status.set(None),
            }
            Button {
                variant: ButtonVariant::Secondary,
                size: ButtonSize::Medium,
                loading: busy(),
                on_click: submit,
                class: "w-full",
                "Change email"
            }
            match status.read().as_ref() {
                Some(Ok(msg)) => rsx! { div { class: "px-1 text-xs text-muted-foreground", "{msg}" } },
                Some(Err(msg)) => rsx! { div { class: "px-1 text-xs text-destructive", "{msg}" } },
                None => rsx! {},
            }
        }
    }
}

/// Account & status content for the mobile bottom sheet — the same
/// roster / status / sign-out actions as the desktop [`AccountSwitcher`]
/// popover (both ride [`AuthCtx`] + [`PresenceLocal`]), restyled as
/// touch-sized rows (≥44px). `on_done` fires after any action so the
/// hosting sheet can close.
#[component]
pub fn AccountSheetBody(on_done: EventHandler<()>) -> Element {
    let ctx = use_context::<AuthCtx>();
    let active = ctx.active;
    let error = ctx.error;
    let local = use_context::<PresenceLocal>();
    let mut manual = local.manual;

    let account = active.read().clone();
    let (name, email) = account.as_ref().map_or_else(
        || ("Signing in…".to_owned(), String::new()),
        |a| (a.name.clone(), a.email.clone()),
    );
    let active_email = account.as_ref().map(|a| a.email.clone());
    let effective = local.effective_status();
    let dot = effective.dot_class();
    let current_status = *manual.read();

    rsx! {
        div { class: "flex flex-col gap-4 pb-2",
            // Signed-in identity card.
            div { class: "flex items-center gap-3 rounded-xl border border-border bg-card px-3 py-3",
                span { class: "relative shrink-0",
                    Avatar { name: name.clone(), email: email.clone(), size: 40 }
                    span { class: "absolute -bottom-0.5 -right-0.5 h-3 w-3 rounded-full border-2 border-card {dot}",
                        title: "{effective.label()}",
                    }
                }
                span { class: "flex min-w-0 flex-col",
                    span { class: "truncate text-sm font-semibold text-foreground", "{name}" }
                    if !email.is_empty() {
                        span { class: "truncate text-xs text-muted-foreground", "{email}" }
                    }
                }
            }
            if let Some(msg) = error.read().as_ref() {
                div { class: "px-1 text-xs text-destructive", "{msg}" }
            }

            section {
                h3 { class: "px-1 pb-1 text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                    "Servers"
                }
                crate::server_registry::ServersPanel {}
            }

            section {
                h3 { class: "px-1 pb-1 text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                    "Sign in"
                }
                LoginForm {}
            }

            // Dev-account quick picker — one-click switch, DEBUG builds only.
            if cfg!(debug_assertions) {
                section {
                    h3 { class: "px-1 pb-1 text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                        "Dev accounts"
                    }
                    div { class: "flex flex-col",
                        for dev in dev_accounts().iter().copied() {
                            button {
                                key: "{dev.email}",
                                r#type: "button",
                                class: "flex min-h-[44px] w-full items-center gap-3 rounded-lg px-2 py-2 text-left active:bg-accent",
                                onclick: move |_| {
                                    on_done.call(());
                                    ctx.switch_account(dev.email);
                                },
                                Avatar { name: dev.name.to_string(), email: dev.email.to_string(), size: 28 }
                                span { class: "flex min-w-0 flex-col",
                                    span { class: "truncate text-sm text-foreground", "{dev.name}" }
                                    span { class: "truncate text-xs text-muted-foreground", "{dev.email}" }
                                }
                                if active_email.as_deref() == Some(dev.email) {
                                    span { class: "ml-auto text-sm text-primary", "●" }
                                }
                            }
                        }
                    }
                }
            }

            section {
                h3 { class: "px-1 pb-1 text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                    "Status"
                }
                div { class: "flex flex-col",
                    for (value , label , status) in STATUS_OPTIONS {
                        button {
                            key: "{label}",
                            r#type: "button",
                            class: "flex min-h-[44px] w-full items-center gap-3 rounded-lg px-2 py-2 text-left active:bg-accent",
                            onclick: move |_| {
                                manual.set(value);
                                on_done.call(());
                            },
                            span { class: "h-2.5 w-2.5 rounded-full {status.dot_class()}" }
                            span { class: "text-sm text-foreground", "{label}" }
                            if current_status == value {
                                span { class: "ml-auto text-sm text-primary", "●" }
                            }
                        }
                    }
                }
            }

            button {
                r#type: "button",
                class: "flex min-h-[44px] w-full items-center justify-center rounded-lg border border-destructive/40 px-3 py-2 text-sm font-medium text-destructive active:bg-destructive/10",
                onclick: move |_| {
                    on_done.call(());
                    ctx.sign_out();
                },
                "Sign out"
            }
        }
    }
}

// ── bottom-left account switcher ────────────────────────────────────

/// Account switcher: avatar + presence dot opening a popover with the
/// account roster (instant switch), the presence status section
/// (Auto/Available/DND), and sign-out. Two skins: the full identity
/// card (default), and `rail` — an icon-sized avatar button for the
/// icon rail's foot, where name/email live inside the popover instead.
#[component]
pub fn AccountSwitcher(#[props(default = false)] rail: bool) -> Element {
    let ctx = use_context::<AuthCtx>();
    let active = ctx.active;
    let error = ctx.error;
    let local = use_context::<PresenceLocal>();
    let mut manual = local.manual;
    let mut open = use_signal(|| false);
    // The "Servers…" item opens a modal hosting the same Servers +
    // Sign-in surface the mobile account sheet carries — a dialog, not
    // the roving-focus menu, so the text inputs behave.
    let mut servers_open = use_signal(|| false);

    let account = active.read().clone();
    let (name, email) = account.as_ref().map_or_else(
        || ("Signing in…".to_owned(), String::new()),
        |a| (a.name.clone(), a.email.clone()),
    );
    let active_email = account.as_ref().map(|a| a.email.clone());

    let effective = local.effective_status();
    let dot = effective.dot_class();
    let current_status = *manual.read();
    let status_options = STATUS_OPTIONS;

    rsx! {
        div { class: if rail { "flex flex-col" } else { "flex w-full flex-col gap-1" },
            Dropdown {
                open: open(),
                on_open_change: move |o| open.set(o),
                class: if rail { "" } else { "w-full" },
                DropdownTrigger { class: if rail { "" } else { "w-full" },
                    if rail {
                        button {
                            "data-testid": "account-switcher",
                            r#type: "button",
                            class: "flex h-8 w-8 items-center justify-center rounded-lg hover:bg-accent/50",
                            title: "Account & status — {name}",
                            span { class: "relative shrink-0",
                                Avatar { name: name.clone(), email: email.clone(), size: 26 }
                                span { class: "absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-card {dot}",
                                    title: "{effective.label()}",
                                }
                            }
                        }
                    } else {
                        button {
                            "data-testid": "account-switcher",
                            r#type: "button",
                            class: "flex w-full items-center gap-2 rounded-xl border border-border bg-card px-2 py-1.5 text-left hover:bg-accent",
                            title: "Account & status",
                            span { class: "relative shrink-0",
                                Avatar { name: name.clone(), email: email.clone(), size: 32 }
                                // Presence status dot, Discord-style on the
                                // avatar's corner.
                                span { class: "absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-card {dot}",
                                    title: "{effective.label()}",
                                }
                            }
                            span { class: "flex min-w-0 flex-col",
                                span { class: "truncate text-xs font-semibold text-foreground", "{name}" }
                                if !email.is_empty() {
                                    span { class: "truncate text-[11px] text-muted-foreground", "{email}" }
                                }
                            }
                        }
                    }
                }
                DropdownContent { side: "top", align: "start", width: "w-64",
                    // Rail skin: the trigger is a bare avatar, so the
                    // identity (name/email) heads the popover instead.
                    if rail {
                        div { class: "flex items-center gap-2 px-2 pb-1 pt-0.5",
                            Avatar { name: name.clone(), email: email.clone(), size: 28 }
                            span { class: "flex min-w-0 flex-col",
                                span { class: "truncate text-xs font-semibold text-foreground", "{name}" }
                                if !email.is_empty() {
                                    span { class: "truncate text-[11px] text-muted-foreground", "{email}" }
                                }
                            }
                        }
                        DropdownSeparator {}
                    }
                    DropdownLabel { "Account" }
                    for (idx, dev) in dev_accounts().iter().copied().enumerate() {
                        DropdownItem {
                            key: "{dev.email}",
                            value: dev.email.to_string(),
                            index: idx,
                            on_select: move |_| {
                                open.set(false);
                                ctx.switch_account(dev.email);
                            },
                            div { class: "flex w-full items-center justify-between gap-2",
                                span { class: "flex min-w-0 items-center gap-2",
                                    Avatar { name: dev.name.to_string(), email: dev.email.to_string(), size: 22 }
                                    span { class: "truncate", "{dev.name}" }
                                }
                                if active_email.as_deref() == Some(dev.email) {
                                    span { class: "text-xs text-primary", "●" }
                                }
                            }
                        }
                    }
                    DropdownSeparator {}
                    DropdownItem {
                        value: "__servers".to_string(),
                        index: dev_accounts().len(),
                        on_select: move |_| {
                            open.set(false);
                            servers_open.set(true);
                        },
                        "Servers…"
                    }
                    DropdownSeparator {}
                    DropdownLabel { "Status" }
                    for (idx, (value, label, status)) in status_options.into_iter().enumerate() {
                        DropdownItem {
                            key: "{label}",
                            value: label.to_string(),
                            index: dev_accounts().len() + 1 + idx,
                            on_select: move |_| {
                                manual.set(value);
                                open.set(false);
                            },
                            div { class: "flex w-full items-center justify-between gap-2",
                                span { class: "flex items-center gap-2",
                                    span { class: "h-2 w-2 rounded-full {status.dot_class()}" }
                                    span { "{label}" }
                                }
                                if current_status == value {
                                    span { class: "text-xs text-primary", "●" }
                                }
                            }
                        }
                    }
                    DropdownSeparator {}
                    DropdownItem {
                        value: "__sign_out".to_string(),
                        index: dev_accounts().len() + status_options.len() + 1,
                        destructive: true,
                        on_select: move |_| {
                            open.set(false);
                            ctx.sign_out();
                        },
                        "Sign out"
                    }
                }
            }
            if let Some(msg) = error.read().as_ref() {
                // This sits in the 48px icon rail, and the message is far
                // wider ("sign in as <email> to continue" measures ~150px).
                // Rendered raw it centred to x = (48 - 150) / 2 = -51 and
                // hung off the LEFT EDGE OF THE VIEWPORT — the user saw an
                // unreadable red sliver in the corner with no way to act on
                // it, while the app sat on "Signing in…" forever.
                //
                // So: fit the rail, keep the full text in the tooltip, and
                // make it do the thing it's asking for. `LoginForm` lives
                // inside this dialog, which is what the release-build
                // sign-in path assumes is "already on screen" — it isn't
                // until something opens it.
                button {
                    class: "mx-auto flex max-w-full items-center justify-center truncate rounded px-1 text-[11px] text-destructive hover:bg-destructive/10",
                    title: "{msg}",
                    onclick: move |_| servers_open.set(true),
                    "Sign in"
                }
            }

            // Servers + sign-in modal — the desktop surface for the
            // multi-server registry (mirrors the mobile account sheet).
            Dialog {
                open: servers_open(),
                on_open_change: move |o| servers_open.set(o),
                class: "sm:max-w-lg",
                DialogHeader {
                    DialogTitle { "Servers" }
                    DialogDescription {
                        "Add a server by URL, pick which one the app connects to, and sign in."
                    }
                }
                section { class: "flex flex-col gap-2",
                    h3 { class: "text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                        "Connections"
                    }
                    crate::server_registry::ServersPanel {}
                }
                section { class: "flex flex-col gap-2",
                    h3 { class: "text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                        if active.read().is_some() { "Account" } else { "Sign in" }
                    }
                    // Signed in, the useful action in this slot is
                    // changing your password; signed out it's signing in.
                    // Same place, because that's where someone looks for
                    // either.
                    if active.read().is_some() {
                        ChangeEmailForm {}
                        ChangePasswordForm {}
                    } else {
                        LoginForm {}
                    }
                }
            }
        }
    }
}

// ── localStorage persistence (web only) ─────────────────────────────

#[cfg(target_arch = "wasm32")]
const ACTIVE_KEY: &str = "task.auth.active";

// Used by the wasm storage fns + the key-shape test; native builds
// have no token cache yet, hence the cfg.
#[cfg(any(target_arch = "wasm32", test))]
fn token_key(email: &str) -> String {
    format!("task.auth.token.{email}")
}

#[cfg(target_arch = "wasm32")]
fn storage() -> Option<web_sys::Storage> {
    web_sys::window().and_then(|w| w.local_storage().ok().flatten())
}

#[cfg(target_arch = "wasm32")]
fn load_cached_token(email: &str) -> Option<String> {
    storage()
        .and_then(|s| s.get_item(&token_key(email)).ok().flatten())
        .filter(|t| !t.is_empty())
}

#[cfg(target_arch = "wasm32")]
fn save_cached_token(email: &str, token: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(&token_key(email), token);
    }
}

#[cfg(target_arch = "wasm32")]
fn clear_cached_token(email: &str) {
    if let Some(s) = storage() {
        let _ = s.remove_item(&token_key(email));
    }
}

#[cfg(target_arch = "wasm32")]
fn load_active_email() -> Option<String> {
    storage()
        .and_then(|s| s.get_item(ACTIVE_KEY).ok().flatten())
        .filter(|e| !e.is_empty())
}

#[cfg(target_arch = "wasm32")]
fn save_active_email(email: &str) {
    if let Some(s) = storage() {
        let _ = s.set_item(ACTIVE_KEY, email);
    }
}

#[cfg(target_arch = "wasm32")]
fn clear_active_email() {
    if let Some(s) = storage() {
        let _ = s.remove_item(ACTIVE_KEY);
    }
}

// Native (desktop/mobile): persist tokens through architect-auth's
// FileTokenStore (atomic, 0600) under `$XDG_DATA_HOME/task/ui-tokens/`
// so a signed-in account survives relaunch — one `<email>.json` per
// account, plus a plain `active` file naming the last account. On the
// iOS/macOS sandbox `HOME` is the app container, so this stays inside it.
#[cfg(not(target_arch = "wasm32"))]
/// Native data dir for cached sessions.
///
/// Apple platforms get `Library/Application Support`, not
/// `~/.local/share`: that is the sanctioned location for app data on
/// iOS/macOS, survives app updates, and is backed up. A dotted
/// XDG-style directory at the container root is a Linux convention iOS
/// makes no promises about — the same reason the server registry kept
/// forgetting its configuration between launches.
///
/// Kept in step with `server_registry::data_dir` deliberately: cached
/// session and chosen server are useless without each other, so they
/// must survive (or not) together.
fn tokens_dir() -> Option<std::path::PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return Some(xdg.join("task").join("ui-tokens"));
    }
    let home = std::path::PathBuf::from(std::env::var_os("HOME")?);
    let base = if cfg!(target_vendor = "apple") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("share")
    };
    Some(base.join("task").join("ui-tokens"))
}

/// A per-email token file. Emails are filename-safe on the targets we
/// ship, but sanitize defensively (only `/` is illegal on unix).
#[cfg(not(target_arch = "wasm32"))]
fn token_store(email: &str) -> Option<architect_auth::client::FileTokenStore> {
    let safe = email.replace(['/', '\\'], "_");
    tokens_dir()
        .map(|d| architect_auth::client::FileTokenStore::new(d.join(format!("{safe}.json"))))
}

#[cfg(not(target_arch = "wasm32"))]
fn load_cached_token(email: &str) -> Option<String> {
    use architect_auth::client::TokenStore as _;
    token_store(email)?
        .load()
        .ok()
        .flatten()
        .map(|s| s.token)
        .filter(|t| !t.is_empty())
}

#[cfg(not(target_arch = "wasm32"))]
fn save_cached_token(email: &str, token: &str) {
    use architect_auth::client::{StoredSession, TokenStore as _};
    if let Some(store) = token_store(email) {
        let _ = store.save(&StoredSession::new(token).with_email(email));
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_cached_token(email: &str) {
    use architect_auth::client::TokenStore as _;
    if let Some(store) = token_store(email) {
        let _ = store.clear();
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn active_path() -> Option<std::path::PathBuf> {
    tokens_dir().map(|d| d.join("active"))
}

#[cfg(not(target_arch = "wasm32"))]
fn load_active_email() -> Option<String> {
    let raw = std::fs::read_to_string(active_path()?).ok()?;
    let email = raw.trim();
    (!email.is_empty()).then(|| email.to_owned())
}

#[cfg(not(target_arch = "wasm32"))]
fn save_active_email(email: &str) {
    if let Some(path) = active_path() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, email);
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn clear_active_email() {
    if let Some(path) = active_path() {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cast_parses_and_skips_malformed_entries() {
        let cast = parse_cast(
            "alice@acme.test:pw:Alice:alice, sam@acme.test:pw , \
             casey@client.test:pw:Casey, nonsense, :nopass",
        );
        let brief: Vec<(&str, &str, &str)> =
            cast.iter().map(|a| (a.email, a.name, a.username)).collect();
        assert_eq!(
            brief,
            vec![
                ("alice@acme.test", "Alice", "alice"),
                // name falls back to the email, username to empty
                ("sam@acme.test", "sam@acme.test", ""),
                ("casey@client.test", "Casey", ""),
            ]
        );
    }

    #[test]
    fn gradient_index_is_deterministic_and_in_range() {
        for dev in DEV_ACCOUNTS {
            let idx = gradient_index(dev.email);
            assert!(idx < task_ui_core::avatar::AVATAR_GRADIENTS.len());
            assert_eq!(idx, gradient_index(dev.email), "stable across calls");
        }
    }

    #[test]
    fn gradient_index_spreads_the_dev_roster() {
        // Not a uniformity proof — just pin that the four dev accounts
        // don't all collapse onto one gradient (a regression guard for
        // hash fn edits).
        let mut seen: Vec<usize> = DEV_ACCOUNTS
            .iter()
            .map(|a| gradient_index(a.email))
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert!(seen.len() >= 2, "dev accounts share one gradient: {seen:?}");
    }

    #[test]
    fn initials_take_first_letters_of_two_words() {
        assert_eq!(initials("Cody Wright"), "CW");
        assert_eq!(initials("Carter Whitlock"), "CW");
        assert_eq!(initials("tom brooks"), "TB");
        assert_eq!(initials("Guest"), "GU");
        assert_eq!(initials("  spaced   out  "), "SO");
        assert_eq!(initials(""), "?");
    }

    #[test]
    fn guest_is_a_dev_account() {
        assert!(DEV_ACCOUNTS.iter().any(|a| a.email == GUEST_EMAIL));
    }

    #[test]
    fn token_keys_are_per_email() {
        assert_eq!(
            token_key("cody@fasttrackstudios.com"),
            "task.auth.token.cody@fasttrackstudios.com"
        );
        assert_ne!(token_key("a@x"), token_key("b@x"));
    }
}
