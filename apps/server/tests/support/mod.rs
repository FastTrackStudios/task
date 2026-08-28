//! Shared boot for the e2e binaries: a sandboxed data root seeded with
//! the repo's example studio, so every test runs against the example
//! vault (`examples/studio/acme-audio/Vault/`) rather than whatever the
//! machine's real `~/.task` happens to hold.
//!
//! That last clause is not hypothetical. These binaries used to set
//! `TASK_SERVER_VAULT_ROOT` (or nothing at all) and let `AppState::new`
//! resolve the data root from the environment — which on a developer
//! machine is the real `~/.task`, so the tests booted every real org,
//! raced each other on its storage registry, and once wrote adoption
//! markers into real vaults. A test that reads the developer's disk is
//! not a test; it is a different program on every machine.
//!
//! One org, `acme-audio`, planted by the same `example_org::install`
//! the integration harness and `task-server admin demo` use — so the
//! world these tests assert against is the world the scenario chapters
//! assert against, and the seeded page every assertion may meet is
//! [`EXAMPLE_PAGE`].

use task_server::AppState;

/// The org these tests boot — the example studio's audio company.
pub const ORG: &str = "acme-audio";

/// The one page `examples/studio/acme-audio/Vault/` seeds into the org
/// vault. No links, no tags — so a graph test must expect it among the
/// orphans, and a manifest test must expect it in the listing.
pub const EXAMPLE_PAGE: &str = "Studio Notes.md";

/// Boot an `AppState` over a fresh tempdir data root holding the
/// example studio. Returns the tempdir so the caller keeps it alive.
pub async fn boot_app_state() -> eyre::Result<(AppState, tempfile::TempDir)> {
    // Serializes env-var twiddling. `cargo test` runs tests on a shared
    // thread pool; without this, two boots interleave their `set_var`s.
    // Safe because the lock is held for the whole window in which
    // `AppState::new` reads the environment.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    let tmp = tempfile::tempdir()?;
    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` for the duration of
    // `AppState::new`, which reads the vars exactly once.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        // A developer's shell (or another test's leftovers) must not
        // leak a vault root or an org filter into this boot.
        std::env::remove_var("TASK_SERVER_VAULT_ROOT");
        std::env::remove_var("TASK_SERVER_ORG");
    }
    let root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    root.init_org(ORG, "ACME Audio", true)
        .map_err(|e| eyre::eyre!("scaffold {ORG}: {e}"))?;
    task_server::example_org::install(&root.org(ORG), ORG)?;
    let state = AppState::new(None).await?;
    drop(guard);
    Ok((state, tmp))
}

/// [`boot_app_state`], served over a real WebSocket on an ephemeral
/// port. Returns the `ws://…/vox` URL.
pub async fn boot_ws() -> eyre::Result<(String, tempfile::TempDir)> {
    let (state, tmp) = boot_app_state().await?;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok((format!("ws://127.0.0.1:{port}/vox"), tmp))
}
