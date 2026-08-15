#![allow(clippy::large_futures)]
//! Discovery tags which orgs the caller actually belongs to (#109
//! criterion 6).
//!
//! The org switcher defaulted to *All organizations*, and "all" meant
//! every org hosted on the server — so a signed-in user's aggregate views
//! fanned out across orgs that were none of their business, and an
//! anonymous visitor's fanned out across all of them.
//!
//! Membership is asked of each org **independently**: every org has its
//! own auth database, and the direction of travel is for an org to be
//! able to live on its own server. So there is no cross-org membership
//! table here and none is assumed — the question is only ever "does this
//! token validate against THIS org".
//!
//! The list itself is deliberately NOT filtered server-side: a client has
//! to see an org before it can sign into it, and discovery runs before
//! any session exists. Each entry is tagged instead, and the client
//! decides what "All" spans (`task_ui_core::orgs::my_orgs`).

use architect_auth::CreateEmailPasswordUser;
use task_server::{AppState, router};

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// 32+ chars, as `auth_secret` requires.
const TEST_AUTH_SECRET: &str = "task-server-test-auth-secret-32+!";

/// Boot a server hosting TWO orgs, so "mine" vs "not mine" is a real
/// distinction rather than a degenerate one-org case.
async fn boot() -> eyre::Result<(String, AppState, tempfile::TempDir)> {
    boot_with(|_| async {}).await
}

/// [`boot`], with a hook that runs against the scaffolded data root
/// BEFORE `AppState::new`.
///
/// The hook exists because cross-org identity is wired at startup:
/// `AppState::new` opens the home org's memberships store only if the
/// file is already there. So a membership written afterwards does not
/// take effect until the server restarts — which is the real operational
/// contract (`admin adopt-principal`, then roll the pod), and a test that
/// wrote the row after boot would pass while production did not.
async fn boot_with<F, Fut>(prepare: F) -> eyre::Result<(String, AppState, tempfile::TempDir)>
where
    F: FnOnce(org_proto::DataRoot) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org("mine", "Mine", true)
        .map_err(|e| eyre::eyre!("scaffold mine: {e}"))?;
    data_root
        .init_org("theirs", "Theirs", false)
        .map_err(|e| eyre::eyre!("scaffold theirs: {e}"))?;
    prepare(data_root).await;
    // `AppState::new` opens one AuthState per org's own auth.sqlite —
    // which is exactly why a token from one org means nothing in another.
    let state = AppState::new(None).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), state, tmp))
}

/// `slug -> member` from the well-known doc.
async fn discover(base: &str, bearer: Option<&str>) -> serde_json::Value {
    let client = reqwest::Client::new();
    let mut req = client.get(format!("{base}/.well-known/task-server.json"));
    if let Some(token) = bearer {
        req = req.bearer_auth(token);
    }
    req.send()
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

#[tokio::test(flavor = "multi_thread")]
async fn discovery_tags_membership_per_org() -> eyre::Result<()> {
    let (base, state, _tmp) = boot().await?;

    // A user that exists ONLY in `mine`.
    let bundle = state
        .org("mine")
        .expect("mine hosted")
        .auth
        .auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "member@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: Some("Member".into()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed user: {e:?}"))?;

    // ── Anonymous discovery: both orgs listed, membership UNKNOWN.
    //    Unknown, not false — the client must still show them, because
    //    seeing an org is how you reach its sign-in.
    let anon = discover(&base, None).await;
    assert_eq!(anon["orgs"].as_array().expect("orgs").len(), 2);
    assert!(member_of(&anon, "mine").is_null());
    assert!(member_of(&anon, "theirs").is_null());

    // ── Signed in: still both listed, but now tagged — and `theirs`
    //    comes back FALSE, because its auth database has never heard of
    //    this token. That tag is what stops "All organizations" from
    //    meaning "every org on this server".
    let signed_in = discover(&base, Some(&bundle.token)).await;
    assert_eq!(signed_in["orgs"].as_array().expect("orgs").len(), 2);
    assert_eq!(member_of(&signed_in, "mine"), serde_json::json!(true));
    assert_eq!(member_of(&signed_in, "theirs"), serde_json::json!(false));

    // ── A garbage token is not a membership claim.
    let bogus = discover(&base, Some("not-a-real-token")).await;
    assert_eq!(member_of(&bogus, "mine"), serde_json::json!(false));
    assert_eq!(member_of(&bogus, "theirs"), serde_json::json!(false));

    Ok(())
}

