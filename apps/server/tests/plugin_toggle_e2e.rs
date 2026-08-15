//! End-to-end cover for the per-org plugin toggle.
//!
//! One booted `AppState` over an org whose `org.toml` deny-lists
//! `mealplan`, exercised on both surfaces:
//!
//! - **wire**: a mealplan RPC fails (the service is not mounted on the
//!   org router — unknown service, the same failure an old client gets
//!   from a server that never shipped the feature) while a core task
//!   RPC on the same connection succeeds;
//! - **HTTP**: `GET /org/{slug}/api` lists the whole catalog with the
//!   mealplan services flagged `"mounted": false` and the top-level
//!   `plugins` entry `"enabled": false`.
//!
//! Self-sandboxed: tempdir data root via `TASK_DATA_ROOT`, one test per
//! binary so the env setup races nothing.

// This test exercises services owned by the `mealplan` plugin;
// a build without it has nothing to cover.
#![cfg(feature = "plugin-mealplan")]

use architect::Scope;
use task_server::AppState;

#[tokio::test(flavor = "multi_thread")]
async fn disabled_plugin_is_unmounted_and_reported() {
    let tmp = tempfile::tempdir().unwrap();
    // SAFETY: one test per binary, so nothing races this env setup.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        for var in ["TASK_SERVER_ORG", "TASK_SERVER_VAULT_ROOT"] {
            std::env::remove_var(var);
        }
    }
    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();
    // Deny-list mealplan in the manifest — the toggle under test.
    let mut manifest = org_root.manifest().unwrap();
    manifest.disabled_plugins = vec!["mealplan".to_owned()].into();
    manifest.write_to_dir(org_root.path()).unwrap();

    let state = AppState::new(None).await.expect("boot AppState");
    let org = state.org("alpha").expect("alpha is hosted");
    assert!(
        !org.plugins.contains("mealplan"),
        "manifest deny-list resolved"
    );
    assert!(org.plugins.contains("fitness"), "only mealplan is off");

    // ── wire: mealplan refuses, task answers ─────────────────────
    let scope = Scope::new();
    let local = state
        .local_server("alpha", &scope)
        .expect("local transport");

    let tasks: task::TaskServiceClient = local.establish().await.expect("task client");
    let listed = tasks.list().await;
    assert!(
        listed.is_ok(),
        "core task RPC must keep working: {listed:?}"
    );

    match local.establish::<mealplan::MealplanServiceClient>().await {
        // Either shape is the toggle working: refused at establish, or
        // established (typed clients can be lazy) and refused per call.
        Err(_) => {}
        Ok(meals) => {
            let res = meals.list().await;
            assert!(
                res.is_err(),
                "mealplan RPC must fail unknown-service when the plugin \
                 is disabled, got: {res:?}"
            );
        }
    }

    // ── HTTP: /org/{slug}/api reports the toggle ─────────────────
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let body: serde_json::Value = reqwest::get(format!("http://127.0.0.1:{port}/org/alpha/api"))
        .await
        .expect("GET /org/alpha/api")
        .error_for_status()
        .expect("200")
        .json()
        .await
        .expect("json body");

    let plugins = body["plugins"].as_array().expect("plugins catalog");
    let mealplan = plugins
        .iter()
        .find(|p| p["id"] == "mealplan")
        .expect("mealplan in the catalog");
    assert_eq!(mealplan["enabled"], false, "mealplan reported disabled");
    assert!(
        plugins
            .iter()
            .filter(|p| p["id"] != "mealplan")
            .all(|p| p["enabled"] == true),
        "every other plugin stays enabled"
    );

    let services = body["services"].as_array().expect("services");
    let (off, on): (Vec<_>, Vec<_>) = services.iter().partition(|s| s["mounted"] == false);
    assert!(
        !off.is_empty() && off.iter().all(|s| s["plugin"] == "mealplan"),
        "exactly the mealplan services are listed unmounted"
    );
    assert!(
        on.iter().any(|s| s["alias"] == "task"),
        "task stays mounted"
    );
    assert_eq!(body["mounted_count"].as_u64().unwrap() as usize, on.len(),);
}
