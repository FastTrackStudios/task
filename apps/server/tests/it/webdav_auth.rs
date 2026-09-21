#![allow(clippy::large_futures)]
//! `/org/{slug}/dav` authorization — acceptance criterion 4 of issue
//! #274, "auth respects existing tokens and root permissions", proved
//! end to end over real HTTP against the mounted router.
//!
//! The bridge's own protocol behaviour is tested at the `files-webdav`
//! seam (`features/task/files/files-webdav/tests/webdav_surface.rs`);
//! what only this level can prove is the half that lives in the server:
//! that a WebDAV client reaches an org's roots **only** with a
//! credential Task already issues, and that the refusal carries the
//! `WWW-Authenticate` challenge an OS mount dialog needs.
//!
//! Unlike `/media`, this route has no observe-only phase — it is new,
//! so it has no deployed clients to black out, and an open WebDAV mount
//! would expose an org's whole project tree. It enforces from boot,
//! which is what the "no token" cases below assert.

use architect_auth::{CreateEmailPasswordUser, SignInEmailPassword};
use base64::Engine as _;
use files::{FilesService as _, RootFlavor};
use task_server::{AppState, AuthState, capability::ServerKeypair, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

const EMAIL: &str = "producer@example.test";
const PASSWORD: &str = "correct-horse-battery-staple";

/// A server with one org that already holds one File Root, so a
/// successful `PROPFIND` has something real to list.
async fn boot() -> eyre::Result<(String, AuthState, tempfile::TempDir)> {
    let auth = AuthState::open("sqlite::memory:", "test-secret-at-least-32-bytes!!!").await?;
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
    }
    org_proto::DataRoot::from_env()
        .map_err(|e| eyre::eyre!("data root: {e}"))?
        .init_org("dav-test", "Dav Test", true)
        .map_err(|e| eyre::eyre!("scaffold org: {e}"))?;
    let state = AppState::new_with_auth(auth.clone(), ServerKeypair::generate_ephemeral()).await?;
    drop(guard);

    // Stage a folder inside the org's files area and make it a root —
    // the same route the RPC surface takes.
    let org = state.org("dav-test").expect("org hosted");
    let root_dir = org.files.data_dir().join("Mix Session");
    std::fs::create_dir_all(&root_dir)?;
    std::fs::write(root_dir.join("mix.wav"), b"take one")?;
    org.files
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Mix Session".into(),
            RootFlavor::Media,
        )
        .await
        .map_err(|e| eyre::eyre!("create_root: {e}"))?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), auth, tmp))
}

async fn seed_user(auth_state: &AuthState) -> eyre::Result<()> {
    auth_state
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: EMAIL.into(),
            password: PASSWORD.into(),
            name: Some("Producer".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?;
    Ok(())
}

async fn sign_in(auth_state: &AuthState) -> eyre::Result<String> {
    Ok(auth_state
        .auth
        .sign_in_email_password(SignInEmailPassword {
            email: EMAIL.into(),
            password: PASSWORD.into(),
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("sign in: {e:?}"))?
        .token)
}

fn basic_header(user: &str, secret: &str) -> String {
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("{user}:{secret}"))
    )
}

/// `PROPFIND Depth: 1` — what a file manager sends when it opens a
/// folder — with an optional `Authorization` header.
async fn propfind(
    url: &str,
    authorization: Option<&str>,
) -> (u16, reqwest::header::HeaderMap, String) {
    let mut req = reqwest::Client::new()
        .request(
            reqwest::Method::from_bytes(b"PROPFIND").expect("PROPFIND is a valid method"),
            url,
        )
        .header("Depth", "1");
    if let Some(value) = authorization {
        req = req.header(reqwest::header::AUTHORIZATION, value);
    }
    let res = req.send().await.expect("request");
    let status = res.status().as_u16();
    let headers = res.headers().clone();
    (status, headers, res.text().await.unwrap_or_default())
}

/// PR #287 review, finding 1 — the mount handshake, **through the
/// router**.
///
/// The `files-webdav` OPTIONS test calls `bridge.handle()` directly, so
/// it could not see that `router.layer(cors_layer())` wrapped these
/// routes: tower-http's `Cors` short-circuits *any* `OPTIONS` — no
/// `Origin` or `Access-Control-Request-Method` required — with a bare
/// 200 and never calls the inner service. Finder/Explorer/gvfs open a
/// mount with exactly that request and read `DAV:`/`Allow:` off the
/// reply, so every OS client refused to mount and `webdav_handler` was
/// never reached (nothing authenticated either). This test only passes
/// with the dav routes outside the CORS layer.
#[tokio::test(flavor = "multi_thread")]
async fn options_through_the_router_advertises_webdav() -> eyre::Result<()> {
    let (base, auth_state, _data_root) = boot().await?;
    seed_user(&auth_state).await?;
    let token = sign_in(&auth_state).await?;

    for url in [
        format!("{base}/org/dav-test/dav"),
        format!("{base}/org/dav-test/dav/"),
        format!("{base}/org/dav-test/dav/Mix%20Session/"),
    ] {
        let res = reqwest::Client::new()
            .request(reqwest::Method::OPTIONS, &url)
            .header(reqwest::header::AUTHORIZATION, format!("Bearer {token}"))
            .send()
            .await?;
        assert_eq!(res.status().as_u16(), 200, "OPTIONS {url}");
        let headers = res.headers().clone();
        let dav = headers
            .get("DAV")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        assert!(
            dav.contains('1') && dav.contains('2'),
            "OPTIONS {url} must advertise WebDAV classes 1 and 2, got {dav:?}"
        );
        let allow = headers
            .get(reqwest::header::ALLOW)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        for verb in ["PROPFIND", "LOCK", "UNLOCK"] {
            assert!(
                allow.contains(verb),
                "OPTIONS {url}: {verb} not in {allow:?}"
            );
        }
    }

    // And the route is still authenticated on OPTIONS — moving it out
    // from under CORS must not have moved it out from under auth.
    let res = reqwest::Client::new()
        .request(
            reqwest::Method::OPTIONS,
            format!("{base}/org/dav-test/dav/"),
        )
        .send()
        .await?;
    assert_eq!(
        res.status().as_u16(),
        401,
        "an unauthenticated OPTIONS must still be refused"
    );

    Ok(())
}

