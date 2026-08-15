#![allow(clippy::large_futures)]
//! Phase 2 integration test — architect-auth wired over vox.
//!
//! Plan (`plans/decentralized-foundation.md` §13 Phase 2):
//! create a user, sign in over vox, get a session token, call
//! `current_session` with that token, assert identity round-trips.
//!
//! Sign-up is NOT exposed over vox in upstream architect-auth's
//! `AuthService` trait — only sign-in / current-session / sign-out
//! are. So the user is created server-side via direct
//! `ArchitectAuth::create_email_password_user`, then the vox path
//! exercises sign-in + current-session.

use architect_auth::{CreateEmailPasswordUser, SignInEmailPassword};
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Returns the `TempDir` so the caller keeps the data root alive for the
/// duration of the test — dropping it early would delete the org out from
/// under the running server.
async fn boot_server() -> eyre::Result<(String, AuthState, tempfile::TempDir)> {
    // In-memory auth DB so tests don't touch the user's XDG dir.
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let keypair = ServerKeypair::generate_ephemeral();

    // `AppState::new_with_auth` resolves its org through
    // `DataRoot::from_env()`, which falls back to `~/.task` when
    // `TASK_DATA_ROOT` is unset. Without this, the test ran against the
    // developer's real Task data — passing only because that data root
    // happened to contain an org, and touching it as a side effect. Every
    // other suite in this directory already sandboxes this way.
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
    }
    org_proto::DataRoot::from_env()
        .map_err(|e| eyre::eyre!("data root: {e}"))?
        .init_org("auth-test", "Auth Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth.clone(), keypair).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("ws://127.0.0.1:{port}/vox"), auth, tmp))
}

#[tokio::test(flavor = "multi_thread")]
async fn signup_signin_current_session_roundtrip() -> eyre::Result<()> {
    let (url, auth_state, _data_root) = boot_server().await?;

    // Seed the user server-side. Open signup over vox is a later
    // phase — Phase 2 just proves the auth pipeline is live.
    let bundle = auth_state
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "alice@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Alice".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?;
    let alice_user_id = bundle.user.id;

    // Sign in over vox.
    let client: architect_auth::proto::AuthServiceClient = vox::connect_lane(&url)
        .establish()
        .await
        .map_err(|e| eyre::eyre!("connect: {e:?}"))?;
    let sign_in = client
        .sign_in_email_password(SignInEmailPassword {
            email: "alice@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("sign_in_email_password: {e:?}"))?;
    assert_eq!(
        sign_in.user.id, alice_user_id,
        "sign-in should return the same user we seeded"
    );
    assert!(
        !sign_in.token.is_empty(),
        "sign-in must yield a non-empty token"
    );

    // Use the token to fetch the current session over vox.
    let current = client
        .current_session(sign_in.token.clone())
        .await
        .map_err(|e| eyre::eyre!("current_session: {e:?}"))?;
    assert_eq!(current.user.id, alice_user_id);
    assert_eq!(current.token, sign_in.token);

    // Wrong token must fail.
    let bad = client.current_session("not-a-real-token".into()).await;
    assert!(bad.is_err(), "bogus token must not resolve to a session");

    Ok(())
}

/// `list_org_members` must not answer callers without a session.
///
/// `AuthService` is deliberately PUBLIC on the org lane — the permission
/// gate can't cover it or signing in would be impossible (see
/// `permits.rs`). That is fine for methods which take the session token as
/// an argument and validate it themselves, which was the stated premise
/// for making the whole service public.
///
/// `list_org_members` broke that premise: on an absent or foreign token it
/// fell through to enumerating every user in the org store. Net effect on
/// a networked server: anyone could read every user's name, email and id,
/// for every org, WITH permission enforcement on. Verified against
/// production 2026-08-08 using a CLI with no credentials, which is how it
/// was found.
#[tokio::test(flavor = "multi_thread")]
async fn member_listing_requires_a_session() -> eyre::Result<()> {
    let (url, auth_state, _data_root) = boot_server().await?;

    auth_state
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "bob@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Bob".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?;

    let client: architect_auth::proto::AuthServiceClient = vox::connect_lane(&url)
        .establish()
        .await
        .map_err(|e| eyre::eyre!("connect: {e:?}"))?;

    // No token at all — the anonymous case that was leaking.
    let anon = client.list_org_members(String::new()).await;
    assert!(
        anon.is_err(),
        "an anonymous caller must not be able to enumerate org users, got: {anon:?}"
    );

    // A token that doesn't validate here is the same as none. On a
    // multi-org server this is also another org's token: each org has its
    // own auth store, so a foreign session must not read these members.
    let foreign = client.list_org_members("not-a-real-token".into()).await;
    assert!(
        foreign.is_err(),
        "a foreign/invalid token must not enumerate org users, got: {foreign:?}"
    );

    // A real session still works — the fallback for orgs that keep no
    // membership rows is preserved, just put behind a session.
    let signed_in = client
        .sign_in_email_password(SignInEmailPassword {
            email: "bob@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("sign in: {e:?}"))?;
    let members = client
        .list_org_members(signed_in.token)
        .await
        .map_err(|e| eyre::eyre!("a validated session must still list members: {e:?}"))?;
    assert!(
        members.iter().any(|m| m.email == "bob@example.test"),
        "expected the seeded user in the listing, got: {members:?}"
    );

    Ok(())
}
