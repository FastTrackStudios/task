//! Cached vox service clients — cross-target (wasm + native).
//!
//! Establishing a vox client opens a WebSocket and runs the handshake.
//! On **wasm** the client's wasm-bindgen callback closures live exactly
//! as long as the established client value — a naive connect → use → drop
//! tears those closures down at the end of the request while the socket
//! can still fire a queued event, surfacing as
//! `closure invoked recursively or after being dropped` and a dead page.
//! So on wasm each `(service, org)` client is established **once** and
//! cached for the page's lifetime, then reused for every request.
//!
//! ## One socket per endpoint (2026-08-06)
//!
//! Production traces caught a single page load opening **72 sockets** to
//! `/org/{slug}/vox`. Two independent causes, both fixed here:
//!
//! 1. **Native established per call.** It had no dropped-closure hazard,
//!    so skipping the cache looked harmless — but "per call" means one
//!    WebSocket per typed client. Native now shares the cache.
//!
//! 2. **The cache had a thundering-herd hole**, and this one hit *both*
//!    targets. An entry can only be inserted after its dial completes, so
//!    every caller arriving during that first dial also missed and dialed.
//!    A page load fans out to dozens of services at once — precisely that
//!    window. The extra roots were then discarded, so the cache looked
//!    like it was working; the only symptoms were a slow load and dozens
//!    of wasted handshakes. [`shared_caller_at`] now single-flights, so
//!    concurrent callers await the same dial.
//!
//! Both targets now resolve an endpoint to ONE root connection and build
//! typed clients as cheap views over its caller. The server's per-org
//! `LayerRouter` was always able to dispatch every service on one
//! connection; only the client was fanning out.
//!
//! Both targets share one transport — `vox_websocket::WsLink::connect`
//! (web-sys `WebSocket` on wasm, `tokio-tungstenite` on native) plus
//! vox-core's `initiator_on(..).establish()`. There is **no** per-target
//! API duplication: architect's generated clients carry no transport
//! assumptions, and the vox stack is already cross-target, so the same
//! `establish_for` works on both.
//!
//! ## Liveness
//!
//! A cached root can die (server restart, socket drop). Every cache
//! access **validates** the root via `Caller::is_connected()` — vox's
//! session-liveness primitive, false the moment the session observes
//! transport EOF/error — and a dead entry is evicted + transparently
//! re-established. We validate per-access rather than keying entries by
//! the app `Connection`'s generation because this cache is *below* that
//! layer: multi-org fan-out (`feeds::*`) reaches it for orgs the app
//! connection isn't even pointed at, so the root's own liveness is the
//! only invariant that always applies. The generation still drives
//! hook-level invalidation upstream (`architect::Connection::generation`).

use crate::vox_session::vox_url;

/// Establish a client of type `C` against `url`, presenting `bearer` at
/// the handshake — no caching. Shared by every public helper; cross-target.
///
/// The dial itself is [`task_dial::establish_at`]: the browser's
/// cancel-safe socket, the native `Authorization` handshake, and the
/// subprotocol rule that carries a token where a browser can set no
/// headers. All of that used to live here, which is why the web app had a
/// working browser dial while every other application hand-rolled one —
/// and the cancel-safety is exactly the part a hand-rolled version gets
/// wrong, which kills the page rather than the request.
///
/// What stays here is the **caching** above it (see the module docs): one
/// root per endpoint, single-flighted, revalidated on every access. That
/// is a UI policy, not a property of dialling.
async fn establish_at<C>(url: &str, bearer: Option<&str>) -> Result<C, String>
where
    C: vox_core::FromVoxLane + 'static,
{
    if url.is_empty() {
        return Err("no vox URL configured (set TASK_VOX_URL[_WEB])".to_owned());
    }
    task_dial::establish_at(url, bearer).await
}

/// Untyped root lane — retains the raw [`vox_core::Caller`] plus the
/// [`vox_core::ConnectionHandle`] (dropping the handle tears the
/// connection down). The rc-fleet replacement for the old fork's
/// `NoopClient`: typed clients are cheap views built from the caller.
#[derive(Clone)]
pub struct RootLane {
    /// The established lane's caller.
    pub caller: vox_core::Caller,
    /// Underlying connection — kept alive with the cache entry.
    _connection: Option<vox_core::ConnectionHandle>,
}

