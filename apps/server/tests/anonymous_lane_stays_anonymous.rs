#![allow(clippy::large_futures)]
//! A lane dialled without a bearer is anonymous **for its whole life**.
//!
//! This is the server-side half of the sign-in race the web app used to
//! lose. Coming back from the central issuer is a fresh page load, and
//! the app used to mount the workspace the instant the authorization
//! code had been handed to the auth service — before the redeemed token
//! was the app's identity. Every org lane the workspace dialled in that
//! window therefore presented no bearer, and the first thing anyone saw
//! after signing in was
//!
//! ```text
//! Couldn't load your workspace
//! tombrooksmusic: list: InvalidPayload("permission denied: anonymous is not a member (project/list)")
//! ```
//!
//! A plain reload fixed it, which is the signature of an ordering bug
//! rather than a permissions one: by the second load the token was in
//! storage before anything dialled.
//!
//! What this test pins is *why the client fix had to be an ordering fix*
//! and could not be a retry. The identity is read ONCE, from the
//! WebSocket upgrade (`task_server::serve_org_vox`), so:
//!
//! - an anonymous lane is refused, and
//! - it is refused **again** on the very same lane after a valid token
//!   exists — the connection cannot learn about it, because nothing
//!   about that connection is ever re-read, and
//! - only a NEW lane, dialled presenting the token, is a member.
//!
//! So the client's only cures are "don't dial until the token is the
//! identity" (what `pages::auth_callback` + the `/auth/callback` route
//! now guarantee) or "throw the lane away and re-dial"
//! (`vox_clients::drop_cached_connections`, on every token change). A
//! retry loop over the failed call is neither, and would sit there
//! failing forever.
//!
//! Enforcement is ON here on purpose: production runs with
//! `TASK_ENFORCE_PERMISSIONS=1`, which is why the anonymous call came
//! back as an error the UI could render rather than as a silent
//! would-deny in the ledger (`connection_identity` covers the
//! observe-only side).

use architect_auth::CreateEmailPasswordUser;
use project::ProjectServiceClient;
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

const ORG: &str = "lane-identity-test";

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Boot a sandboxed, ENFORCING server with one org. Returns the org
/// lane's URL, the auth state to mint a session from, and the data root
/// (which must outlive the test or the org vanishes underneath it).
async fn boot() -> eyre::Result<(String, AuthState, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
    }
    org_proto::DataRoot::from_env()
        .map_err(|e| eyre::eyre!("data root: {e}"))?
        .init_org(ORG, "Lane Identity Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth.clone(), ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    Ok((format!("ws://127.0.0.1:{port}/org/{ORG}/vox"), auth, tmp))
}

/// Dial the org lane, optionally presenting `token` as the browser does
/// — `Sec-WebSocket-Protocol: vox.v1, vox.bearer.<token>`. Mirrors
/// `task_ui_core::vox_clients::dial_ws`; the token deliberately never
/// rides the URL, where proxy logs would capture it.
async fn dial(url: &str, token: Option<&str>) -> eyre::Result<ProjectServiceClient> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let mut request = url.into_client_request()?;
    let offered = match token {
        Some(token) => format!("vox.v1, vox.bearer.{token}"),
        None => "vox.v1".to_owned(),
    };
    request
        .headers_mut()
        .insert("sec-websocket-protocol", offered.parse()?);
    let (stream, _response) = tokio_tungstenite::connect_async(request).await?;
    vox_core::initiator_on(vox_websocket::WsLink::new(stream))
        .establish::<ProjectServiceClient>()
        .await
        .map_err(|e| eyre::eyre!("establish: {e:?}"))
}

#[tokio::test(flavor = "multi_thread")]
async fn an_anonymous_lane_never_becomes_a_member() -> eyre::Result<()> {
    let (url, auth, _data_root) = boot().await?;

    // 1. The lane the racing web app dialled: opened before anybody had
    //    signed in, so it offered no bearer.
    let anonymous = dial(&url, None).await?;
    let refused = anonymous.list().await;
    let message = format!("{refused:?}");
    assert!(
        refused.is_err() && message.contains("permission denied") && message.contains("anonymous"),
        "an anonymous org lane must be refused by name, got: {message}"
    );

    // 2. A session now exists — exactly the state the app reaches a few
    //    hundred milliseconds later, when the issuer's token has been
    //    redeemed and adopted.
    let bundle = auth
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "member@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Member".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?;
    let token = bundle.token;

    // 3. THE POINT. Calling again on the SAME lane is refused just the
    //    same. The identity was read at the upgrade and there is no
    //    later moment at which it is re-read, so a retry — however
    //    patient, however many times — cannot ever succeed. This is why
    //    the client fix is an ordering fix.
    let still_refused = anonymous.list().await;
    let message = format!("{still_refused:?}");
    assert!(
        still_refused.is_err() && message.contains("anonymous"),
        "a lane dialled anonymously must stay anonymous even once a valid \
         session exists — otherwise retrying the failed call would be a \
         legitimate fix, and it is not. Got: {message}"
    );

    // 4. A lane dialled AFTER the token is the identity is a member, and
    //    the same call answers. That is what the app now guarantees by
    //    mounting nothing that talks to an org until `AuthCtx::active`
    //    has resolved.
    let signed_in = dial(&url, Some(&token)).await?;
    signed_in
        .list()
        .await
        .map_err(|e| eyre::eyre!("a lane carrying a valid session must be a member: {e:?}"))?;

    Ok(())
}
