//! Vox endpoint configuration.
//!
//! The connection machinery itself lives in [`crate::vox_clients`]
//! (one cached connection root per org, typed clients as views over
//! its caller) and the app root's `Connection<Caller>`
//! (`architect::use_app_reactive`). This module only answers "what
//! base URL do we dial" — compile-time on wasm, env on native.

/// Compile-time default for the wasm build (overridden by
/// `TASK_VOX_URL_WEB` at build time). Matches the dev server's bind.
pub const DEFAULT_VOX_URL: &str = "ws://127.0.0.1:18080/vox";

use std::sync::{LazyLock, RwLock};

/// The user-selected active server (from the multi-server registry).
/// When set, it overrides the env/same-origin default in [`vox_url`].
/// Held in a process-global so the plain (non-component) `vox_url` +
/// establish paths can read it; the app root keeps it in sync with the
/// active the app's server registry via [`set_active_server`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ActiveServer {
    /// vox base URL — `ws(s)://host[:port]` (a trailing `/vox` is fine;
    /// [`crate::vox_clients`] normalizes it).
    pub url: String,
    /// The session token issued when signing into this server, if any.
    pub token: Option<String>,
}

static ACTIVE: LazyLock<RwLock<Option<ActiveServer>>> = LazyLock::new(|| RwLock::new(None));

/// The session token for the DEFAULT (same-origin / env) server — the one
/// [`vox_url`] resolves when no registry entry is selected, which is the
/// deployed case (`task.starcommand.live` serves the app and vox from one
/// origin, and nothing is "selected").
///
/// Multi-server tokens live on the registry entry and reach us through
/// [`ActiveServer::token`]; this holder is what the *unselected* default
/// server has instead. [`bearer`] picks between them.
static SESSION_TOKEN: LazyLock<RwLock<Option<String>>> = LazyLock::new(|| RwLock::new(None));

/// Publish (or clear) the default server's session token. Called by the
/// auth root whenever the active account resolves or is signed out.
///
/// Returns `true` when the token actually CHANGED — the caller uses that
/// to tear down connections established under the previous identity
/// ([`crate::vox_clients::drop_cached_connections`]); a socket presents
/// its identity once, at establish, so an already-open anonymous socket
/// stays anonymous no matter what this holder says afterwards.
pub fn set_session_token(token: Option<String>) -> bool {
    let token = token.filter(|t| !t.trim().is_empty());
    match SESSION_TOKEN.write() {
        Ok(mut w) if *w != token => {
            *w = token;
            true
        }
        _ => false,
    }
}

/// Per-org session tokens from the home identity locker, keyed by org
/// slug.
///
/// Auth stores are per-org: a token issued by `codywright` is not a
/// credential anywhere else, and presenting it to another org resolves
/// to `anonymous`. That is why one global bearer can only ever make you
/// a member of ONE org, and why the org switcher could never show more.
///
/// `IdentityService::list_links` hands back the token for every org this
/// account has linked, so the client can present the RIGHT credential
/// per org instead of the same wrong one everywhere.
static LINKED_TOKENS: LazyLock<RwLock<std::collections::HashMap<String, String>>> =
    LazyLock::new(|| RwLock::new(std::collections::HashMap::new()));

/// Publish the locker's tokens. Returns `true` when the set changed —
/// callers tear down cached connections on a change, since a socket
/// presents its identity once, at establish.
pub fn set_linked_tokens(tokens: std::collections::HashMap<String, String>) -> bool {
    match LINKED_TOKENS.write() {
        Ok(mut w) if *w != tokens => {
            *w = tokens;
            true
        }
        _ => false,
    }
}

/// Every org slug the locker gave us a token for.
#[must_use]
pub fn linked_slugs() -> Vec<String> {
    LINKED_TOKENS
        .read()
        .map(|r| r.keys().cloned().collect())
        .unwrap_or_default()
}

/// The bearer to present when dialing vox.
///
/// A user-selected server carries its OWN token on the registry entry, and
/// only that one: falling back to the default server's token there would
/// present one server's credential to another. With nothing selected
/// (the same-origin default) the holder above is the identity.
#[must_use]
pub fn bearer() -> Option<String> {
    if let Some(server) = active_server() {
        return server.token.filter(|t| !t.trim().is_empty());
    }
    SESSION_TOKEN.read().ok().and_then(|r| r.clone())
}

/// The bearer for a SPECIFIC org — the locker's token when we hold one,
/// otherwise the ambient identity.
///
/// The fallback matters: the org that issued the ambient token is
/// normally not in the locker (you don't link your home to itself), so
/// falling back is what keeps home working, and any org we have no link
/// for behaves exactly as it did before.
#[must_use]
pub fn bearer_for(slug: &str) -> Option<String> {
    if let Ok(map) = LINKED_TOKENS.read()
        && let Some(token) = map.get(slug)
        && !token.trim().is_empty()
    {
        return Some(token.clone());
    }
    bearer()
}