/// A membership row makes a home token mean something in another org.
///
/// This is `plans/one-account-per-server.md` from the outside: sign in
/// once, at home, and every org holding a row reports `member: true` —
/// which is what the client turns into "All organizations" actually
/// spanning them (`orgs::selected_slugs`, the choke point every
/// multi-org fan-out goes through).
///
/// The negative half matters as much. After the org lane learned to
/// accept home tokens, that row is the ONLY fence between an org's data
/// and any valid home session — a missing row must read as "not a
/// member", never as "member with the default role".
#[tokio::test(flavor = "multi_thread")]
async fn a_membership_row_extends_a_home_token_to_another_org() -> eyre::Result<()> {
    // The principal must exist before a row can name it, and the row must
    // exist before boot — so both happen in the prepare hook.
    let seeded: std::sync::Arc<std::sync::Mutex<Option<uuid::Uuid>>> =
        std::sync::Arc::new(std::sync::Mutex::new(None));
    let out = std::sync::Arc::clone(&seeded);

    let (base, state, _tmp) = boot_with(move |root| async move {
        // Pin the signing secret so this open and `AppState::new`'s agree
        // — a token minted under a different secret would simply not
        // validate, and the test would "fail" for an unrelated reason.
        // Safe here: `boot_with` holds ENV_LOCK across this hook.
        unsafe {
            std::env::set_var("TASK_AUTH_SECRET", TEST_AUTH_SECRET);
        }
        let home = root.org("mine");
        let url = format!("sqlite://{}?mode=rwc", home.auth_db().display());
        let auth = task_server::AuthState::open(&url, TEST_AUTH_SECRET)
            .await
            .expect("open mine auth");
        let bundle = auth
            .auth
            .create_email_password_user(CreateEmailPasswordUser {
                email: "cody@example.test".into(),
                password: "correct-horse-battery-staple".into(),
                name: Some("Cody".into()),
                username: None,
                image: None,
                metadata_json: None,
                ip_address: None,
                user_agent: None,
            })
            .await
            .expect("seed principal");
        *out.lock().expect("seed slot") = Some(bundle.user.id);

        // A row for `theirs` only: `mine` needs none, since a token its
        // own store issued is already membership there.
        let m = task_server::memberships::Memberships::open(&home.memberships_db())
            .await
            .expect("open memberships");
        m.upsert(bundle.user.id, "theirs", Some("member"))
            .await
            .expect("write membership");
    })
    .await?;

    let user_id = seeded.lock().expect("seed slot").expect("principal seeded");
    let home_auth = &state.org("mine").expect("mine hosted").auth.auth;

    // One sign-in, at home, as a person actually does.
    let bundle = home_auth
        .sign_in_email_password(architect_auth::SignInEmailPassword {
            email: "cody@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("sign in: {e:?}"))?;
    assert_eq!(bundle.user.id, user_id, "same principal");

    // BOTH orgs report membership off that ONE session.
    let doc = discover(&base, Some(&bundle.token)).await;
    assert_eq!(member_of(&doc, "mine"), serde_json::json!(true));
    assert_eq!(
        member_of(&doc, "theirs"),
        serde_json::json!(true),
        "the row is what makes a home token mean something here"
    );

    // Another principal at home, with no row, stays out — membership is
    // per-account, not "anyone the home org happens to know".
    let stranger = home_auth
        .create_email_password_user(CreateEmailPasswordUser {
            email: "stranger@example.test".into(),
            password: "correct-horse-battery-staple".into(),
            name: None,
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .map_err(|e| eyre::eyre!("seed stranger: {e:?}"))?;
    let doc = discover(&base, Some(&stranger.token)).await;
    assert_eq!(member_of(&doc, "mine"), serde_json::json!(true));
    assert_eq!(
        member_of(&doc, "theirs"),
        serde_json::json!(false),
        "no row, no membership — the row is the fence"
    );

    Ok(())
}
