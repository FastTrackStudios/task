//! Chapter twenty-eight — the app's own client dials over iroh.
//!
//! Every other chapter drives the harness's [`integration::client::Session`],
//! which was purpose-built for the suite. This one drives the REAL app
//! client — `task_ui_core::vox_clients::establish_for`, the function
//! behind every typed client the desktop app builds — and proves the
//! whole native path:
//!
//! - discovery hands the transport an org's iroh endpoint id
//!   (`iroh_transport::note_org_endpoints`, fed by the well-known doc in
//!   the app);
//! - the device endpoint binds once, key persisted, addresses resolved
//!   from `TASK_IROH_PEER_DIR` exactly the way the demo's servers
//!   exchange theirs;
//! - the dial is by bare id — **no URL is configured anywhere in this
//!   test**, which is the assertion that matters: were the iroh path to
//!   fail, `caller_for` would fall back to `org_ws_url` and error with
//!   "no vox URL configured", failing the test;
//! - and the session token rides every call as a global middleware on
//!   the shared caller (`Caller::with_global_middleware`), so a typed
//!   client reaches the permission gate as Alice, not as anonymous.
//!
//! One test, deliberately: the device endpooint and the env it reads
//! (`TASK_IROH_PEER_DIR`, `TASK_DEVICE_KEY`) are process-wide, so this
//! binary stays single-test the way `restart.rs` stays single-server.

use project::ProjectServiceClient;

use integration::scenario::Scenario;

#[tokio::test(flavor = "multi_thread")]
async fn the_app_client_reaches_an_org_over_iroh_as_a_signed_in_person() {
    let s = Scenario::open().await;

    // The address exchange: what `iroh_host::publish_addr` writes on a
    // real server and `just demo serve` wires through TASK_IROH_PEER_DIR.
    let peers = tempfile::tempdir().expect("peer dir");
    let addr = serde_json::to_vec(&s.orgs.acme.endpoint.addr()).expect("encode addr");
    std::fs::write(peers.path().join("acme-audio.json"), addr).expect("publish addr");

    // The device's own state: fresh key, fresh peer knowledge. Set
    // BEFORE the first dial — the transport reads both lazily, once.
    let device_key = tempfile::tempdir().expect("device key dir");
    // SAFETY: single-test binary; nothing else reads env concurrently.
    unsafe {
        std::env::set_var("TASK_IROH_PEER_DIR", peers.path());
        std::env::set_var("TASK_DEVICE_KEY", device_key.path().join("device.ed25519"));
        // The negative half: no URL means no WebSocket fallback, so a
        // pass proves the traffic went over iroh.
        std::env::remove_var("TASK_VOX_URL");
    }

    // What the app learns from discovery and sign-in, told directly:
    // the org's endpoint id, and Alice's real session token.
    task_ui_core::iroh_transport::note_org_endpoints([(
        "acme-audio",
        Some(s.orgs.acme.endpoint.id().to_string().as_str()),
    )]);
    task_ui_core::vox_session::set_session_token(Some(s.people.alice.token.clone()));

    // The app's own establish path, end to end.
    let projects: ProjectServiceClient = task_ui_core::vox_clients::establish_for("acme-audio")
        .await
        .expect("establish over iroh");
    let listed = projects.list().await.expect("list projects as Alice");
    // The example studio plants no project pages — empty is the seeded
    // truth. The claim is the call round-tripped through the gate.
    assert!(listed.is_empty(), "the seeded vault has no project pages");

    // And the identity genuinely rode along: a second client over the
    // SAME cached connection makes a call the gate refuses to anonymous
    // callers. Creating a project is member-gated.
    let made = projects
        .create(project::ProjectInfo {
            title: "Dialled In".into(),
            ..Default::default()
        })
        .await
        .expect("create a project as Alice over iroh");
    assert_eq!(made.title, "Dialled In");
}
