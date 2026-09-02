#![allow(clippy::large_futures)]
//! Discovery tags membership for a CENTRAL account too.
//!
//! `org_membership.rs` proves the well-known doc tags each org with
//! whether the caller's token validates there, or is linked there through
//! the home org's memberships table. Both halves read the token as a
//! session one of this server's auth stores minted.
//!
//! A person who signed in through the issuer holds a token this server
//! never minted. The org lane already resolves it (`CentralFallbackResolver`
//! asks the issuer, then the memberships table); discovery did not, so
//! the very first thing the iOS app asked after the redirect sign-in
//! landed — "which orgs am I in?" — came back `member: false` for every
//! org, and the client drew an empty world over a server that was
//! answering every RPC. This is that gap, pinned.
//!
//! Its own binary: `central_auth::configured()` reads
//! `TASK_CENTRAL_AUTH_URL` once per process, so a binary that also boots
//! servers without an issuer could not decide which one it is testing.
//! The issuer here is an unreachable address; the cache is seeded with the
//! token the test presents, exactly as `central_auth_e2e` does, so no
//! network is involved and "unreachable" is never what the test measures.

use task_server::{AppState, router};

/// The answer the issuer would give for our token.
const TOKEN: &str = "central-access-token-for-the-discovery-test";

async fn boot(
    prepare: impl AsyncFnOnce(org_proto::DataRoot),
) -> eyre::Result<(String, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    // SAFETY: this binary holds one test, so nothing else reads the env
    // while `AppState::new` does.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        // Nobody listens here. Every answer this test needs is seeded into
        // the resolver's cache below.
        std::env::set_var("TASK_CENTRAL_AUTH_URL", "http://127.0.0.1:9");
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org("mine", "Mine", true)
        .map_err(|e| eyre::eyre!("scaffold mine: {e}"))?;
    data_root
        .init_org("theirs", "Theirs", false)
        .map_err(|e| eyre::eyre!("scaffold theirs: {e}"))?;
    data_root
        .init_org("nobodys", "Nobody's", false)
        .map_err(|e| eyre::eyre!("scaffold nobodys: {e}"))?;
    prepare(data_root).await;
    let state = AppState::new(None).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), tmp))
}

fn member_of(doc: &serde_json::Value, slug: &str) -> serde_json::Value {
    doc["orgs"]
        .as_array()
        .expect("orgs array")
        .iter()
        .find(|o| o["slug"] == slug)
        .expect("org present")["member"]
        .clone()
}

/// A token the issuer vouches for is a member exactly where the
/// memberships table says, and nowhere else.
#[tokio::test(flavor = "multi_thread")]
async fn discovery_tags_a_central_principal_by_its_membership_rows() -> eyre::Result<()> {
    let principal = uuid::Uuid::new_v4();
    let (base, _tmp) = boot(async |data_root| {
        // The rows `admin adopt-principal --principal <issuer id>` writes:
        // keyed to the ISSUER's id, in the home org's store, and present
        // before boot because that is when the server opens it.
        let home = data_root.org("mine");
        let m = task_server::memberships::Memberships::open(&home.memberships_db())
            .await
            .expect("open memberships");
        m.upsert(principal, "mine", Some("owner"))
            .await
            .expect("mine row");
        m.upsert(principal, "theirs", Some("member"))
            .await
            .expect("theirs row");
    })
    .await?;

    // What the issuer would say about TOKEN, without asking it.
    task_server::central_auth::configured()
        .expect("the issuer is configured for this binary")
        .remember_for_test(TOKEN, Some(principal.to_string()));

    let doc: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/.well-known/task-server.json"))
        .bearer_auth(TOKEN)
        .send()
        .await?
        .json()
        .await?;

    assert_eq!(member_of(&doc, "mine"), true, "home org, owner row");
    assert_eq!(member_of(&doc, "theirs"), true, "another org, member row");
    assert_eq!(
        member_of(&doc, "nobodys"),
        false,
        "no row means not a member — tagged false, never null, so the client \
         can tell 'signed in elsewhere' from 'signed out'"
    );

    // And a token the issuer rejects is a member of nothing.
    task_server::central_auth::configured()
        .expect("configured")
        .remember_for_test("some-stale-token", None);
    let doc: serde_json::Value = reqwest::Client::new()
        .get(format!("{base}/.well-known/task-server.json"))
        .bearer_auth("some-stale-token")
        .send()
        .await?
        .json()
        .await?;
    for slug in ["mine", "theirs", "nobodys"] {
        assert_eq!(member_of(&doc, slug), false, "{slug}: rejected token");
    }
    Ok(())
}

