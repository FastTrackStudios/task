#![allow(clippy::large_futures)]
//! `task doctor`'s skew check passes against a server built from the
//! same tree.
//!
//! The CLI links `task_server::schema_stamps()` and diffs it against
//! the `schema_stamps` map the running server serves from
//! `/.well-known/task-server.json`. Two mounted descriptors share the
//! wire name `SyncService` (`files_proto`'s facets/devices lane and
//! `files_sync`'s replica lane — both macros name a service after its
//! trait). The list carried that name twice, the server's JSON map kept
//! only the last stamp, and the CLI compared both — so `task doctor`
//! against production reported `STALE — stamp mismatch on: SyncService`
//! with the CLI and the server built from one commit. Nothing about
//! cargo features or the wire differed; the guard was comparing itself
//! against half of itself.
//!
//! These go through the real router over HTTP, the way the CLI does,
//! and through the same `schema_stamp::diff` the CLI calls.

use std::collections::{HashMap, HashSet};

use task_server::{AppState, router};

async fn boot() -> eyre::Result<(String, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    let guard = crate::support::env_lock().await;
    // SAFETY: held under `ENV_LOCK` while `AppState` reads the env.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
    }
    let data_root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    data_root
        .init_org("studio", "Studio", true)
        .map_err(|e| eyre::eyre!("scaffold studio: {e}"))?;
    let state = AppState::new(None).await?;
    drop(guard);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("http://127.0.0.1:{port}"), tmp))
}

/// The served map, diffed exactly as `task doctor` diffs it, is clean —
/// and it holds every stamp this build computes, none collapsed away.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_schema_check_is_clean_against_the_same_build() -> eyre::Result<()> {
    let (base, _tmp) = boot().await?;
    let doc: serde_json::Value = reqwest::get(format!("{base}/.well-known/task-server.json"))
        .await?
        .json()
        .await?;
    let served = doc["schema_stamps"]
        .as_object()
        .expect("the well-known doc serves schema_stamps");

    let local = task_server::schema_stamps();
    let skew =
        org_proto::schema_stamp::diff(&local, |name| served.get(name).and_then(|v| v.as_str()));
    assert!(
        skew.is_clean(),
        "a CLI and server from one tree must agree; skew: {skew:?}"
    );
    assert_eq!(
        served.len(),
        local.len(),
        "the served map lost entries — a stamp name appears twice in \
         `schema_stamps()`, and a JSON map keeps only one of them"
    );

    // The name that broke it is stamped once, folding both lanes.
    let sync: Vec<_> = local.iter().filter(|(n, _)| *n == "SyncService").collect();
    assert_eq!(sync.len(), 1, "SyncService is one wire service: {sync:?}");
    let lanes: Vec<_> = task_server::permits::mounted_descriptors()
        .into_iter()
        .filter(|d| d.service_name == "SyncService")
        .collect();
    assert_eq!(
        lanes.len(),
        2,
        "files_proto's and files_sync's SyncService are both mounted"
    );
    let folded = lanes
        .iter()
        .flat_map(|d| d.methods.iter())
        .fold(0u64, |acc, m| acc ^ m.id.0);
    assert_eq!(sync[0].1, format!("{folded:016x}"));
    Ok(())
}

/// The `/org/{slug}/api` half (`task doctor`'s API check looks services
/// up by name): every listed service's stamp is the one this build
/// computes for that name, whichever same-named entry is found first.
#[tokio::test(flavor = "multi_thread")]
async fn doctor_api_check_is_clean_against_the_same_build() -> eyre::Result<()> {
    let (base, _tmp) = boot().await?;
    let api: serde_json::Value = reqwest::get(format!("{base}/org/studio/api"))
        .await?
        .json()
        .await?;
    let local: HashMap<&str, String> = task_server::schema_stamps().into_iter().collect();
    for s in api["services"].as_array().expect("services array") {
        let name = s["name"].as_str().expect("name");
        assert_eq!(
            s["stamp"].as_str(),
            local.get(name).map(String::as_str),
            "{name} ({:?})",
            s["alias"]
        );
    }
    Ok(())
}

/// Sharing a wire name is survivable only while the method names differ:
/// a method id hashes `service.method`, and the router keeps the last
/// handler mounted for an id. Two mounted methods with one id would
/// silently route one lane's calls to the other.
#[test]
fn no_two_mounted_methods_share_a_wire_id() {
    let mut seen = HashSet::new();
    for d in task_server::permits::mounted_descriptors() {
        for m in d.methods {
            assert!(
                seen.insert(m.id.0),
                "{}.{} collides with another mounted method's id",
                d.service_name,
                m.method_name
            );
        }
    }
}