impl vox_core::FromVoxLane for RootLane {
    const SERVICE_NAME: &'static str = "Noop";

    fn from_vox_lane(
        caller: vox_core::Caller,
        connection: Option<vox_core::ConnectionHandle>,
    ) -> Self {
        Self {
            caller,
            _connection: connection,
        }
    }
}

/// Per-org vox endpoint: the configured base retargeted at
/// `/org/<slug>/vox`.
///
/// Errors on an **empty slug** rather than producing `/org//vox` — this
/// is the choke point every org client funnels through, so no caller
/// can dial before org discovery has resolved a real slug (the app-root
/// supervisor and several hooks run with `home_slug` == "" until the
/// well-known fetch lands; previously that raced into a doomed
/// WebSocket to `/org//vox` plus a console error). Callers just retry /
/// re-run when the org-list signal fires.
/// Percent-encode a URL query component: everything but unreserved
/// characters (RFC 3986) is `%XX`-escaped.
fn percent_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn org_ws_url(slug: &str) -> Result<String, String> {
    if slug.is_empty() {
        return Err("awaiting org discovery (no org slug yet)".to_owned());
    }
    let base = vox_url();
    if base.is_empty() {
        return Err("no vox URL configured (set TASK_VOX_URL[_WEB])".to_owned());
    }
    let trimmed = base.trim_end_matches("/vox").trim_end_matches('/');
    // Share-guest mode (issue #272): the app was opened from a review
    // link and holds no account — every org connection dials the
    // token-scoped guest lane instead. Same wire contract, so every
    // typed client works unchanged.
    if let Some(guest) = crate::vox_session::guest_share()
        && guest.org == slug
    {
        let pw = match &guest.pw {
            // Percent-encode: the password is user-chosen text riding a
            // URL query — '&', '#', '+', '=' etc. would truncate or
            // corrupt it (the landing page encodes the same value).
            Some(pw) if !pw.is_empty() => format!("?pw={}", percent_encode_component(pw)),
            _ => String::new(),
        };
        return Ok(format!(
            "{trimmed}/org/{slug}/share/{}/vox{pw}",
            guest.token
        ));
    }
    Ok(format!("{trimmed}/org/{slug}/vox"))
}

/// Server-level vox endpoint (`/server/vox`) — the process-wide surface
/// (identity locker, etc.), NOT a per-org one. Normalizes any active
/// base — `wss://host/vox`, bare `wss://host`, or a per-org
/// `wss://host/org/<slug>/vox` — down to `wss://host/server/vox` by
/// keeping only the scheme + authority.
fn server_ws_url(base_override: Option<&str>) -> Result<String, String> {
    let base = base_override.map(str::to_owned).unwrap_or_else(vox_url);
    let base = base.trim();
    if base.is_empty() {
        return Err("no vox URL configured (set TASK_VOX_URL[_WEB])".to_owned());
    }
    let (scheme, rest) = base.split_once("://").unwrap_or(("wss", base));
    let host = rest.split('/').next().unwrap_or(rest);
    Ok(format!("{scheme}://{host}/server/vox"))
}

/// Establish *any* service client against the server-level `/server/vox`
/// endpoint (see [`server_ws_url`]). Cross-target — `establish_at`
/// handles the wasm vs native transport. Used for the identity locker,
/// which is mounted per server-process, not per org.
pub async fn establish_server<C>(base_override: Option<&str>) -> Result<C, String>
where
    C: vox_core::FromVoxLane + Clone + 'static,
{
    let caller = shared_caller_at(&server_ws_url(base_override)?).await?;
    Ok(C::from_vox_lane(caller, None))
}

/// Drop every cached connection, closing its socket.
///
/// A connection presents its identity ONCE, at establish, so a socket
/// opened while signed out stays anonymous for its whole life no matter
/// what the token holder says later. Signing in or out therefore has to
/// tear the old sockets down; the next call re-dials under the new
/// identity. `crate::vox_session::set_session_token` reports whether the
/// token actually changed, which is the trigger.
pub fn drop_cached_connections() {
    // In-flight dials first: a dial that completes after this would
    // otherwise publish a root established under the OLD identity.
    with_inflight(std::collections::HashMap::clear);
    with_roots(std::collections::HashMap::clear);
}

