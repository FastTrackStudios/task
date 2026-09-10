//! Where a call goes, and what identity it carries.
//!
//! Two questions, kept apart on purpose:
//!
//! - **Routing** — given a flag, an environment variable, a stored
//!   session and a default, which server base wins, and what is the
//!   per-org endpoint on it? That is [`pick_server_base`] and friends,
//!   all pure functions over their inputs so the precedence can be
//!   unit-tested without a filesystem or a socket.
//! - **Identity** — which stored token, if any, may be presented to
//!   that endpoint? That is [`session_bearer_for`], and its answer is
//!   scoped twice over (by server and by org) for reasons its own doc
//!   sets out.
//!
//! [`TaskClient`](crate::TaskClient) composes the two. Nothing here
//! knows about services or clients.

use crate::session;

// ── routing ─────────────────────────────────────────────────────────

/// Pure precedence fold for "which server". `flag_or_env` is the
/// caller's explicit target (for the CLI, `--server` / `TASK_VOX_URL`,
/// already flag-over-env courtesy of clap), `session_url` the active
/// session entry's stored server.
///
/// Kept as a free function taking both inputs rather than reading them
/// itself, because precedence is the part worth testing and the part
/// worth reading, and neither survives being tangled up with
/// environment lookups.
#[must_use]
pub fn pick_server_base(flag_or_env: Option<&str>, session_url: Option<&str>) -> String {
    if let Some(u) = flag_or_env.filter(|u| !u.trim().is_empty()) {
        return session::normalize_server_base(u);
    }
    if let Some(u) = session_url.filter(|u| !u.trim().is_empty()) {
        return session::normalize_server_base(u);
    }
    session::DEFAULT_LOCAL_VOX.to_owned()
}

/// The per-org vox endpoint on a resolved base.
#[must_use]
pub fn org_vox_url(base: &str, slug: &str) -> String {
    format!("{base}/org/{slug}/vox")
}

/// The org slug carried by a per-org vox URL, if it has that shape.
/// `…/org/<slug>/vox` → `<slug>`.
#[must_use]
pub fn slug_of(url: &str) -> Option<&str> {
    url.rsplit_once("/org/")
        .and_then(|(_, rest)| rest.strip_suffix("/vox"))
}

/// HTTP(S) base for the server's plain HTTP routes (`/blobs/*`,
/// `/.well-known/*`), derived from a vox base (`ws→http`, `wss→https`).
#[must_use]
pub fn http_base_of(vox_base: &str) -> String {
    if let Some(rest) = vox_base.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = vox_base.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        vox_base.to_owned()
    }
}

/// Normalize any server reference to the **server-management**
/// endpoint (`…/server/vox` — `OrgManagementService` /
/// `SnapshotService`), the server-level counterpart of a per-org URL.
///
/// The `/vox` strip in the middle is not cosmetic: `TASK_VOX_URL` is
/// frequently set to a per-org *hint* ending in `/vox`, and appending
/// the management path to that yields `…/vox/server/vox`, which routes
/// nowhere and fails as a connection error rather than as the
/// configuration mistake it is.
#[must_use]
pub fn normalize_server_vox(raw: &str) -> String {
    if raw.ends_with("/server/vox") {
        return raw.to_owned();
    }
    let ws: String = if let Some(rest) = raw.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if let Some(rest) = raw.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if raw.starts_with("ws://") || raw.starts_with("wss://") {
        raw.to_owned()
    } else {
        format!("ws://{raw}")
    };
    let trimmed = ws.trim_end_matches('/').trim_end_matches("/vox");
    format!("{trimmed}/server/vox")
}

/// The default server-management endpoint when nothing is configured.
pub const DEFAULT_SERVER_VOX: &str = "ws://127.0.0.1:18080/server/vox";

// ── identity ────────────────────────────────────────────────────────

/// `scheme://host[:port]` of a URL — everything up to the first path
/// segment.
#[must_use]
pub fn origin(u: &str) -> &str {
    let (scheme, rest) = u.split_once("://").unwrap_or(("", u));
    let prefix = if scheme.is_empty() {
        0
    } else {
        scheme.len() + 3
    };
    let authority_len = rest.find('/').unwrap_or(rest.len());
    &u[..prefix + authority_len]
}

