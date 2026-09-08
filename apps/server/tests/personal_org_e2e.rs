#![allow(clippy::large_futures)]
//! A brand-new account gets somewhere to save things.
//!
//! ADR 0003 makes the sibling apps ordinary clients: Keyflow signs a
//! person in at `fts-auth`, gets an access token, and writes a chart
//! through the same permits as everything else. That works for a person
//! who already belongs to an org. It does not work for a person who
//! signed up five seconds ago — `memberships::role_for` returns `None`
//! everywhere, every lane refuses (correctly), and `create_org` is
//! fenced to home-org users, so there is no move that gets them out of
//! it. The account is real and can reach nothing.
//!
//! `ensure_personal_org` is the one call that answers that state. What
//! makes it safe to expose where `create_org` is not: **the caller does
//! not choose the slug**. It is derived from the principal, so the call
//! grants exactly one org, to the account that asked, and cannot be
//! aimed at anybody else's.
//!
//! Its own binary: `central_auth::configured()` reads
//! `TASK_CENTRAL_AUTH_URL` once per process.

use org_proto::{CreateOrgRequest, OrgManagementServiceClient, PersonalOrgRequest};
use task_server::{AppState, router};
use tokio_tungstenite::tungstenite::client::IntoClientRequest as _;

/// The answer the issuer would give for our token.
const TOKEN: &str = "central-access-token-for-the-personal-org-test";
const EMAIL: &str = "newcomer@example.test";

/// Boot a server whose home org has a memberships store and nothing in
/// it — the state a real deployment is in for an account it has never
/// seen.
async fn boot() -> eyre::Result<(String, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    // SAFETY: this binary's tests run sequentially against one boot.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        // Nobody listens here. The one answer this test needs is seeded
        // into the resolver's cache below.
        std::env::set_var("TASK_CENTRAL_AUTH_URL", "http://127.0.0.1:9");
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org("mine", "Mine", true)
        .map_err(|e| eyre::eyre!("scaffold mine: {e}"))?;
    // Opened before boot, because `build_home_identity` only wires
    // cross-org identity when the file is already on disk. Empty: this
    // principal is a member of nothing, which is the whole premise.
    task_server::memberships::Memberships::open(&data_root.org("mine").memberships_db())
        .await
        .map_err(|e| eyre::eyre!("open memberships: {e}"))?;

    let state = AppState::new(None).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), tmp))
}

async fn server_lane(base: &str) -> eyre::Result<OrgManagementServiceClient> {
    let ws = format!("{}/server/vox", base.replacen("http", "ws", 1));
    let mut request = ws.into_client_request()?;
    request
        .headers_mut()
        .insert("sec-websocket-protocol", "vox.v1".parse()?);
    let (stream, _) = tokio_tungstenite::connect_async(request).await?;
    vox_core::initiator_on(vox_websocket::WsLink::new(stream))
        .establish::<OrgManagementServiceClient>()
        .await
        .map_err(|e| eyre::eyre!("establish: {e:?}"))
}

async fn discover(base: &str, bearer: &str) -> serde_json::Value {
    reqwest::Client::new()
        .get(format!("{base}/.well-known/task-server.json"))
        .bearer_auth(bearer)
        .send()
        .await
        .expect("discovery")
        .json()
        .await
        .expect("json")
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

/// The whole arc: refused, provisioned, idempotent, and a member at the
/// end of it.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_account_provisions_one_org_and_can_then_be_seen_to_belong() -> eyre::Result<()> {
    let principal = uuid::Uuid::new_v4();
    let (base, _tmp) = boot().await?;
    task_server::central_auth::configured()
        .expect("the issuer is configured for this binary")
        .remember_profile_for_test(TOKEN, &principal.to_string(), Some(EMAIL));

    let mgmt = server_lane(&base).await?;

    // ── The state this call exists for. A valid account, vouched for by
    //    the issuer, is a member of nothing.
    let before = discover(&base, TOKEN).await;
    assert_eq!(
        member_of(&before, "mine"),
        serde_json::json!(false),
        "the premise: no membership row anywhere"
    );

    // ── And `create_org` will not help them. It is fenced on being a
    //    home-org principal, which is exactly what they are not — so
    //    without this feature the account is stuck.
    let refused = mgmt
        .create_org(CreateOrgRequest {
            session_token: TOKEN.to_owned(),
            slug: "a-slug-i-picked".to_owned(),
            display_name: "Mine Now".to_owned(),
            is_home: false,
        })
        .await;
    assert!(
        refused.is_err(),
        "a principal with no membership must not be able to name an org"
    );

    // ── Nor does being unknown. Provisioning is unfenced on
    //    MEMBERSHIP, never on identity — that distinction is the only
    //    thing between this call and an open door, so it is asserted
    //    before the happy path rather than after it.
    task_server::central_auth::configured()
        .expect("configured")
        .remember_for_test("nobody-knows-this", None);
    for bogus in ["nobody-knows-this", ""] {
        let out = mgmt
            .ensure_personal_org(PersonalOrgRequest {
                session_token: bogus.to_owned(),
            })
            .await;
        assert!(out.is_err(), "`{bogus}` must not mint an org");
    }
    assert_eq!(
        mgmt.list_orgs()
            .await
            .map_err(|e| eyre::eyre!("list: {e:?}"))?
            .len(),
        1,
        "nothing was created for a token nobody vouches for"
    );

    // ── The one call that answers it.
    let created = mgmt
        .ensure_personal_org(PersonalOrgRequest {
            session_token: TOKEN.to_owned(),
        })
        .await
        .map_err(|e| eyre::eyre!("provisioning refused a valid account: {e:?}"))?;

    // The slug is derived from the principal, not chosen. The readable
    // half comes from the email; the unique half from the id, so two
    // accounts sharing a local part cannot collide.
    let expected = format!("newcomer-{}", &principal.simple().to_string()[..8]);
    assert_eq!(created.slug, expected);
    assert!(
        !created.is_home,
        "a personal org never claims the home slot"
    );

    // ── Idempotent. Calling again is how a client avoids keeping state
    //    about whether it has provisioned yet, so it must be free of
    //    consequence.
    let again = mgmt
        .ensure_personal_org(PersonalOrgRequest {
            session_token: TOKEN.to_owned(),
        })
        .await
        .map_err(|e| eyre::eyre!("second call refused: {e:?}"))?;
    assert_eq!(again.slug, created.slug);
    let hosted = mgmt
        .list_orgs()
        .await
        .map_err(|e| eyre::eyre!("list: {e:?}"))?;
    assert_eq!(
        hosted.len(),
        2,
        "home plus exactly one personal org, not one per call"
    );

    // ── The point of all of it: they now belong somewhere, by the same
    //    membership row every other lane fences on. Nothing here is a
    //    special case downstream.
    let after = discover(&base, TOKEN).await;
    assert_eq!(
        member_of(&after, &created.slug),
        serde_json::json!(true),
        "the new org is theirs"
    );
    assert_eq!(
        member_of(&after, "mine"),
        serde_json::json!(false),
        "and provisioning granted nothing beyond it"
    );

    Ok(())
}