/// The shared root connection for one endpoint URL — establish once,
/// reuse for every service.
///
/// This is the single choke point that makes "one socket per endpoint"
/// true. Keyed by the **full URL**, not a slug: the same slug on two
/// servers (multi-server registry) must be two independent sockets, and
/// switching the active server must not hand back the previous server's
/// root.
///
/// The cache owns the [`RootLane`], which owns the
/// [`vox_core::ConnectionHandle`] — that ownership is what holds the
/// socket open, since typed clients built from the caller are views with
/// no session handle of their own.
///
/// Entries are validated on access via `Caller::is_connected()` rather
/// than keyed by the app `Connection`'s generation: this cache sits
/// *below* that layer (multi-org fan-out reaches it for orgs the app
/// connection isn't pointed at), so the root's own liveness is the only
/// invariant that always holds. A dead root is evicted and re-established
/// transparently.
/// What a cached connection is keyed by: the endpoint URL **and the
/// identity it was established under**.
///
/// The URL alone is not enough. A connection presents its bearer once, at
/// the WebSocket upgrade, so an anonymous root and a signed-in root to the
/// same endpoint are different connections that must never be swapped for
/// each other — handing a cached anonymous root to a signed-in caller is
/// exactly the silent-fail-open this whole change exists to remove.
type RootKey = (String, Option<String>);

/// A dial in progress, shared by every caller that asked for the same
/// URL + identity while it was in flight.
type SharedDial = futures_util::future::Shared<DialFuture>;
#[cfg(target_arch = "wasm32")]
type DialFuture = futures_util::future::LocalBoxFuture<'static, Result<vox_core::Caller, String>>;
#[cfg(not(target_arch = "wasm32"))]
type DialFuture = futures_util::future::BoxFuture<'static, Result<vox_core::Caller, String>>;

async fn shared_caller_at(url: &str) -> Result<vox_core::Caller, String> {
    shared_caller_with(url, crate::vox_session::bearer()).await
}

/// [`shared_caller_at`] with the identity chosen by the caller — the
/// per-org path, where the credential depends on WHICH org is being
/// dialled rather than on one ambient session.
async fn shared_caller_with(url: &str, bearer: Option<String>) -> Result<vox_core::Caller, String> {
    // Identity is part of the cache key, not just the dial: a root
    // established anonymously can never become authenticated (the server
    // read the bearer once, at upgrade), so handing it to a signed-in
    // caller would silently keep them anonymous. `drop_cached_connections`
    // clears the old identity's roots on sign-in/out; this keying is the
    // belt to that braces, covering the window where a dial started before
    // the token landed.
    //
    // Keying on the bearer is also what makes per-org tokens safe here:
    // two orgs with different credentials get two cache entries, never
    // one socket serving both.
    let key = (url.to_owned(), bearer.clone());
    if let Some(caller) = cached_live_caller(&key) {
        return Ok(caller);
    }
    // SINGLE-FLIGHT. The cache alone is not enough: it can only be
    // populated *after* a dial completes, so every caller that arrives
    // during the first dial also misses and dials. A page load fans out
    // to dozens of services at once, which is exactly that window —
    // measured at 72 concurrent dials to one endpoint. The losers were
    // then thrown away, so the cache "worked" and the symptom was purely
    // a slow load plus 71 wasted handshakes.
    //
    // So concurrent callers now await the SAME dial rather than starting
    // their own.
    let dial = with_inflight(|inflight| {
        if let Some(dial) = inflight.get(&key) {
            return dial.clone();
        }
        let owned = key.clone();
        let fut = async move {
            let root = establish_at::<RootLane>(&owned.0, owned.1.as_deref()).await?;
            let caller = insert_root(&owned, root);
            // Clear the in-flight slot so a later dial (after this root
            // dies) starts fresh. Callers already awaiting this `Shared`
            // still get its cached result.
            with_inflight(|inflight| inflight.remove(&owned));
            Ok(caller)
        };
        // `DialFuture` is already cfg'd (LocalBoxFuture on wasm, BoxFuture
        // on native), so this one line covers both targets.
        let dial: SharedDial = futures_util::FutureExt::shared(Box::pin(fut) as DialFuture);
        inflight.insert(key.clone(), dial.clone());
        dial
    });
    // Awaited with no lock held — the dial is a network round trip.
    dial.await
}

