//! In-process (`LocalServer`) transport check.
//!
//! Proves architect's "inject remote vs local, one client": the same
//! `ProjectServiceClient` the WebSocket transport produces can be
//! established over an in-memory vox link against the org's
//! `LayerRouter` — no socket, no running `task-server`. This is what
//! lets a native binary (CLI / desktop) drive the backend embedded.
//!
//! Boots `AppState` over the data root (same as the WS e2e tests), then
//! serves the first hosted org locally and round-trips `list()`.

use architect::Scope;
use project::ProjectServiceClient;
use task_server::AppState;

#[tokio::test(flavor = "multi_thread")]
async fn local_transport_round_trip() {
    let state = AppState::new(None).await.expect("boot AppState");
    let Some(slug) = state.org_slugs().into_iter().next() else {
        eprintln!("no org hosted in the data root — skipping local-transport test");
        return;
    };

    let scope = Scope::new();
    let local = state
        .local_server(&slug, &scope)
        .expect("org is hosted, so a local server should build");

    // Same client type as the WS transport, established in-process.
    let projects: ProjectServiceClient = local
        .establish()
        .await
        .expect("establish ProjectServiceClient over the in-process link");

    // A real RPC over the in-memory link: dispatch through the
    // LayerRouter to the project backend and back. We assert it returns
    // without error (the row count depends on the org's vault).
    let rows = projects
        .list()
        .await
        .expect("project list() round-trips over local transport");
    eprintln!(
        "local transport: project list() returned {} rows",
        rows.len()
    );

    // A second establish proves the same router multiplexes more than one
    // session (each establish gets its own in-memory link + acceptor).
    let _projects2: ProjectServiceClient = local
        .establish()
        .await
        .expect("a second client establishes over the same local server");

    scope.close().await;
}
