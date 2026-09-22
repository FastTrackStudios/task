//! Async snapshot HTTP trigger — `POST /server/snapshot?wait=0` +
//! `GET /server/snapshot/status`. Separate test binary so its env-var
//! setup never races the synchronous-cycle e2e test.

use task_server::AppState;

const BACKUP_TOKEN: &str = "async-backup-token";

/// The async HTTP trigger: `POST /server/snapshot?wait=0` returns 202
/// immediately, the cycle runs on a background task, and
/// `GET /server/snapshot/status` reports it through to `done`. This
/// is the path external callers use when a full cycle outruns a proxy
/// request timeout. Real HTTP roundtrip against a bound router.
#[tokio::test(flavor = "multi_thread")]
async fn async_snapshot_kickoff_and_status() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: serialized — these integration tests run single-threaded
    // per binary for env-var setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_BACKUP_GIT_TOKEN", BACKUP_TOKEN);
        for var in [
            "TASK_BACKUP_REMOTE_BASE",
            "TASK_BACKUP_GIT_USER",
            "TASK_SERVER_ORG",
        ] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();
    std::fs::write(org_root.vault_dir().join("note.md"), "state A\n").unwrap();

    let state = AppState::new(None).await.expect("boot AppState");
    let scope = state.scope.clone();
    let app = task_server::router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");
    let client = reqwest::Client::new();

    // Bad token → 401 on both routes.
    let r = client
        .get(format!("{base}/server/snapshot/status"))
        .bearer_auth("wrong")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 401);

    // Kick off → 202 with phase=running.
    let r = client
        .post(format!("{base}/server/snapshot?wait=0"))
        .bearer_auth(BACKUP_TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), 202);
    let body: serde_json::Value = r.json().await.unwrap();
    assert_eq!(body["phase"], "running");
    assert!(body["started_at"].is_string());

    // Poll status until terminal.
    let mut final_phase = String::new();
    let mut repos = 0usize;
    for _ in 0..100 {
        let s: serde_json::Value = client
            .get(format!("{base}/server/snapshot/status"))
            .bearer_auth(BACKUP_TOKEN)
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap();
        let phase = s["phase"].as_str().unwrap_or("").to_string();
        if phase == "done" || phase == "failed" {
            final_phase = phase;
            repos = s["repos"].as_array().map_or(0, Vec::len);
            assert!(s["finished_at"].is_string());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
    assert_eq!(final_phase, "done", "async cycle must reach done");
    assert_eq!(repos, 2, "alpha repo + full repo");

    scope.close().await;
}