/// Run `f` against the per-target in-flight dial map. Same locking
/// discipline as [`with_roots`]: never held across an await.
#[cfg(target_arch = "wasm32")]
fn with_inflight<R>(f: impl FnOnce(&mut std::collections::HashMap<RootKey, SharedDial>) -> R) -> R {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static INFLIGHT: RefCell<HashMap<RootKey, SharedDial>> = RefCell::new(HashMap::new());
    }
    INFLIGHT.with(|m| f(&mut m.borrow_mut()))
}

#[cfg(not(target_arch = "wasm32"))]
fn with_inflight<R>(f: impl FnOnce(&mut std::collections::HashMap<RootKey, SharedDial>) -> R) -> R {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static INFLIGHT: OnceLock<Mutex<HashMap<RootKey, SharedDial>>> = OnceLock::new();
    let m = INFLIGHT.get_or_init(|| Mutex::new(HashMap::new()));
    let mut guard = m.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// Look up a cached root, evicting it if the connection has died.
fn cached_live_caller(key: &RootKey) -> Option<vox_core::Caller> {
    with_roots(|roots| match roots.get(key) {
        Some(root) if root.caller.is_connected() => Some(root.caller.clone()),
        Some(_) => {
            tracing::warn!(url = key.0, "vox: cached root is dead; re-establishing");
            roots.remove(key);
            None
        }
        None => None,
    })
}

/// Publish a freshly established root, resolving the dial race.
///
/// Two tasks can miss the cache and dial the same URL concurrently (the
/// lock is deliberately not held across the dial — a network round trip
/// under a global lock would serialize every org's first connect). The
/// loser drops its socket rather than evicting the winner, so callers
/// that already hold the winner's caller keep a live connection.
fn insert_root(key: &RootKey, root: RootLane) -> vox_core::Caller {
    with_roots(|roots| match roots.get(key) {
        Some(existing) if existing.caller.is_connected() => existing.caller.clone(),
        _ => {
            let caller = root.caller.clone();
            roots.insert(key.clone(), root);
            caller
        }
    })
}

/// Run `f` against the per-target root cache.
///
/// wasm is single-threaded, so a `thread_local` is the whole story.
/// Native needs a process-global map because the desktop app establishes
/// from whatever runtime thread happens to poll the future. The lock is
/// never held across an await (see [`insert_root`]), so a plain `std`
/// mutex is enough and this needs no async-lock dependency.
#[cfg(target_arch = "wasm32")]
fn with_roots<R>(f: impl FnOnce(&mut std::collections::HashMap<RootKey, RootLane>) -> R) -> R {
    use std::cell::RefCell;
    use std::collections::HashMap;
    thread_local! {
        static ROOTS: RefCell<HashMap<RootKey, RootLane>> = RefCell::new(HashMap::new());
    }
    ROOTS.with(|roots| f(&mut roots.borrow_mut()))
}

#[cfg(not(target_arch = "wasm32"))]
fn with_roots<R>(f: impl FnOnce(&mut std::collections::HashMap<RootKey, RootLane>) -> R) -> R {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static ROOTS: OnceLock<Mutex<HashMap<RootKey, RootLane>>> = OnceLock::new();
    let roots = ROOTS.get_or_init(|| Mutex::new(HashMap::new()));
    // Poisoning only means some other caller panicked mid-map-edit; the
    // map itself is still a valid cache, so recover rather than cascade.
    let mut guard = roots.lock().unwrap_or_else(|e| e.into_inner());
    f(&mut guard)
}

/// One shared [`vox_core::Caller`] per org — the handle every typed
/// client is built from. One socket per org on both targets, no matter
/// how many services a page touches: the server's per-org `LayerRouter`
/// dispatches every service on that one connection.
///
/// This also backs the app root's `Connection<Caller>`
/// (`architect::use_app_reactive` over the active org) — pages that
/// migrate to atom hooks build clients from the shared caller; legacy
/// `feeds::*` fns ride the same socket through [`establish_for`].
pub async fn caller_for(slug: &str) -> Result<vox_core::Caller, String> {
    // Per-org credential: auth stores are per-org, so the ambient token
    // is only valid for the org that issued it. `bearer_for` falls back
    // to the ambient one when the locker has no link for this slug.
    let bearer = crate::vox_session::bearer_for(slug);

    // Native prefers iroh whenever discovery has produced the org's
    // endpoint id (`iroh_transport`): dial by bare id, identity as
    // per-call bearer metadata — the registration model, not the dev
    // URL. Same cache, same single-flight, keyed by an `iroh://` pseudo
    // URL so an iroh root and a ws root to the same org can never be
    // mistaken for each other. A failed iroh dial falls back to the
    // WebSocket rather than failing the caller: a laptop that lost UDP
    // still has the URL it discovered the org through.
    #[cfg(not(target_arch = "wasm32"))]
    if let Some(id) = crate::iroh_transport::org_endpoint_id(slug) {
        // Share-guest sessions stay on ws: the guest lane is a
        // token-scoped HTTP route with no iroh counterpart.
        let guest = crate::vox_session::guest_share().is_some_and(|g| g.org == slug);
        if !guest {
            match shared_caller_iroh(&id, bearer.clone()).await {
                Ok(caller) => return Ok(caller),
                Err(err) => {
                    tracing::warn!(%slug, %err, "iroh dial failed; falling back to WebSocket");
                }
            }
        }
    }

    shared_caller_with(&org_ws_url(slug)?, bearer).await
}

/// The shared root over iroh — [`shared_caller_with`]'s twin, riding
/// the same cache and single-flight maps under an `iroh://<id>` key.
/// The bearer is part of the key for the same reason as on ws (an
/// anonymous root must never be handed to a signed-in caller), but it
/// travels differently: not on an upgrade — iroh has none — but as a
/// global middleware on the root caller, presented on every call by
/// every typed client built from it.
#[cfg(not(target_arch = "wasm32"))]
async fn shared_caller_iroh(id: &str, bearer: Option<String>) -> Result<vox_core::Caller, String> {
    let key = (format!("iroh://{id}"), bearer.clone());
    if let Some(caller) = cached_live_caller(&key) {
        return Ok(caller);
    }
    let dial = with_inflight(|inflight| {
        if let Some(dial) = inflight.get(&key) {
            return dial.clone();
        }
        let owned = key.clone();
        let id = id.to_owned();
        let fut = async move {
            let (caller, connection) = crate::iroh_transport::dial(&id, owned.1.as_deref()).await?;
            let root = RootLane {
                caller,
                _connection: connection,
            };
            let caller = insert_root(&owned, root);
            with_inflight(|inflight| inflight.remove(&owned));
            Ok(caller)
        };
        let dial: SharedDial = futures_util::FutureExt::shared(Box::pin(fut) as DialFuture);
        inflight.insert(key.clone(), dial.clone());
        dial
    });
    dial.await
}

/// Establish *any* service client against a specific org's vox endpoint:
/// a cheap typed view over the org's ONE cached connection
/// ([`caller_for`]). Identical on both targets.
pub async fn establish_for<C>(slug: &str) -> Result<C, String>
where
    C: vox_core::FromVoxLane + Clone + 'static,
{
    let caller = caller_for(slug).await?;
    Ok(C::from_vox_lane(caller, None))
}

/// A client against an endpoint that is not one of this server's orgs.
///
/// The central auth issuer is the case this exists for: it mounts the
/// *same* `AuthService` an org lane does, so the typed client works
/// against it unchanged — what differs is only which URL and which
/// credential, and both are the caller's to choose here.
///
/// **Shared**, unlike the private `establish_at` above, which dials
/// fresh each time: identity resolves on a connection the issuer keeps,
/// so one dial per issuer is right and one per call is not.
pub async fn establish_shared_at<C>(url: &str, bearer: Option<String>) -> Result<C, String>
where
    C: vox_core::FromVoxLane + Clone + 'static,
{
    let caller = shared_caller_with(url, bearer).await?;
    Ok(C::from_vox_lane(caller, None))
}

#[cfg(test)]
mod subprotocol_tests {
    //! The subprotocol rule itself moved to `task-dial` with the dial it
    //! belongs to, and its tests went with it. What is left here is the
    //! URL handling this module still owns.

    #[test]
    fn link_passwords_survive_url_hostile_characters() {
        // The guest dial splices the password into the ws URL query —
        // '&' would truncate it, '#' would end the URL, '+' would
        // decode as a space server-side. RFC 3986 unreserved passes
        // through untouched.
        assert_eq!(
            super::percent_encode_component("a&b #c+d=e"),
            "a%26b%20%23c%2Bd%3De"
        );
        assert_eq!(super::percent_encode_component("Ok-1._~x"), "Ok-1._~x");
    }
}
