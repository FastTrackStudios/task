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

/// The subprotocol every dial offers and the server selects. Offering any
/// subprotocol makes the server's echo mandatory, so this is what lets the
/// bearer subprotocol below be added without breaking the handshake.
///
/// Mirrors `task_server::VOX_SUBPROTOCOL` — duplicated rather than
/// imported because the web client must not depend on the server crate.
#[cfg(any(target_arch = "wasm32", test))]
const VOX_SUBPROTOCOL: &str = "vox.v1";

/// Prefix of the subprotocol carrying the session token
/// (`task_server::VOX_BEARER_SUBPROTOCOL_PREFIX`).
#[cfg(any(target_arch = "wasm32", test))]
const VOX_BEARER_SUBPROTOCOL_PREFIX: &str = "vox.bearer.";

/// Establish a client of type `C` against `url`, presenting `bearer` at
/// the handshake — no caching. Shared by every public helper; cross-target.
///
/// ## Why the identity rides the handshake
///
/// vox middleware is per *typed client*, keyed to a service descriptor
/// (`Caller::call` skips middleware entirely when no service is attached,
/// and generated clients attach their own descriptor inside
/// `from_vox_lane`), so there is no per-call choke point here to hang a
/// token on — every one of the ~117 client constructions would have to
/// remember, and a forgotten one fails OPEN and silently. One connection
/// per endpoint is now genuinely true (see the module docs), so the token
/// is presented ONCE, at establish, and the server applies it to every
/// call on that connection.
async fn establish_at<C>(url: &str, bearer: Option<&str>) -> Result<C, String>
where
    C: vox_core::FromVoxLane + 'static,
{
    use vox_core::initiator_on;
    if url.is_empty() {
        return Err("no vox URL configured (set TASK_VOX_URL[_WEB])".to_owned());
    }
    #[cfg(target_arch = "wasm32")]
    let link = dial_ws(url, bearer).await?;
    #[cfg(not(target_arch = "wasm32"))]
    let link = dial_ws_native(url, bearer).await?;
    initiator_on(link)
        .establish::<C>()
        .await
        .map_err(|e| format!("establish `{url}`: {e:?}"))
}

/// The subprotocol list a dial offers: always [`VOX_SUBPROTOCOL`], plus
/// `vox.bearer.<token>` when signed in.
///
/// The token deliberately does NOT ride a URL query parameter — those land
/// in every proxy and access log on the path. Session tokens are
/// base64url-no-pad, whose alphabet is a subset of the RFC 7230 token
/// charset a subprotocol value must use, so no extra encoding is needed;
/// a token containing anything else is dropped rather than sent as a
/// malformed header that would fail the whole handshake.
///
/// Browser-only: native presents the identity as an `Authorization`
/// header instead (see [`dial_ws_native`] for why symmetry is a trap).
/// `cfg(test)` keeps it compiled for the unit tests, which run natively.
#[cfg(any(target_arch = "wasm32", test))]
fn subprotocols(bearer: Option<&str>) -> Vec<String> {
    let mut protos = vec![VOX_SUBPROTOCOL.to_owned()];
    if let Some(token) = bearer.filter(|t| {
        !t.is_empty()
            && t.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    }) {
        protos.push(format!("{VOX_BEARER_SUBPROTOCOL_PREFIX}{token}"));
    }
    protos
}