/// The identity locker answers a CENTRAL account too.
///
/// `list_links` is the call the web app makes right after sign-in to
/// learn which orgs it can present a credential for. The locker validated
/// the token against the home org's own store and nothing else, so an
/// issuer-minted token — admitted by every org lane — came back
/// `invalid session token` here, and the switcher never grew past one org.
/// `central_auth::home_principal` is the shared answer; this pins it on
/// the lane that surfaced the gap.
#[tokio::test(flavor = "multi_thread")]
async fn the_locker_answers_a_central_principal() -> eyre::Result<()> {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

    let principal = uuid::Uuid::new_v4();
    let (base, _tmp) = boot(async |data_root| {
        let home = data_root.org("mine");
        let m = task_server::memberships::Memberships::open(&home.memberships_db())
            .await
            .expect("open memberships");
        m.upsert(principal, "theirs", Some("member"))
            .await
            .expect("theirs row");
    })
    .await?;
    let central = task_server::central_auth::configured().expect("configured");
    central.remember_for_test(TOKEN, Some(principal.to_string()));
    central.remember_for_test("unknown-to-everyone", None);

    let ws = format!("{}/server/vox", base.replacen("http", "ws", 1));
    let mut request = ws.into_client_request()?;
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "vox.v1".parse()?);
    let (stream, _) = tokio_tungstenite::connect_async(request).await?;
    let locker = vox_core::initiator_on(vox_websocket::WsLink::new(stream))
        .establish::<identity_proto::IdentityServiceClient>()
        .await
        .map_err(|e| eyre::eyre!("establish: {e:?}"))?;

    // An account the issuer vouches for, with a row here: a home-org
    // account as far as the locker is concerned — empty, but answered.
    let links = locker
        .list_links(TOKEN.to_owned())
        .await
        .map_err(|e| eyre::eyre!("locker refused the central token: {e:?}"))?;
    assert!(links.is_empty(), "a fresh principal has linked nothing yet");

    // One the issuer rejects is still nobody.
    let refused = locker.list_links("unknown-to-everyone".to_owned()).await;
    assert!(
        refused.is_err(),
        "a token nobody knows must not read the locker"
    );
    Ok(())
}