/// `host[:port]` of a URL, scheme dropped — `https://a/x` and
/// `wss://a/y` are the same server.
#[must_use]
pub fn authority(u: &str) -> &str {
    let o = origin(u);
    o.split_once("://").map_or(o, |(_, rest)| rest)
}

/// The stored session token to present when dialing `url`, if any.
///
/// Scoped to the target twice over:
///
/// - **by server** — a token is only offered to the same authority that
///   issued it, so pointing a client at another host never hands that
///   host the credential for the one we're signed into;
/// - **by org** — auth stores are per-org, so a token from `codywright`
///   is not a credential in `cbu`; it resolves to `anonymous` there. The
///   entry whose slug matches the URL's `/org/<slug>/vox` wins, and only
///   if none matches do we fall back to the active entry.
///
/// Without the org half, targeting `cbu` would present whichever
/// session happened to be active — right host, wrong org, refused — and
/// the refusal reads identically to being signed out.
///
/// A `None` here is not necessarily "signed out": it is also what a
/// session file whose token never made it to disk produces. See
/// [`crate::session`] — that module now makes the difference loud, and
/// this function is the reason it had to.
#[must_use]
pub fn session_bearer_for(url: &str) -> Option<String> {
    let session = session::load().ok().flatten()?;
    // Authority only: a session saved as `https://host` must sign a dial
    // to `wss://host/org/x/vox` — same server, different scheme — or the
    // call goes out anonymous and every command reads "not a member".
    let same_server =
        |e: &session::ServerEntry| authority(&e.url) == authority(url) && !e.token.is_empty();
    if let Some(slug) = slug_of(url)
        && let Some(entry) = session
            .servers
            .values()
            .find(|e| e.slug == slug && same_server(e))
    {
        return Some(entry.token.clone());
    }
    let entry = session.active_server()?;
    same_server(entry).then(|| entry.token.clone())
}

