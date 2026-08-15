#![allow(clippy::large_futures)]
//! Live sanity check — the real sign-in flow the web account
//! switcher performs, against the locally running dev server.
//!
//! Mirrors `crates/ui/src/auth.rs::resolve_session`: establish an
//! `AuthServiceClient` over the home org's vox endpoint
//! (`/org/<slug>/vox`, where `AuthServerMiddleware` wraps the
//! mounted `AuthService`), `sign_in_email_password` with the seeded
//! dev credentials, then `whoami` with the issued token.
//!
//! The dev server isn't part of the test environment, so an
//! unreachable endpoint SKIPS (prints + passes) instead of failing —
//! the in-process variant of this round-trip is covered by
//! `tests/auth.rs`. Override the target with `TASK_LIVE_VOX_URL`.

use architect_auth::SignInEmailPassword;

const DEFAULT_LIVE_URL: &str = "ws://127.0.0.1:18080/org/codywright/vox";
const DEV_EMAIL: &str = "cody@fasttrackstudios.com";
const DEV_PASSWORD: &str = "dev-cody-2026";

#[tokio::test(flavor = "multi_thread")]
async fn live_sign_in_and_whoami_for_cody() -> eyre::Result<()> {
    let url = std::env::var("TASK_LIVE_VOX_URL").unwrap_or_else(|_| DEFAULT_LIVE_URL.to_owned());

    let client: architect_auth::proto::AuthServiceClient =
        match vox::connect_lane(&url).establish().await {
            Ok(client) => client,
            Err(e) => {
                eprintln!("SKIP: dev server not reachable at {url} ({e:?})");
                return Ok(());
            }
        };

    // Real sign-in with the seeded dev credentials — the same call
    // the web switcher makes (`crates/ui/src/auth.rs`).
    let bundle = client
        .sign_in_email_password(SignInEmailPassword {
            email: DEV_EMAIL.into(),
            password: DEV_PASSWORD.into(),
            ip_address: None,
            user_agent: Some("auth-live-test".into()),
        })
        .await
        .map_err(|e| eyre::eyre!("sign_in_email_password({DEV_EMAIL}): {e:?}"))?;
    assert!(!bundle.token.is_empty(), "sign-in must issue a token");
    assert_eq!(bundle.user.email.as_deref(), Some(DEV_EMAIL));

    // whoami with the fresh token — the switcher's cached-token
    // validation path.
    let me = client
        .whoami(bundle.token.clone())
        .await
        .map_err(|e| eyre::eyre!("whoami: {e:?}"))?;
    assert_eq!(me.id, bundle.user.id);
    assert_eq!(me.email.as_deref(), Some(DEV_EMAIL));

    // Deliberately NOT signing out: the switcher keeps tokens cached
    // for instant switch-back, and a dev browser may hold this one.
    Ok(())
}