/// The MCP account lane admits a CENTRAL principal.
///
/// `POST /mcp` found a caller's orgs by asking each org's own auth store
/// whether the token validated there, plus the locker. A token the
/// issuer minted validates nowhere local, so the lane answered "no
/// reachable org for this token" to the very person who owns every org
/// on the server — and the telemetry tools, gated on `admin` in the
/// home org's own store, refused them a second time. Membership rows
/// are the fence the RPC lane uses; this pins that the MCP lane uses the
/// same one, and that an `owner` is an operator.
#[tokio::test(flavor = "multi_thread")]
async fn the_mcp_lane_admits_a_central_principal_and_its_owner_reads_telemetry() -> eyre::Result<()>
{
    let principal = uuid::Uuid::new_v4();
    let (base, _tmp) = boot(async |data_root| {
        let home = data_root.org("mine");
        let m = task_server::memberships::Memberships::open(&home.memberships_db())
            .await
            .expect("open memberships");
        m.upsert(principal, "mine", Some("owner"))
            .await
            .expect("mine row");
        m.upsert(principal, "theirs", Some("member"))
            .await
            .expect("theirs row");
    })
    .await?;
    // Telemetry tools are listed only when a backend is configured;
    // nobody listens here, and `telemetry_status` never dials.
    // SAFETY: single-threaded with respect to env writes — see `boot`.
    unsafe {
        std::env::set_var("TASK_TELEMETRY_TEMPO_URL", "http://127.0.0.1:9");
    }
    task_server::central_auth::configured()
        .expect("configured")
        .remember_for_test(TOKEN, Some(principal.to_string()));

    let client = reqwest::Client::new();
    let rpc = |method: &'static str, params: serde_json::Value| {
        let client = client.clone();
        let url = format!("{base}/mcp");
        async move {
            client
                .post(url)
                .bearer_auth(TOKEN)
                .json(&serde_json::json!({
                    "jsonrpc": "2.0", "id": 1, "method": method, "params": params
                }))
                .send()
                .await
                .expect("POST /mcp")
                .json::<serde_json::Value>()
                .await
                .expect("json-rpc body")
        }
    };

    let listed = rpc("tools/list", serde_json::json!({})).await;
    assert!(
        listed.get("error").is_none(),
        "a central principal with membership rows must reach the lane: {listed}"
    );
    let names: Vec<&str> = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|t| t["name"].as_str())
        .collect();
    assert!(names.contains(&"list_orgs"), "{names:?}");
    assert!(names.contains(&"telemetry_status"), "{names:?}");

    let orgs = rpc(
        "tools/call",
        serde_json::json!({"name": "list_orgs", "arguments": {}}),
    )
    .await;
    let text = orgs["result"]["content"][0]["text"]
        .as_str()
        .expect("list_orgs text");
    assert!(text.contains("mine") && text.contains("theirs"), "{text}");
    assert!(
        !text.contains("nobodys"),
        "no row, no reach — the same fence as the RPC lane: {text}"
    );

    let status = rpc(
        "tools/call",
        serde_json::json!({"name": "telemetry_status", "arguments": {}}),
    )
    .await;
    let text = status["result"]["content"][0]["text"]
        .as_str()
        .expect("telemetry_status text");
    let payload: serde_json::Value = serde_json::from_str(text)?;
    assert_eq!(
        payload["allowed"], true,
        "an owner of the home org is an operator: {payload}"
    );
    Ok(())
}

/// Discovery names the principal it validated.
///
/// A client that holds a cached token used to learn who it was by
/// dialling the org lane for `whoami`, failing (an issuer token is
/// unknown to the org's store), then asking the issuer, then fetching
/// discovery AGAIN with the token — four round trips on every reload.
/// Discovery already resolves the token to tag membership; handing the
/// account back in the same response is what lets the client restore a
/// session from one fetch.
#[tokio::test(flavor = "multi_thread")]
async fn discovery_names_the_principal_it_validated() -> eyre::Result<()> {
    let principal = uuid::Uuid::new_v4();
    let (base, _tmp) = boot(async |data_root| {
        let home = data_root.org("mine");
        let m = task_server::memberships::Memberships::open(&home.memberships_db())
            .await
            .expect("open memberships");
        m.upsert(principal, "mine", Some("owner"))
            .await
            .expect("mine row");
    })
    .await?;
    task_server::central_auth::configured()
        .expect("configured")
        .remember_for_test(TOKEN, Some(principal.to_string()));

    let client = reqwest::Client::new();
    let doc: serde_json::Value = client
        .get(format!("{base}/.well-known/task-server.json"))
        .bearer_auth(TOKEN)
        .send()
        .await?
        .json()
        .await?;
    assert_eq!(doc["principal"]["id"], principal.to_string());
    assert_eq!(doc["principal"]["via"], "issuer");

    // No token, no principal — and never an error.
    let anon: serde_json::Value = client
        .get(format!("{base}/.well-known/task-server.json"))
        .send()
        .await?
        .json()
        .await?;
    assert!(anon["principal"].is_null(), "{}", anon["principal"]);
    Ok(())
}
