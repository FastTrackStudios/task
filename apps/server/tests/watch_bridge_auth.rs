#![allow(clippy::large_futures)]
//! Watch bridge auth — the `/org/{slug}/watch/v1/*` bearer rule.
//!
//! The watch inherits the phone's REAL session token over the
//! WatchConnectivity config bridge (`ui::watch_sync` → `PhoneSync.swift`),
//! so the bridge must accept an architect-auth session token exactly like
//! it accepts the static `TASK_WATCH_TOKEN` device token. This pins all
//! three paths against a live server:
//!
//! 1. a valid session token (issued by the org's auth engine) → 200;
//! 2. a garbage token → 401;
//! 3. the static `TASK_WATCH_TOKEN` → 200 (deployed watch builds).

use architect_auth::CreateEmailPasswordUser;
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const SLUG: &str = "watch-test";
const DEVICE_TOKEN: &str = "watch-bridge-test-device-token";

/// Boot a sandboxed server (temp `TASK_DATA_ROOT`, in-memory auth DB)
/// with the watch device token configured. Returns the HTTP base and
/// the auth state (for seeding users); the `TempDir` keeps the data
/// root alive for the duration of the test.
async fn boot_server() -> eyre::Result<(String, AuthState, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let keypair = ServerKeypair::generate_ephemeral();

    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_WATCH_TOKEN", DEVICE_TOKEN);
    }
    org_proto::DataRoot::from_env()
        .map_err(|e| eyre::eyre!("data root: {e}"))?
        .init_org(SLUG, "Watch Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth.clone(), keypair).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), auth, tmp))
}

#[tokio::test(flavor = "multi_thread")]
async fn watch_bridge_accepts_session_and_device_tokens() -> eyre::Result<()> {
    let (base, auth_state, _data_root) = boot_server().await?;

    // Seed a user — creation returns a live session bundle (sign-up
    // logs you in), whose token is exactly what the phone forwards.
    let bundle = auth_state
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "watch@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Watch User".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?;
    let session_token = bundle.token;

    let http = reqwest::Client::new();
    let url = format!("{base}/org/{SLUG}/watch/v1/timer/active");

    // 1. The phone-inherited session token authenticates.
    let res = http.get(&url).bearer_auth(&session_token).send().await?;
    assert_eq!(
        res.status(),
        reqwest::StatusCode::OK,
        "session token should pass: {}",
        res.text().await.unwrap_or_default()
    );

    // 2. A bogus token is rejected.
    let res = http
        .get(&url)
        .bearer_auth("not-a-real-token")
        .send()
        .await?;
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 3. No token at all is rejected.
    let res = http.get(&url).send().await?;
    assert_eq!(res.status(), reqwest::StatusCode::UNAUTHORIZED);

    // 4. The static device token still works (deployed watch builds).
    let res = http.get(&url).bearer_auth(DEVICE_TOKEN).send().await?;
    assert_eq!(
        res.status(),
        reqwest::StatusCode::OK,
        "device token should pass: {}",
        res.text().await.unwrap_or_default()
    );

    Ok(())
}