/// Native dial. Unlike a browser, a native client controls its handshake
/// request, so the token goes in a plain `Authorization: Bearer` header —
/// the same channel the HTTP surface (`/blobs`, `/media`, the watch
/// bridge) already accepts.
///
/// This replaces `WsLink::connect`, which takes only a URL. It builds the
/// same `tokio_tungstenite` stream and hands it to the public
/// `WsLink::new`.
///
/// ## Why native offers NO subprotocol
///
/// Symmetry with the browser dial would be nice, and costs an outage.
/// tungstenite is stricter than RFC 6455 here: the spec lets a server that
/// selects no subprotocol simply omit the response header (§4.2.2, and
/// browsers accept that), but tungstenite treats "I offered, you didn't
/// echo" as a **handshake failure**
/// (`SubProtocolError::NoSubProtocol`). So a native client offering
/// `vox.bearer.…` cannot talk to any peer that doesn't echo it — an older
/// task-server, or an ingress/proxy that drops the header. The
/// `Authorization` header needs no negotiation and has none of that
/// coupling, so native uses it alone.
#[cfg(not(target_arch = "wasm32"))]
async fn dial_ws_native(
    url: &str,
    bearer: Option<&str>,
) -> Result<vox_websocket::WsLink<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>, String>
{
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let mut request = url
        .into_client_request()
        .map_err(|e| format!("ws request `{url}`: {e:?}"))?;
    if let Some(token) = bearer.filter(|t| !t.is_empty()) {
        if let Ok(value) = format!("Bearer {token}").parse() {
            request.headers_mut().insert("authorization", value);
        }
    }
    let (stream, _response) = tokio_tungstenite::connect_async(request)
        .await
        .map_err(|e| format!("ws connect `{url}`: {e:?}"))?;
    Ok(vox_websocket::WsLink::new(stream))
}

