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