/// Dial `url` and establish `C`, presenting the stored session identity
/// on the handshake.
///
/// `vox::connect_lane` takes only a URL, and vox middleware is per typed
/// client (keyed to a service descriptor) rather than per connection — so
/// there is no choke point on the call path to hang a token on. The
/// identity therefore rides the WebSocket upgrade, as the web client does
/// it (`task_ui_core::vox_clients`), and the server applies it to every
/// call on the connection. Without this a client reaches the permission
/// gate as `principal=anonymous` on every RPC — fine while the gate is
/// observe-only, refused the moment `TASK_ENFORCE_PERMISSIONS=1`.
///
/// The token goes in `Authorization`, NOT the `vox.bearer.…` subprotocol
/// the browser uses: tungstenite fails the handshake outright when it
/// offers a subprotocol the peer doesn't echo, which would make a native
/// client unable to reach an older server or anything behind a proxy that
/// drops the header. See `dial_ws_native` in task-ui-core.
///
/// **wss:// callers must install a rustls `CryptoProvider` first.** A
/// binary whose graph unifies both `ring` and `aws-lc-rs` leaves rustls
/// unable to choose, and the failure surfaces here as an opaque connect
/// error. `apps/cli`'s `main()` installs `ring` explicitly; anything
/// else linking this crate for TLS must do the same.
pub async fn dial_authenticated<C>(url: &str) -> Result<C, vox_core::ConnectionError>
where
    C: vox_core::FromVoxLane,
{
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    // A tokenless dial stays on the stock path — identical behaviour to
    // an unauthenticated client, including its error shapes.
    let Some(token) = session_bearer_for(url) else {
        return vox::connect_lane(url).establish().await;
    };
    let request = {
        let build = || {
            let mut request = url.into_client_request().ok()?;
            request
                .headers_mut()
                .insert("authorization", format!("Bearer {token}").parse().ok()?);
            Some(request)
        };
        build()
    };
    // An unrepresentable URL or header is not an auth problem; let the
    // stock path produce its usual error for it.
    let Some(request) = request else {
        return vox::connect_lane(url).establish().await;
    };
    match tokio_tungstenite::connect_async(request).await {
        Ok((stream, _response)) => {
            vox_core::initiator_on(vox_websocket::WsLink::new(stream))
                .establish::<C>()
                .await
        }
        // Report through the stock path so the caller's error hint (and
        // the embedded fallback above it) still applies.
        Err(_) => vox::connect_lane(url).establish().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn org_path_is_not_part_of_identity_scope() {
        // Every org on one server shares the session, so the per-org
        // routing suffix must not make the token look out-of-scope.
        assert_eq!(
            origin("wss://task.starcommand.live/org/codywright/vox"),
            "wss://task.starcommand.live"
        );
        assert_eq!(
            origin("wss://task.starcommand.live"),
            "wss://task.starcommand.live"
        );
    }

    #[test]
    fn a_different_server_is_a_different_scope() {
        // The point of the check: pointing elsewhere must never hand
        // that host the credential we hold for this one.
        assert_ne!(
            origin("wss://task.starcommand.live/org/x/vox"),
            origin("wss://evil.example/org/x/vox"),
        );
        // Port and scheme are part of the authority, not decoration.
        assert_ne!(
            origin("ws://127.0.0.1:18080/vox"),
            origin("ws://127.0.0.1:9/vox")
        );
        assert_ne!(origin("ws://host/vox"), origin("wss://host/vox"));
    }

    #[test]
    fn flag_or_env_beats_session() {
        assert_eq!(
            pick_server_base(
                Some("wss://task.starcommand.live/vox"),
                Some("ws://127.0.0.1:18080")
            ),
            "wss://task.starcommand.live"
        );
        // …and the flip: env pointing local wins over a stored
        // remote session — the URL switch IS the selector.
        assert_eq!(
            pick_server_base(
                Some("ws://127.0.0.1:18080/vox"),
                Some("wss://task.starcommand.live")
            ),
            "ws://127.0.0.1:18080"
        );
    }

    #[test]
    fn session_beats_default() {
        assert_eq!(
            pick_server_base(None, Some("wss://task.starcommand.live/vox")),
            "wss://task.starcommand.live"
        );
        // Legacy "local" session entries resolve to the default.
        assert_eq!(
            pick_server_base(None, Some("local")),
            session::DEFAULT_LOCAL_VOX
        );
    }

    #[test]
    fn default_when_nothing_set() {
        assert_eq!(pick_server_base(None, None), session::DEFAULT_LOCAL_VOX);
        // Blank values don't shadow lower-precedence sources.
        assert_eq!(
            pick_server_base(Some(""), Some(" ")),
            session::DEFAULT_LOCAL_VOX
        );
    }

    #[test]
    fn org_url_appends_per_org_path() {
        assert_eq!(
            org_vox_url(
                &pick_server_base(Some("wss://task.starcommand.live/vox"), None),
                "codywright"
            ),
            "wss://task.starcommand.live/org/codywright/vox"
        );
        assert_eq!(
            slug_of("wss://task.starcommand.live/org/codywright/vox"),
            Some("codywright")
        );
        assert_eq!(slug_of("ws://127.0.0.1:18080/server/vox"), None);
    }

    #[test]
    fn ws_http_derivation() {
        assert_eq!(
            http_base_of("wss://task.starcommand.live"),
            "https://task.starcommand.live"
        );
        assert_eq!(
            http_base_of("ws://127.0.0.1:18080"),
            "http://127.0.0.1:18080"
        );
    }

    #[test]
    fn server_vox_never_doubles_the_suffix() {
        // The per-org hint shape `TASK_VOX_URL` often holds.
        assert_eq!(
            normalize_server_vox("ws://127.0.0.1:18080/vox"),
            "ws://127.0.0.1:18080/server/vox"
        );
        assert_eq!(
            normalize_server_vox("https://task.starcommand.live"),
            "wss://task.starcommand.live/server/vox"
        );
        // Already correct: left alone.
        assert_eq!(
            normalize_server_vox("ws://h/server/vox"),
            "ws://h/server/vox"
        );
        // Scheme-less input gets the insecure default, matching
        // `normalize_server_base`.
        assert_eq!(normalize_server_vox("h:9"), "ws://h:9/server/vox");
    }
}