/// Cancel-safe browser WebSocket dial (wasm replacement for
/// `vox_websocket::WsLink::connect`).
///
/// `WsLink::connect`'s dial phase is not cancel-safe: it attaches
/// `onopen`/`onerror` wasm-bindgen closures to the connecting socket and
/// only detaches them on the *success* path. On the error path — and,
/// worse, when the connect **future is dropped mid-dial** (the app-root
/// supervisor restarts the moment org discovery lands and its signal
/// dependency fires) — the closures drop while still attached to a
/// socket that hasn't finished failing. The browser then delivers the
/// socket's `error`/`close` event into the freed closure, surfacing as
/// an uncaught `closure invoked recursively or after being dropped`.
///
/// This dial keeps the connect-phase closures in a guard whose `Drop`
/// **detaches them from the socket first** (and closes a socket we're
/// abandoning), so no event can ever reach a dropped closure — drop
/// order inside one synchronous Rust drop can't be interleaved with
/// browser event dispatch. On success the guard detaches and hands the
/// open socket to `WsLink::new`, which installs the steady-state
/// handlers it owns.
#[cfg(target_arch = "wasm32")]
async fn dial_ws(url: &str, bearer: Option<&str>) -> Result<vox_websocket::WsLink, String> {
    use std::cell::RefCell;
    use std::rc::Rc;

    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    /// Connect-phase state: the socket plus its temporary handlers.
    /// Detaches the handlers before the closure fields drop; closes the
    /// socket unless the dial completed and ownership moved to `WsLink`.
    struct Dial {
        ws: web_sys::WebSocket,
        _onopen: Closure<dyn FnMut()>,
        _onerror: Closure<dyn FnMut(web_sys::Event)>,
        _onclose: Closure<dyn FnMut(web_sys::CloseEvent)>,
        keep_open: bool,
    }
    impl Drop for Dial {
        fn drop(&mut self) {
            // Detach FIRST — after these lines the browser holds no
            // reference into the closures, so dropping them (field drop,
            // right after this body) is always safe.
            self.ws.set_onopen(None);
            self.ws.set_onerror(None);
            self.ws.set_onclose(None);
            if !self.keep_open {
                // Abandoned dial (error or caller cancellation): tear the
                // socket down so it doesn't keep connecting in the void.
                let _ = self.ws.close();
            }
        }
    }

    // The browser's ONE lever on a WebSocket handshake: a browser cannot
    // set request headers, and the token must not ride the URL, so the
    // subprotocol list is where identity goes (see `subprotocols`).
    let protocols = js_sys::Array::new();
    for proto in subprotocols(bearer) {
        protocols.push(&wasm_bindgen::JsValue::from_str(&proto));
    }
    let ws = web_sys::WebSocket::new_with_str_sequence(url, &protocols)
        .map_err(|e| format!("WebSocket::new `{url}`: {e:?}"))?;
    ws.set_binary_type(web_sys::BinaryType::Arraybuffer);

    let (tx, rx) = futures_channel::oneshot::channel::<Result<(), String>>();
    let tx = Rc::new(RefCell::new(Some(tx)));

    // FnMut (not `Closure::once`) so a stray double-fire can't trip
    // wasm-bindgen's invoked-after-consumed check; the oneshot's
    // take() makes later fires no-ops.
    let tx_open = Rc::clone(&tx);
    let onopen = Closure::wrap(Box::new(move || {
        if let Some(tx) = tx_open.borrow_mut().take() {
            let _ = tx.send(Ok(()));
        }
    }) as Box<dyn FnMut()>);
    let tx_error = Rc::clone(&tx);
    let err_url = url.to_owned();
    let onerror = Closure::wrap(Box::new(move |_: web_sys::Event| {
        if let Some(tx) = tx_error.borrow_mut().take() {
            let _ = tx.send(Err(format!("WebSocket open failed: `{err_url}`")));
        }
    }) as Box<dyn FnMut(web_sys::Event)>);
    // `close` can arrive without a preceding `error` (clean rejection);
    // without this handler such a dial would hang forever.
    let tx_close = Rc::clone(&tx);
    let close_url = url.to_owned();
    let onclose = Closure::wrap(Box::new(move |e: web_sys::CloseEvent| {
        if let Some(tx) = tx_close.borrow_mut().take() {
            let _ = tx.send(Err(format!(
                "WebSocket closed during open: `{close_url}` (code {})",
                e.code()
            )));
        }
    }) as Box<dyn FnMut(web_sys::CloseEvent)>);

    ws.set_onopen(Some(onopen.as_ref().unchecked_ref()));
    ws.set_onerror(Some(onerror.as_ref().unchecked_ref()));
    ws.set_onclose(Some(onclose.as_ref().unchecked_ref()));

    let mut dial = Dial {
        ws,
        _onopen: onopen,
        _onerror: onerror,
        _onclose: onclose,
        keep_open: false,
    };

    // Cancellation-safe await: dropping this future drops `dial`, whose
    // Drop detaches the handlers and closes the half-open socket.
    rx.await.map_err(|_| "dial cancelled".to_owned())??;

    // Success: keep the socket, detach the connect-phase handlers (the
    // guard's Drop), and let WsLink install its own.
    dial.keep_open = true;
    let ws = dial.ws.clone();
    drop(dial);
    Ok(vox_websocket::WsLink::new(ws))
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
    shared_caller_with(&org_ws_url(slug)?, bearer).await
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

#[cfg(test)]
mod subprotocol_tests {
    use super::{VOX_SUBPROTOCOL, subprotocols};

    #[test]
    fn anonymous_offers_only_the_plain_protocol() {
        assert_eq!(subprotocols(None), vec![VOX_SUBPROTOCOL.to_owned()]);
        assert_eq!(subprotocols(Some("")), vec![VOX_SUBPROTOCOL.to_owned()]);
    }

    #[test]
    fn signed_in_appends_the_bearer_protocol() {
        // A real session token's shape: base64url, no padding.
        assert_eq!(
            subprotocols(Some("Zm9v-ba_r9")),
            vec!["vox.v1".to_owned(), "vox.bearer.Zm9v-ba_r9".to_owned()]
        );
    }

    #[test]
    fn a_token_outside_the_subprotocol_charset_is_dropped() {
        // Not a silent downgrade for real tokens — `generate_token`
        // produces base64url-no-pad, which always passes. This guards
        // the case where something else ends up in the holder: sending
        // it raw would produce a malformed header and fail the ENTIRE
        // handshake, so the page would go dark rather than degrade.
        for bad in ["has space", "has,comma", "base64+pad=", "quote\"d"] {
            assert_eq!(
                subprotocols(Some(bad)),
                vec![VOX_SUBPROTOCOL.to_owned()],
                "{bad:?} should not be offered as a subprotocol"
            );
        }
    }

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