/// Set (or clear) the active server. Called from the app root whenever
/// the active registry entry changes.
pub fn set_active_server(server: Option<ActiveServer>) {
    if let Ok(mut w) = ACTIVE.write() {
        *w = server.filter(|s| !s.url.trim().is_empty());
    }
}

/// The active server, if one is selected.
#[must_use]
pub fn active_server() -> Option<ActiveServer> {
    ACTIVE.read().ok().and_then(|r| r.clone())
}

/// The configured vox base URL.
///
/// Wasm resolution order:
/// 1. `TASK_VOX_URL_WEB` baked at build time (dev harnesses, the
///    multiplayer suite — explicit always wins);
/// 2. **same-origin at runtime** — `ws(s)://<location.host>/vox` —
///    when the page is served from a real host. This is what makes
///    one deployed image work behind any hostname: the deployment's
///    ingress routes `/vox` + `/org` + `/.well-known` to the server
///    on the same origin as the static bundle;
/// 3. [`DEFAULT_VOX_URL`] when the page itself is on
///    localhost/127.0.0.1 (a bare `dx serve` without the env —
///    same-origin would dial the static server's own port).
///
/// Native: `TASK_VOX_URL` at runtime, empty when unset.
#[must_use]
pub fn vox_url() -> String {
    // A user-selected server (multi-server registry) always wins.
    if let Some(server) = active_server() {
        return server.url;
    }
    #[cfg(target_arch = "wasm32")]
    {
        if let Some(baked) = option_env!("TASK_VOX_URL_WEB") {
            return baked.to_string();
        }
        if let Some(url) = same_origin_vox_url() {
            return url;
        }
        DEFAULT_VOX_URL.to_string()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var("TASK_VOX_URL").unwrap_or_default()
    }
}

/// A share-guest session (issue #272): the app was opened from a
/// review share link, holds no account, and every org connection must
/// dial the scoped guest lane instead of the org lane.
#[derive(Clone, Debug, PartialEq)]
pub struct GuestShare {
    pub org: String,
    /// The share-link token — the whole grant.
    pub token: String,
    /// The link password the visitor entered, if any.
    pub pw: Option<String>,
}

fn guest_share_slot() -> &'static std::sync::Mutex<Option<GuestShare>> {
    static SLOT: std::sync::OnceLock<std::sync::Mutex<Option<GuestShare>>> =
        std::sync::OnceLock::new();
    SLOT.get_or_init(|| std::sync::Mutex::new(None))
}

/// Enter (or leave, with `None`) share-guest mode. Set once at boot by
/// the guest entry page, before any connection is established.
pub fn set_guest_share(session: Option<GuestShare>) {
    *guest_share_slot().lock().expect("guest share slot") = session;
}

#[must_use]
pub fn guest_share() -> Option<GuestShare> {
    guest_share_slot().lock().expect("guest share slot").clone()
}

/// The guest session's HTTP(S) origin for media/rendition fetches —
/// `Some` only in share-guest mode with an explicit server pinned.
/// A guest app may be served from a different origin than the task
/// server (dev dx, static hosting), where origin-relative media URLs
/// would hit the app's static server; members keep relative URLs
/// (same-origin routing is the deployed shape).
#[must_use]
pub fn guest_http_base() -> Option<String> {
    guest_share()?;
    let server = active_server()?;
    let http = server
        .url
        .replacen("wss://", "https://", 1)
        .replacen("ws://", "http://", 1);
    Some(
        http.trim_end_matches("/vox")
            .trim_end_matches('/')
            .to_owned(),
    )
}

/// Derive `ws(s)://<host>/vox` from the page's own origin. `None` only
/// when there's no window (tests, workers) or a non-http(s) origin.
///
/// The FastTrackStudio / single-binary deploy serves the app AND vox from
/// one origin (the server, or an ingress routing `/vox`+`/org`+`/.well-known`
/// to it), so the page's own host is always the vox host — including on
/// localhost and LAN/Tailscale hosts. The separate-static-server dev flow
/// bakes `TASK_VOX_URL_WEB` (checked before this in `vox_url`), so it is
/// unaffected.
#[cfg(target_arch = "wasm32")]
fn same_origin_vox_url() -> Option<String> {
    let location = web_sys::window()?.location();
    let host = location.host().ok()?; // host[:port]
    let protocol = location.protocol().ok()?;
    if protocol != "http:" && protocol != "https:" {
        return None;
    }
    let scheme = match protocol.as_str() {
        "https:" => "wss",
        _ => "ws",
    };
    Some(format!("{scheme}://{host}/vox"))
}
