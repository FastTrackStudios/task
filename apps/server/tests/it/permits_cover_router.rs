//! The drift guard between `org_layer_router` and `permits::mounts`.
//!
//! `permits::mounts()` is a hand-kept parallel list of what the router
//! mounts — the same shape `schema_stamps` used to be, and the same shape
//! that silently rotted into "2 of 71 services actually gated". This test
//! makes the rot a build failure: mount a service without adding its
//! permit table and the counts diverge here.
//!
//! Since the plugin toggle, both sides are functions of the org's
//! `PluginSet`: the router takes its per-plugin branches from
//! `OrgAppState::plugins` and the registry filters via
//! `permits::mounts_for`. Two orgs are booted — one plain (everything
//! on: the default MUST equal the pre-plugin router) and one with a
//! deny-list — and each org's router must match its filtered registry.
//!
//! It boots a real `AppState` (against a throwaway data root) because the
//! router can only be built from a live org state.

use task_server::{AppState, org_layer_router, permits};

/// Serializes the env twiddle below — `AppState::new` reads
/// `TASK_DATA_ROOT` once at boot.
static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

#[tokio::test(flavor = "multi_thread")]
async fn every_mounted_service_has_a_permit_table() {
    let tmp = tempfile::tempdir().expect("temp data root");
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` for the duration of `AppState::new`,
    // which reads the var exactly once.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
    }
    // An empty data root hosts no orgs (bootstrap mode), so scaffold two:
    // one plain, one with a plugin deny-list in its manifest.
    let data_root = org_proto::DataRoot::from_env().expect("data root");
    data_root
        .init_org("permits-test", "Permits Test", true)
        .expect("scaffold org");
    let denied = data_root
        .init_org("no-mealplan", "No Mealplan", false)
        .expect("scaffold deny-list org");
    let mut manifest = denied.manifest().expect("deny-list org manifest");
    manifest.disabled_plugins = vec!["mealplan".to_owned(), "fitness".to_owned()].into();
    manifest
        .write_to_dir(denied.path())
        .expect("write deny-list manifest");
    let state = AppState::new(None).await.expect("boot AppState");
    drop(guard);

    // ── Plain org: default = everything, exactly the full registry ──
    let org = state.org("permits-test").expect("plain org hosted");
    let router = org_layer_router(&org);
    assert_eq!(
        router.len(),
        permits::mounts().len(),
        "org_layer_router mounts {} services but permits::mounts() lists {} — \
         add the new service (and its permit table + plugin id) to \
         `permits::mounts`",
        router.len(),
        permits::mounts().len(),
    );
    assert_eq!(
        permits::mounts_for(&org.plugins).len(),
        permits::mounts().len(),
        "no deny-list must resolve to the full registry",
    );

    // ── Deny-list org: the router drops exactly the denied plugins ──
    let org = state.org("no-mealplan").expect("deny-list org hosted");
    assert!(!org.plugins.contains("mealplan"));
    assert!(!org.plugins.contains("fitness"));
    let filtered = permits::mounts_for(&org.plugins);
    let router = org_layer_router(&org);
    assert_eq!(
        router.len(),
        filtered.len(),
        "org_layer_router's plugin branches disagree with \
         permits::mounts_for for the same PluginSet",
    );
    // Only mealplan + fitness mounts are gone; everything else intact.
    let dropped: Vec<&str> = permits::mounts()
        .iter()
        .filter(|m| m.plugin == "mealplan" || m.plugin == "fitness")
        .map(|m| m.descriptor.service_name)
        .collect();
    // Only provable when at least one of the denied plugins is compiled
    // in — a core-only build's catalog never contained their mounts.
    #[cfg(any(feature = "plugin-mealplan", feature = "plugin-fitness"))]
    assert!(!dropped.is_empty(), "the denied plugins own mounts");
    assert_eq!(router.len(), permits::mounts().len() - dropped.len());
    for m in &filtered {
        assert!(
            !dropped.contains(&m.descriptor.service_name),
            "{} belongs to a denied plugin but survived the filter",
            m.descriptor.service_name,
        );
    }

    // And the tables themselves are whole: no untabled service, no method
    // missing a permit (which would be fail-closed under enforcement), no
    // permit naming a method that does not exist.
    let coverage = permits::coverage();
    assert!(
        coverage.is_complete(),
        "permit coverage is incomplete:\n{}",
        permits::coverage_summary(),
    );
    assert_eq!(coverage.services, permits::mounts().len());
}
