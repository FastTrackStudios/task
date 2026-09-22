#![allow(clippy::large_futures)]
//! The identity presented at WebSocket **upgrade** reaches the permission
//! gate, on every call of that connection.
//!
//! This is the regression test for the bug in issue #109: the permission
//! gate was computing the right answer on every RPC and throwing it away,
//! because nothing ever attached the session token to a vox call. Every
//! request — including a signed-in owner's — arrived as
//! `principal=anonymous`, so flipping `TASK_ENFORCE_PERMISSIONS=1` would
//! have denied 100% of traffic rather than just the anonymous share.
//!
//! What makes this checkable without a full enforcement flip is that the
//! gate still runs in observe-only mode and records what it WOULD have
//! refused ([`permission_deny_ledger`]). So the assertion is symmetric and
//! needs no enforcement:
//!
//! - anonymous connection → the ledger records a would-deny;
//! - connection carrying a valid session → the ledger stays empty.
//!
//! Both bearer channels are exercised, because the two client families
//! can do different things: browsers can set no request headers (and the
//! token must not ride the URL, where proxy logs would capture it), so
//! they present it as a `vox.bearer.<token>` **subprotocol**; native
//! clients use a plain `Authorization: Bearer` header.

use architect_auth::CreateEmailPasswordUser;
use task_server::{AppState, AuthState, capability::ServerKeypair, permission_deny_ledger, router};
use vault_proto::VaultSyncClient;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Boot a sandboxed server. Returns `(ws url, auth state, data root)` —
/// the `TempDir` must outlive the test or the org vanishes underneath it.
async fn boot() -> eyre::Result<(String, AuthState, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        // Observe-only is the mode under test: the gate must evaluate and
        // record, and refuse nothing.
        std::env::remove_var("TASK_ENFORCE_PERMISSIONS");
    }
    org_proto::DataRoot::from_env()
        .map_err(|e| eyre::eyre!("data root: {e}"))?
        .init_org("identity-test", "Identity Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth.clone(), ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    Ok((format!("ws://127.0.0.1:{port}/vox"), auth, tmp))
}

/// How a client presents itself at the handshake.
enum Present {
    /// No subprotocol at all — every client built before this change
    /// (the deployed web bundle, iOS, the watch bridge). Deploy safety
    /// depends on these still connecting.
    Legacy,
    /// The plain subprotocol and no token: a current client, signed out.
    Anonymous,
    /// `Sec-WebSocket-Protocol: vox.v1, vox.bearer.<token>` — the browser
    /// channel (the only client-controlled field on a browser handshake).
    Subprotocol(String),
    /// `Authorization: Bearer <token>` — the native channel.
    Header(String),
}

/// Dial `url` presenting `how`, and establish a `VaultSyncClient` on it.
/// Mirrors `task_ui_core::vox_clients::dial_ws_native`.
async fn establish(url: &str, how: Present) -> eyre::Result<VaultSyncClient> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let mut request = url.into_client_request()?;
    let headers = request.headers_mut();
    match how {
        Present::Legacy => {}
        Present::Anonymous => {
            headers.insert("sec-websocket-protocol", "vox.v1".parse()?);
        }
        Present::Subprotocol(token) => {
            headers.insert(
                "sec-websocket-protocol",
                format!("vox.v1, vox.bearer.{token}").parse()?,
            );
        }
        Present::Header(token) => {
            headers.insert("sec-websocket-protocol", "vox.v1".parse()?);
            headers.insert("authorization", format!("Bearer {token}").parse()?);
        }
    }
    let (stream, _response) = tokio_tungstenite::connect_async(request).await?;
    vox_core::initiator_on(vox_websocket::WsLink::new(stream))
        .establish::<VaultSyncClient>()
        .await
        .map_err(|e| eyre::eyre!("establish: {e:?}"))
}

