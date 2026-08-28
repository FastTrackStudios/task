//! In-process (`LocalServer`) transport check.
//!
//! Proves architect's "inject remote vs local, one client": the same
//! `ProjectServiceClient` the WebSocket transport produces can be
//! established over an in-memory vox link against the org's
//! `LayerRouter` — no socket, no running `task-server`. This is what
//! lets a native binary (CLI / desktop) drive the backend embedded.
//!
//! Boots `AppState` over the repo's example studio (see `support`,
//! same as the WS e2e tests), then serves the example org locally and
//! round-trips `list()`.

use architect::Scope;
use project::ProjectServiceClient;

// Not every binary uses every helper the shared module offers.
#[allow(dead_code)]
mod support;

#[tokio::test(flavor = "multi_thread")]
async fn local_transport_round_trip() {
    // Over the example studio (see `support`) rather than whatever data
    // root the environment resolves — "the first hosted org" on a
    // developer machine used to be a real one.
    let (state, _tmp) = support::boot_app_state().await.expect("boot AppState");
    let slug = support::ORG.to_string();
    assert!(
        state.org_slugs().contains(&slug),
        "the example org is hosted"
    );

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
