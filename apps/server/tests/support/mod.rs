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

/// A page `examples/studio/acme-audio/Vault/` seeds into the org vault.
/// No links, no tags — so a graph test must expect it among the orphans,
/// and a manifest test must expect it in the listing.
///
/// It is **not the only** seeded page, and a test must not assume it is.
/// It was once, and a manifest test pinned the listing to exactly this
/// plus whatever it wrote — so the day the seed grew a booking, that
/// test failed complaining about the manifest rather than about the
/// seed. Assert what you put is present, not that nothing else is.
pub const EXAMPLE_PAGE: &str = "Studio Notes.md";

/// The org root of a boot, from the tempdir that boot returned.
///
/// **Use this rather than `DataRoot::from_env()`** for any on-disk
/// assertion. `TASK_DATA_ROOT` is set under a lock for the duration of
/// `AppState::new` and then left pointing at whichever boot ran last —
/// so a test that reads it afterwards is reading a sibling test's
/// tempdir, and `cargo test` runs the binary's tests concurrently. The
/// tempdir the caller is already holding is the unambiguous answer.
#[must_use]
pub fn org_root(tmp: &tempfile::TempDir) -> org_proto::OrgRoot {
    org_proto::DataRoot::new(tmp.path().to_owned()).org(ORG)
}

/// Boot an `AppState` over a fresh tempdir data root holding the
/// example studio. Returns the tempdir so the caller keeps it alive.
pub async fn boot_app_state() -> eyre::Result<(AppState, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    let state = boot_over(tmp.path(), |_| {}).await?;
    Ok((state, tmp))
}

/// [`boot_ws`], with a chance to write onto the data root **after** the
/// example is planted and **before** `AppState::new` reads it.
///
/// The window matters for exactly one kind of test: anything that has
/// to be true about a server *coming up on a disk somebody else left*.
/// Boot-time migrations run inside `AppState::new`, so a test that
/// writes afterwards is testing nothing.
pub async fn boot_ws_with(
    prepare: impl FnOnce(&org_proto::OrgRoot),
) -> eyre::Result<(String, tempfile::TempDir)> {
    let tmp = tempfile::tempdir()?;
    let state = boot_over(tmp.path(), prepare).await?;
    Ok((serve(state).await?, tmp))
}

/// Boot a **second** server over a data root a previous boot produced —
/// the "restart" shape, for asserting that whatever happens at startup
/// is idempotent against its own output.
pub async fn boot_ws_over(tmp: &tempfile::TempDir) -> eyre::Result<(String, ())> {
    let state = boot_over(tmp.path(), |_| {}).await?;
    Ok((serve(state).await?, ()))
}

/// Serve an `AppState` over a real WebSocket on an ephemeral port.
async fn serve(state: AppState) -> eyre::Result<String> {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
    let port = listener.local_addr()?.port();
    let app = task_server::router(state);
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    Ok(format!("ws://127.0.0.1:{port}/vox"))
}

async fn boot_over(
    root: &std::path::Path,
    prepare: impl FnOnce(&org_proto::OrgRoot),
) -> eyre::Result<AppState> {
    // Serializes env-var twiddling. `cargo test` runs tests on a shared
    // thread pool; without this, two boots interleave their `set_var`s.
    // Safe because the lock is held for the whole window in which
    // `AppState::new` reads the environment.
    static ENV_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    let guard = ENV_LOCK.lock().await;
    // SAFETY: held under `ENV_LOCK` for the duration of
    // `AppState::new`, which reads the vars exactly once.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", root);
        // A developer's shell (or another test's leftovers) must not
        // leak a vault root or an org filter into this boot.
        std::env::remove_var("TASK_SERVER_VAULT_ROOT");
        std::env::remove_var("TASK_SERVER_ORG");
    }
    let data_root = org_proto::DataRoot::new(root.to_owned());
    // Skipped on a re-boot over a root a previous boot produced: the
    // org is already scaffolded, and `init_org` refuses rather than
    // reinitialising — which is right, and means "boot again" has to
    // say so here.
    if !data_root.org(ORG).path().is_dir() {
        data_root
            .init_org(ORG, "ACME Audio", true)
            .map_err(|e| eyre::eyre!("scaffold {ORG}: {e}"))?;
    }
    let org = data_root.org(ORG);
    task_server::example_org::install(&org, ORG)?;
    prepare(&org);
    let state = AppState::new(None).await?;
    drop(guard);
    Ok(state)
}

/// [`boot_app_state`], served over a real WebSocket on an ephemeral
/// port. Returns the `ws://…/vox` URL.
pub async fn boot_ws() -> eyre::Result<(String, tempfile::TempDir)> {
    boot_ws_with(|_| {}).await
}
