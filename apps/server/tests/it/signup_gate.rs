#![allow(clippy::large_futures)]
//! Anonymous self-registration must be refused when the gate enforces.
//!
//! architect-auth has no email/password signup toggle:
//! `email_password_enabled` gates sign-IN as well, and `disable_signup` /
//! `signup_enabled` are OneTap- and SIWE-specific. So open registration
//! was on and reachable by anyone — and because the org lane hands every
//! validated user the `member` role (`DEFAULT_ORG_ROLE`), that made
//! permission enforcement bypassable in ONE call: sign up, become a
//! member, read the org.
//!
//! Verified reachable on production 2026-08-08 with a credential-less
//! CLI; the only complaint was password length.
//!
//! The fix is a permit, not a feature flag: `sign_up_email_password`
//! points at `auth/signup` instead of `public/**`, so the gate refuses
//! anonymous callers while sign-in stays public.

use task_server::{AppState, AuthState, capability::ServerKeypair, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn boot(enforce: bool) -> eyre::Result<(String, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        if enforce {
            std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
        } else {
            std::env::remove_var("TASK_ENFORCE_PERMISSIONS");
        }
    }
    org_proto::DataRoot::from_env()
        .map_err(|e| eyre::eyre!("data root: {e}"))?
        .init_org("signup-test", "Signup Test", true)
        .map_err(|e| eyre::eyre!("scaffold: {e}"))?;
    let state = AppState::new_with_auth(auth, ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router(state)).await;
    });
    Ok((format!("ws://127.0.0.1:{port}/vox"), tmp))
}

#[tokio::test(flavor = "multi_thread")]
async fn anonymous_signup_is_refused_when_enforcing() -> eyre::Result<()> {
    let (url, _tmp) = boot(true).await?;
    let client: architect_auth::proto::AuthServiceClient = vox::connect_lane(&url)
        .establish()
        .await
        .map_err(|e| eyre::eyre!("connect: {e:?}"))?;

    let attempt = client
        .sign_up_email_password(architect_auth::SignUpEmailPassword {
            email: "intruder@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Intruder".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await;
    assert!(
        attempt.is_err(),
        "an anonymous caller must not be able to self-register into an org, got: {attempt:?}"
    );

    // Sign-in stays public, or nobody could ever authenticate.
    let signin = client
        .sign_in_email_password(architect_auth::SignInEmailPassword {
            email: "nobody@example.test".into(),
            password: "whatever-it-does-not-matter".into(),
            ip_address: None,
            user_agent: None,
        })
        .await;
    let msg = format!("{signin:?}");
    assert!(
        !msg.contains("permission denied"),
        "sign-in must remain reachable for anonymous callers, got: {msg}"
    );

    Ok(())
}
