//! Central auth against a real issuer.
//!
//! The unit tests in `central_auth` cover the cache; they cannot cover
//! the thing most likely to be wrong, which is whether this server and
//! the auth server agree on the shape of a session response. That is a
//! contract between two repositories, and the only way to know is to
//! ask a running one.
//!
//! **Opt-in.** Set `TASK_CENTRAL_AUTH_URL` to an issuer and these run;
//! without it they skip, because a test suite that needs a network
//! service to pass is a test suite people stop running. Locally:
//!
//! ```sh
//! podman run -d --rm --name fts-auth-pg \
//!   -e POSTGRES_PASSWORD=dev -e POSTGRES_DB=fts_auth \
//!   -p 5433:5432 docker.io/library/postgres:16-alpine
//!
//! cd ../fts/fts-auth && AUTH_SECRET=$(openssl rand -base64 48) \
//!   AUTH_DATABASE_URL=postgres://postgres:dev@localhost:5433/fts_auth \
//!   AUTH_BIND_ADDR=127.0.0.1:8099 AUTH_BASE_URL=http://localhost:8099 cargo run
//!
//! TASK_CENTRAL_AUTH_URL=http://127.0.0.1:8099 cargo test -p task-server --test central_auth_e2e
//! ```

use task_server::central_auth::CentralAuth;

/// The issuer under test, or `None` to skip.
fn issuer() -> Option<String> {
    let raw = std::env::var("TASK_CENTRAL_AUTH_URL").ok()?;
    let url = raw.trim().trim_end_matches('/').to_owned();
    (!url.is_empty()).then_some(url)
}

/// Make an account and return `(user_id, session_token)`.
///
/// A fresh address per run: the suite must not depend on what a previous
/// run left behind, and sign-up on an existing address is a conflict
/// rather than a second session.
async fn sign_up(base: &str) -> (String, String) {
    let email = format!(
        "task-central-{}@example.invalid",
        uuid::Uuid::new_v4().simple()
    );
    let body = serde_json::json!({
        "email": email,
        "password": "correct-horse-battery-staple-9271",
        "name": "Central Auth Test",
    });
    let res = reqwest::Client::new()
        .post(format!("{base}/auth/sign-up/email"))
        .json(&body)
        .send()
        .await
        .expect("sign up");
    assert!(
        res.status().is_success(),
        "sign-up failed: {}",
        res.status()
    );
    let v: serde_json::Value = res.json().await.expect("sign-up json");
    let user_id = v["user"]["id"].as_str().expect("user.id").to_owned();
    let token = v["token"].as_str().expect("token").to_owned();
    (user_id, token)
}

/// The contract: a token the issuer minted resolves to its user.
///
/// This is the assertion that would have caught a response-shape change
/// in the auth server — the field names are read here exactly as
/// `CentralAuth::introspect` reads them.
#[tokio::test]
async fn a_real_token_resolves_to_its_user() {
    let Some(base) = issuer() else {
        eprintln!("skipping: TASK_CENTRAL_AUTH_URL not set");
        return;
    };
    let (user_id, token) = sign_up(&base).await;

    let auth = CentralAuth::new(&base);
    assert_eq!(
        auth.user_for(&token).await.as_deref(),
        Some(user_id.as_str()),
        "the issuer minted this token and did not recognise it back"
    );
}

/// A token the issuer never minted is nobody — and the answer must be
/// "no", not an error that a caller could mistake for "cannot tell".
#[tokio::test]
async fn a_forged_token_resolves_to_nobody() {
    let Some(base) = issuer() else {
        eprintln!("skipping: TASK_CENTRAL_AUTH_URL not set");
        return;
    };
    let auth = CentralAuth::new(&base);
    assert!(
        auth.user_for("not-a-token-anyone-issued").await.is_none(),
        "a forged token was accepted"
    );
}

/// An unreachable issuer refuses; it never widens access.
///
/// The failure this guards is the tempting one: treating "I could not
/// ask" as "probably fine". An auth server being down must lock people
/// out, not let everybody in.
#[tokio::test]
async fn an_unreachable_issuer_refuses() {
    // Port 1 on loopback: nothing listens, and the connection is
    // refused immediately rather than hanging until the timeout.
    let auth = CentralAuth::new("http://127.0.0.1:1");
    assert!(auth.user_for("any-token-at-all").await.is_none());
}

/// The second answer comes from memory, not the network.
///
/// Identity resolves on every RPC, so without this the auth server sees
/// one request per call and becomes a hard dependency of every
/// keystroke. Proven by taking the issuer away: if the second lookup
/// still answers, it did not ask.
#[tokio::test]
async fn a_resolved_token_is_not_asked_about_twice() {
    let Some(base) = issuer() else {
        eprintln!("skipping: TASK_CENTRAL_AUTH_URL not set");
        return;
    };
    let (user_id, token) = sign_up(&base).await;

    let auth = CentralAuth::new(&base);
    assert_eq!(
        auth.user_for(&token).await.as_deref(),
        Some(user_id.as_str())
    );

    // Same cache, an issuer that cannot answer. Only a remembered answer
    // can satisfy this.
    let stale = CentralAuth::new("http://127.0.0.1:1");
    stale.remember_for_test(&token, Some(user_id.clone()));
    assert_eq!(
        stale.user_for(&token).await.as_deref(),
        Some(user_id.as_str()),
        "the cached answer was not used"
    );
}

/// Discovery advertises the issuer — the client's only way to learn it.
///
/// The client has to know where to sign in *before* it has a session,
/// and discovery is the one thing it fetches in that state. If this
/// field is missing the app silently signs in against the home org
/// instead, which succeeds locally and then fails everywhere else, so
/// it is worth an assertion rather than an assumption.
///
/// Both directions in one test: unset means self-hosted and the field
/// is null, which is the default every existing deployment is in.
#[tokio::test(flavor = "multi_thread")]
async fn discovery_says_where_accounts_come_from() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        for var in ["TASK_SERVER_ORG", "TASK_SERVER_VAULT_ROOT"] {
            std::env::remove_var(var);
        }
    }
    // Set before the first read: the issuer is resolved once per process.
    let expected = issuer();
    if let Some(url) = &expected {
        unsafe { std::env::set_var("TASK_CENTRAL_AUTH_URL", url) };
    } else {
        unsafe { std::env::remove_var("TASK_CENTRAL_AUTH_URL") };
    }

    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    data_root.init_org("alpha", "Alpha", true).unwrap();

    let state = task_server::AppState::new(None).await.expect("boot");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    let doc: serde_json::Value = reqwest::get(format!(
        "http://127.0.0.1:{port}/.well-known/task-server.json"
    ))
    .await
    .expect("discovery")
    .json()
    .await
    .expect("discovery json");

    match expected {
        Some(url) => assert_eq!(
            doc["central_auth"].as_str(),
            Some(url.as_str()),
            "discovery must name the issuer the client should sign in against"
        ),
        None => assert!(
            doc["central_auth"].is_null(),
            "a self-hosted server must advertise no issuer: {}",
            doc["central_auth"]
        ),
    }
}
