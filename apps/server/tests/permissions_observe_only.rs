//! Observe-only mode: it must PASS EVERYTHING THROUGH and still say
//! exactly what it would have refused.
//!
//! Two claims, both load-bearing for the rollout:
//!
//! 1. **Defaults are unchanged.** With no `TASK_ENFORCE_PERMISSIONS`, a
//!    client with no bearer token still gets a real answer from a tabled
//!    service — the same as before any permit table existed. (Every other
//!    e2e test in this directory connects unauthenticated too, so they
//!    would all fail if this regressed; this one asserts it on purpose.)
//! 2. **The observe-only log is diagnosable.** The refusal that did not
//!    happen is recorded with the principal, the reason, and — the part
//!    the stock `TracingAudit` drops on the floor — the service and
//!    method.

use task_server::{AppState, permission_deny_ledger, router};
use vault_proto::VaultSyncClient;

static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread")]
async fn observe_only_passes_through_and_records_what_it_would_deny() {
    let tmp = tempfile::tempdir().expect("temp data root");
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState::new` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::remove_var("TASK_ENFORCE_PERMISSIONS");
    }
    let data_root = org_proto::DataRoot::from_env().expect("data root");
    data_root
        .init_org("observe-test", "Observe Test", true)
        .expect("scaffold org");
    let state = AppState::new(None).await.expect("boot AppState");
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });

    // No bearer token anywhere: the gate resolves `Principal::Anonymous`,
    // the role engine denies it, and observe-only lets it through anyway.
    let url = format!("ws://127.0.0.1:{port}/vox");
    let sync: VaultSyncClient = vox::connect_lane(&url).establish().await.unwrap();
    let manifest = sync
        .manifest("default".to_string())
        .await
        .expect("observe-only must not refuse the call");
    // A real answer, not a denial encoded as an empty one.
    eprintln!("manifest entries: {}", manifest.files.len());

    // …and the would-be denial is on the record, with enough to act on.
    let (denials, _overflow) = permission_deny_ledger().snapshot();
    let joined = denials
        .iter()
        .map(|d| d.reason.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !denials.is_empty(),
        "observe-only recorded nothing for an unauthenticated call",
    );
    assert!(
        joined.contains("vault-sync/manifest"),
        "the would-deny record names neither the service nor the method:\n{joined}",
    );
    assert!(
        joined.contains("anonymous"),
        "the would-deny record does not name the principal:\n{joined}",
    );
}
