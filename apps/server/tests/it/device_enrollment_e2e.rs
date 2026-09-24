//! `DeviceEnrollmentService` on `/server/vox`: one call enrols a machine
//! with every org the caller's account is a member of.
//!
//! The setup is the central-auth one: a principal the issuer vouches
//! for, whose memberships the server mirrors (`remember_orgs_for_test`
//! stands in for the issuer's answer). The assertions are about what
//! the server does with it — which orgs get a device row, which do not,
//! and that asking twice leaves one row.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use files_proto::DeviceEnrollmentServiceClient;
use files_proto::service::sync::SyncService as _;
use task_server::{AppState, router};

const TOKEN: &str = "central-session-token";
const ENDPOINT: &str = "4bca942e5de4cda31d40c920ee4b88b01bf08cfd34ed1777cfcb953fcd072c6f";

async fn boot() -> eyre::Result<(AppState, String, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
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
    // The home org's membership table is what the account lane resolves
    // an account against; opening it is what makes the server treat
    // `mine` as a home with an identity to consult.
    let home = data_root.org("mine");
    task_server::memberships::Memberships::open(&home.memberships_db())
        .await
        .map_err(|e| eyre::eyre!("open memberships: {e}"))?;
    let state = AppState::new(None).await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state.clone());
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((state, format!("ws://127.0.0.1:{port}/server/vox"), tmp))
}

async fn enrollment(ws: &str) -> eyre::Result<DeviceEnrollmentServiceClient> {
    let client: DeviceEnrollmentServiceClient = vox::connect_lane(ws)
        .establish()
        .await
        .map_err(|e| eyre::eyre!("establish: {e:?}"))?;
    Ok(client)
}

#[tokio::test(flavor = "multi_thread")]
async fn one_call_enrols_the_machine_in_every_org_the_account_is_in() -> eyre::Result<()> {
    let _guard = crate::support::env_lock().await;
    let (state, ws, _tmp) = boot().await?;

    let principal = uuid::Uuid::new_v4();
    let central = task_server::central_auth::configured().expect("central auth configured");
    central.remember_for_test(TOKEN, Some(principal.to_string()));
    central.remember_orgs_for_test(
        TOKEN,
        Some(vec![
            ("mine".to_owned(), Some("owner".to_owned())),
            ("theirs".to_owned(), Some("member".to_owned())),
        ]),
    );

    let client = enrollment(&ws).await?;
    let enrolled = client
        .enroll_everywhere(TOKEN.into(), ENDPOINT.into(), "the laptop".into())
        .await
        .map_err(|e| eyre::eyre!("enroll: {e}"))?;

    let mut slugs: Vec<&str> = enrolled.iter().map(|e| e.slug.as_str()).collect();
    slugs.sort_unstable();
    assert_eq!(
        slugs,
        ["mine", "theirs"],
        "both orgs the issuer grants, and not the one it does not"
    );
    assert!(
        enrolled.iter().all(|e| !e.display_name.is_empty()),
        "each answer names the org: {enrolled:?}"
    );

    // The rows are the same rows `task files device pair` makes, org by
    // org — one per org, carrying the endpoint that was presented.
    for slug in ["mine", "theirs"] {
        let org = state.org(slug).expect("org is live");
        let devices = org.files.devices().await.expect("devices");
        let ours: Vec<_> = devices
            .iter()
            .filter(|d| d.endpoint.as_deref() == Some(ENDPOINT))
            .collect();
        assert_eq!(ours.len(), 1, "{slug}: one row for the machine");
        assert_eq!(ours[0].name, "the laptop");
        assert!(!ours[0].revoked);
    }
    let nobodys = state.org("nobodys").expect("org is live");
    assert!(
        nobodys
            .files
            .devices()
            .await
            .expect("devices")
            .iter()
            .all(|d| d.endpoint.as_deref() != Some(ENDPOINT)),
        "an org the account is not in is not touched"
    );

    // Asking again renames rather than duplicates.
    let again = client
        .enroll_everywhere(TOKEN.into(), ENDPOINT.into(), "the same laptop".into())
        .await
        .map_err(|e| eyre::eyre!("enroll again: {e}"))?;
    assert_eq!(again.len(), 2);
    let mine = state.org("mine").expect("org is live");
    let ours: Vec<_> = mine
        .files
        .devices()
        .await
        .expect("devices")
        .into_iter()
        .filter(|d| d.endpoint.as_deref() == Some(ENDPOINT))
        .collect();
    assert_eq!(ours.len(), 1, "idempotent on the endpoint");
    assert_eq!(ours[0].name, "the same laptop");
    assert_eq!(
        again[0].device_id,
        enrolled
            .iter()
            .find(|e| e.slug == again[0].slug)
            .expect("same org")
            .device_id,
        "the same device row comes back"
    );

    // `enrollments` reads what `enroll_everywhere` wrote, and nothing
    // for a machine nobody enrolled.
    let listed = client
        .enrollments(TOKEN.into(), ENDPOINT.into())
        .await
        .map_err(|e| eyre::eyre!("enrollments: {e}"))?;
    assert_eq!(listed.len(), 2);
    let unknown = client
        .enrollments(TOKEN.into(), "not-enrolled-anywhere".into())
        .await
        .map_err(|e| eyre::eyre!("enrollments: {e}"))?;
    assert!(unknown.is_empty());
    Ok(())
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_that_names_nobody_is_refused() -> eyre::Result<()> {
    let _guard = crate::support::env_lock().await;
    let (_state, ws, _tmp) = boot().await?;
    let client = enrollment(&ws).await?;
    let refused = client
        .enroll_everywhere("no-such-session".into(), ENDPOINT.into(), "x".into())
        .await;
    assert!(refused.is_err(), "an unknown token enrols nothing");
    let blank = client
        .enroll_everywhere(String::new(), ENDPOINT.into(), "x".into())
        .await;
    assert!(blank.is_err(), "no token, no enrolment");
    Ok(())
}