/// One test, sequenced against one server: the credential cases are
/// about the *same* mount, and booting a server per case would say
/// nothing extra while multiplying the env-var dance.
#[tokio::test(flavor = "multi_thread")]
async fn webdav_mount_requires_an_existing_task_credential() -> eyre::Result<()> {
    let (base, auth_state, _data_root) = boot().await?;
    let mount = format!("{base}/org/dav-test/dav/");
    let root = format!("{base}/org/dav-test/dav/Mix%20Session/");

    seed_user(&auth_state).await?;
    let token = sign_in(&auth_state).await?;

    // ── Anonymous: refused, and the refusal is a Basic challenge. A
    //    bare 401 without this header makes Finder fail the mount
    //    silently instead of prompting.
    let (status, headers, _) = propfind(&mount, None).await;
    assert_eq!(status, 401, "an anonymous WebDAV mount must be refused");
    let challenge = headers
        .get(reqwest::header::WWW_AUTHENTICATE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(
        challenge.starts_with("Basic "),
        "401 must carry a Basic challenge: {challenge:?}"
    );
    // Not even the root itself, and not with a garbage credential.
    let (status, _, _) = propfind(&root, None).await;
    assert_eq!(status, 401, "an anonymous root read must be refused");
    let (status, _, _) = propfind(&mount, Some("Bearer not-a-real-token")).await;
    assert_eq!(status, 401, "an invalid bearer must be refused");
    let (status, _, _) = propfind(&mount, Some(&basic_header(EMAIL, "wrong-password"))).await;
    assert_eq!(status, 401, "a wrong password must be refused");
    let (status, _, _) = propfind(&mount, Some(&basic_header(EMAIL, "not-a-real-token"))).await;
    assert_eq!(status, 401, "a bogus token-as-password must be refused");

    // ── Bearer session — native clients.
    let (status, _, body) = propfind(&mount, Some(&format!("Bearer {token}"))).await;
    assert_eq!(status, 207, "a valid session must reach the mount");
    assert!(body.contains("Mix%20Session"), "root not listed: {body}");

    // ── Basic with the session token as the password — what a user
    //    pastes into Finder's dialog without putting a password on disk.
    let (status, _, body) = propfind(&root, Some(&basic_header(EMAIL, &token))).await;
    assert_eq!(status, 207, "session-token-as-password must authenticate");
    assert!(body.contains("mix.wav"), "live tree not listed: {body}");

    // ── Basic with the real email + password — what a human types.
    let (status, _, body) = propfind(&root, Some(&basic_header(EMAIL, PASSWORD))).await;
    assert_eq!(status, 207, "email/password must authenticate");
    assert!(body.contains("mix.wav"), "live tree not listed: {body}");

    // ── …and a burst of them mints NO sessions at all. A Basic client
    //    re-presents its credential on every request and has nowhere to
    //    keep a token, so signing in per request would pile up live
    //    sessions (a left-up Finder mount is ~288/user/day, never
    //    signed out) and caching a minted session would let a rotated
    //    password keep working until the cache expired. The password is
    //    verified against the stored hash instead (PR #287 review).
    let sessions_before = auth_state
        .auth
        .list_sessions(architect_auth::ListSessions {
            session_token: token.clone(),
        })
        .await
        .map_err(|e| eyre::eyre!("list sessions: {e:?}"))?
        .len();
    for _ in 0..10 {
        let (status, _, _) = propfind(&root, Some(&basic_header(EMAIL, PASSWORD))).await;
        assert_eq!(status, 207);
    }
    let sessions_after = auth_state
        .auth
        .list_sessions(architect_auth::ListSessions {
            session_token: token.clone(),
        })
        .await
        .map_err(|e| eyre::eyre!("list sessions: {e:?}"))?
        .len();
    assert_eq!(
        sessions_after, sessions_before,
        "password-authenticated WebDAV requests must not mint sessions"
    );

    // ── A rotated password takes effect on the very next request —
    //    there is no window in which the old one still works.
    let user_id = auth_state
        .auth
        .find_user_by_email(EMAIL)
        .await
        .map_err(|e| eyre::eyre!("find user: {e:?}"))?
        .ok_or_else(|| eyre::eyre!("seeded user missing"))?
        .id;
    auth_state
        .auth
        .set_user_password_local_trusted(user_id, "a-completely-different-passphrase")
        .await
        .map_err(|e| eyre::eyre!("rotate password: {e:?}"))?;
    let (status, _, _) = propfind(&root, Some(&basic_header(EMAIL, PASSWORD))).await;
    assert_eq!(
        status, 401,
        "the old password must stop working immediately after rotation"
    );
    let (status, _, _) = propfind(
        &root,
        Some(&basic_header(EMAIL, "a-completely-different-passphrase")),
    )
    .await;
    assert_eq!(status, 207, "the new password must work immediately");

    // ── An org this server does not host answers exactly like a bad
    //    credential — a caller does not get to enumerate orgs.
    let (status, _, _) = propfind(
        &format!("{base}/org/no-such-org/dav/"),
        Some(&format!("Bearer {token}")),
    )
    .await;
    assert_eq!(status, 401, "an unhosted org must not be distinguishable");

    Ok(())
}
