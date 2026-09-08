#![allow(clippy::large_futures)]
//! Three ways to reach no organization, and three different answers.
//!
//! `reachable_orgs` answers with a list, and an empty list has more than
//! one cause. The MCP lane used to collapse them into one message —
//! "no reachable org — pass `org`, or check `list_orgs`" — which is
//! sound advice for exactly one of them and misdirection for the other
//! two.
//!
//! It matters because of who reads it. A client holding an expired token
//! was told its *account* had no workspace, which sends someone looking
//! for a provisioning bug when the fix is to sign in again. That is not
//! hypothetical: a Keyflow client sent a bogus bearer while this lane was
//! being wired up and reported back that the account had no workspace.
//!
//! Its own binary: the harness sets `TASK_DATA_ROOT` and
//! `TASK_CENTRAL_AUTH_URL` as process env vars, and
//! `central_auth::configured()` reads the issuer once per process.

use serde_json::{Value, json};
use task_server::AppState;

/// A token the issuer vouches for, belonging to nobody on this server.
const VOUCHED: &str = "an-access-token-the-issuer-knows";

async fn refusal(client: &reqwest::Client, url: &str, token: Option<&str>) -> String {
    let mut req = client.post(url).json(&json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {},
    }));
    if let Some(t) = token {
        req = req.bearer_auth(t);
    }
    let body: Value = req
        .send()
        .await
        .expect("POST /mcp")
        .json()
        .await
        .expect("json-rpc body");
    assert!(
        body["result"].is_null(),
        "expected a refusal, got a result: {body}"
    );
    body["error"]["message"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn the_three_ways_to_reach_no_org_say_three_different_things() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        // Nobody listens here. The one answer this test needs is seeded
        // into the resolver's cache below.
        std::env::set_var("TASK_CENTRAL_AUTH_URL", "http://127.0.0.1:9");
        for var in [
            "TASK_SERVER_ORG",
            "TASK_SERVER_VAULT_ROOT",
            "TASK_MCP_TOKEN",
        ] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::from_env().expect("data root");
    data_root
        .init_org("mine", "Mine", true)
        .expect("scaffold the home org");
    // Present and empty: the account below is a member of nothing, which
    // is the state under test rather than a broken fixture.
    task_server::memberships::Memberships::open(&data_root.org("mine").memberships_db())
        .await
        .expect("open memberships");

    let state = AppState::new(None).await.expect("boot");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let url = format!("http://127.0.0.1:{port}/mcp");
    let client = reqwest::Client::new();

    task_server::central_auth::configured()
        .expect("the issuer is configured for this binary")
        .remember_profile_for_test(
            VOUCHED,
            &uuid::Uuid::new_v4().to_string(),
            Some("nobody@example.test"),
        );

    // ── No credential at all. Nothing to diagnose: send one.
    let anon = refusal(&client, &url, None).await;
    assert!(
        anon.contains("no bearer token"),
        "an anonymous caller should be told to send a token, got: {anon}"
    );

    // ── A token nobody can place. The fix is a new session, and saying
    //    anything about organizations here sends the reader to the wrong
    //    place entirely.
    let dead = refusal(&client, &url, Some("expired-or-invented")).await;
    assert!(
        dead.contains("not recognised"),
        "an unknown token should be named as such, got: {dead}"
    );
    assert!(
        !dead.contains("organization"),
        "an unknown token must not be reported as an org problem, got: {dead}"
    );

    // ── A real account that belongs to nothing. Only here is
    //    `list_orgs` useful advice, and only here does asking for a
    //    personal org make sense.
    let homeless = refusal(&client, &url, Some(VOUCHED)).await;
    assert!(
        homeless.contains("does not belong to any organization"),
        "a vouched account with no memberships should be told so, got: {homeless}"
    );

    // ── And the three are actually distinct, which is the whole point.
    assert_ne!(anon, dead);
    assert_ne!(dead, homeless);
    assert_ne!(anon, homeless);
}