/// The gate's would-deny ledger as `(total occurrences, reasons)`.
///
/// **Occurrences, not distinct reasons**: the ledger keys by reason string
/// and counts repeats, so "did this call get denied" can only be answered
/// by the total. Comparing the reason list would silently pass whenever a
/// call reproduced a reason already on the record — which is exactly the
/// case the interesting assertion below is about.
fn would_denies() -> (u64, String) {
    let (denials, _overflow) = permission_deny_ledger().snapshot();
    let total = denials.iter().map(|d| d.count).sum();
    let reasons = denials
        .iter()
        .map(|d| format!("{}x {}", d.count, d.reason))
        .collect::<Vec<_>>()
        .join("\n");
    (total, reasons)
}

#[tokio::test(flavor = "multi_thread")]
async fn upgrade_identity_reaches_the_gate() -> eyre::Result<()> {
    let (url, auth, _data_root) = boot().await?;

    let bundle = auth
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "owner@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Owner".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?;
    let token = bundle.token;

    // ── 1. Anonymous: the gate would refuse (and, being observe-only,
    //       answers anyway — the production behaviour today).
    let anon = establish(&url, Present::Anonymous).await?;
    anon.manifest("default".to_string())
        .await
        .map_err(|e| eyre::eyre!("observe-only must not refuse: {e:?}"))?;
    // Two ledger records per refused call: the decision itself, and the
    // observe-only marker that carries `(service/method)` — see
    // `permits::GateAudit`.
    const PER_DENIED_CALL: u64 = 2;
    let (anon_total, reasons) = would_denies();
    assert_eq!(
        anon_total, PER_DENIED_CALL,
        "an anonymous call should record exactly one would-deny; \
         ledger was:\n{reasons}",
    );
    assert!(
        reasons.contains("vault-sync/manifest") && reasons.contains("anonymous"),
        "the would-deny should name the method and the principal; \
         ledger was:\n{reasons}",
    );
    drop(anon);

    // ── 2. Signed in over each channel: the SAME call, with the identity
    //       presented at the handshake, must produce NO would-deny at all.
    //       A total that doesn't move is the assertion — it fails both if
    //       the caller is still anonymous and if they authenticated but
    //       came out without the permission.
    let mut expected = anon_total;
    for (label, how) in [
        ("subprotocol", Present::Subprotocol(token.clone())),
        ("authorization header", Present::Header(token.clone())),
    ] {
        let client = establish(&url, how).await?;
        client
            .manifest("default".to_string())
            .await
            .map_err(|e| eyre::eyre!("{label}: manifest failed: {e:?}"))?;
        let (total, reasons) = would_denies();
        assert_eq!(
            total, expected,
            "a call on a connection authenticated by {label} was still \
             not permitted — the token did not reach the gate. \
             Ledger:\n{reasons}",
        );
    }

    // ── 3. A garbage token must fall BACK to anonymous, never through.
    //       "No token" and "token rejected" both resolve to
    //       `Principal::Anonymous` — the distinction lives in the
    //       `auth.outcome` wide field, not in the principal.
    // ── 4. A client from before this change must still connect. It
    //       offers no subprotocol, so the server must not echo one:
    //       tungstenite fails the handshake on an unrequested echo
    //       (`ServerSentSubProtocolNoneRequested`), which is what a
    //       rollout to the deployed iOS / watch / web clients would hit.
    let legacy = establish(&url, Present::Legacy)
        .await
        .map_err(|e| eyre::eyre!("a pre-change client can no longer connect: {e:?}"))?;
    legacy
        .manifest("default".to_string())
        .await
        .map_err(|e| eyre::eyre!("legacy client: manifest failed: {e:?}"))?;
    expected += PER_DENIED_CALL; // still anonymous, so still a would-deny

    let bogus = establish(&url, Present::Subprotocol("not-a-real-token".into())).await?;
    bogus
        .manifest("default".to_string())
        .await
        .map_err(|e| eyre::eyre!("observe-only must not refuse: {e:?}"))?;
    expected += PER_DENIED_CALL;
    let (total, reasons) = would_denies();
    assert_eq!(
        total, expected,
        "a rejected token must fall back to anonymous and be recorded as \
         a would-deny, not pass the gate. Ledger:\n{reasons}",
    );

    Ok(())
}
