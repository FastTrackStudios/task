//! A server and a laptop, syncing in both directions over iroh.
//!
//! Everything else that tests this system's replication does it between
//! two backends in one process, or between two daemons
//! (`files-daemon`'s own suite). This is the deployment shape: a real
//! `AppState` with its org router and permit gate on one side, a
//! `SyncDaemon` on the other, and the two halves an operator actually
//! performs — the org admits the device's endpoint id from
//! `admitted-devices`, the device admits the org's.
//!
//! The direction under test is the one the product could not do at all
//! until `device_sync` existed: **the server collecting what the laptop
//! did offline**. The other direction is asserted first, because a test
//! that started with the laptop's work would not prove the laptop ever
//! had the project.

// Every e2e binary compiles the whole of `support`; each uses a subset.
#[allow(dead_code)]
mod support;

use std::time::Duration;

use architect::iroh_link::iroh;
use files::{FilesService as _, RootFlavor};
use files_daemon::SyncDaemon;
use task_server::device_sync;

/// Bind an endpoint into a shared book — the test-process stand-in for
/// the pkarr record a deployed endpoint publishes as it binds.
async fn bind(book: &files::AddressBook) -> iroh::Endpoint {
    let endpoint = files::bind_endpoint(iroh::SecretKey::generate(), Some(book.clone()))
        .await
        .expect("bind an endpoint");
    book.add_endpoint_info(endpoint.addr());
    endpoint
}

// t[verify files.topology.multi-server] — bytes move directly between
// two machines that can reach each other, and a device is one of them
#[tokio::test(flavor = "multi_thread")]
async fn a_laptops_offline_work_reaches_the_server() {
    let (state, _tmp) = support::boot_app_state().await.expect("boot the server");
    let mut org = state.org(support::ORG).expect("the example org");
    let org_root = state.data_root.org(support::ORG).path().to_path_buf();
    let book = files::AddressBook::new();

    // The org's endpoint, serving its whole router — the same call
    // `iroh_host` makes at boot.
    let server_endpoint = bind(&book).await;
    // What `iroh_host::bind_org` does at boot: the backend is told its
    // own endpoint id, which is what `coordinator()` hands to a device.
    task_server::attach_peering(
        &mut org,
        server_endpoint.id().to_string(),
        files::IrohRemotes::port(server_endpoint.clone()),
    );
    {
        let (org, gate, serving) = (
            org.clone(),
            state.write_gate.clone(),
            server_endpoint.clone(),
        );
        tokio::spawn(async move {
            task_server::serve_org_iroh(org, gate, &serving).await;
        });
    }

    // A project on the server, checkpointed so its content is in the
    // store and can be pulled.
    // Inside the org's own files dir: a root outside that boundary is
    // refused, which is the confinement `files` enforces on adoption.
    let project = org_root.join("files").join("Album");
    std::fs::create_dir_all(&project).unwrap();
    std::fs::write(project.join("mix.wav"), b"the rough mix").unwrap();
    let root = org
        .files
        .create_root(
            project.to_string_lossy().into_owned(),
            "Album".into(),
            RootFlavor::Media,
        )
        .await
        .expect("create the root");
    org.files
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint");

    // The laptop: its own store, its own persisted identity, serving
    // its own replica lane.
    let laptop_dir = tempfile::tempdir().expect("laptop dir");
    let laptop_store = files::FilesBackend::new(laptop_dir.path(), laptop_dir.path().join("vault"))
        .expect("laptop backend");
    let laptop =
        SyncDaemon::open(laptop_store, laptop_dir.path().join("daemon")).expect("laptop daemon");
    let laptop_endpoint = bind(&book).await;
    let laptop_id = laptop.attach_endpoint(laptop_endpoint);

    // Pairing, as the app performs it: the machine's endpoint id goes to
    // the org, the org's comes back, and the person's own sign-in is the
    // authority for both. No file is edited and no id is typed.
    let paired = files::service::sync::SyncService::enroll_device(
        &org.files,
        laptop_id.clone(),
        "cody's macbook".into(),
    )
    .await
    .expect("enrol this machine");
    assert_eq!(paired.endpoint.as_deref(), Some(laptop_id.as_str()));

    let coordinator = files::service::sync::SyncService::coordinator(&org.files)
        .await
        .expect("the org's own endpoint id");
    laptop.admit_peer(&coordinator);

    // Down: the laptop takes the album, adopting it from what the
    // server says it holds.
    laptop
        .sync_from_peer(
            &server_endpoint.id().to_string(),
            root.id,
            vec![],
            laptop_dir.path(),
        )
        .await
        .expect("choose the album");
    laptop.tick().await;
    let on_laptop = laptop_dir.path().join("Album").join("mix.wav");
    assert_eq!(
        std::fs::read(&on_laptop).expect("the album never reached the laptop"),
        b"the rough mix"
    );

    // Offline work, and a save point before the lid closes.
    std::fs::write(&on_laptop, b"the final mix").unwrap();
    laptop
        .checkpoint_now(root.id)
        .await
        .expect("checkpoint the laptop's work");

    // Up: the server's own sweep — admission is already on disk, so
    // this is exactly what the timer runs in production.
    tokio::time::timeout(
        Duration::from_secs(30),
        device_sync::sweep(&state, support::ORG, &server_endpoint),
    )
    .await
    .expect("the sweep did not finish");

    assert_eq!(
        std::fs::read(project.join("mix.wav")).expect("read the server's tree"),
        b"the final mix",
        "the laptop's work never reached the server"
    );

    // And the org gate recognised the device the whole way through.
    //
    // This is the assertion a live run is needed to think of. Sync
    // works perfectly with the gate in its default observe-only mode
    // even when the device arrives as `anonymous`, because observe-only
    // records what it would have refused and refuses nothing. The first
    // deployment to set `TASK_ENFORCE_PERMISSIONS=1` would then find
    // that no device can sync at all — and nothing before that moment
    // would have said so.
    //
    // So the check is the symmetric one `connection_identity` uses: an
    // admitted device's pull must leave the would-deny ledger untouched
    // on the replica lane.
    let (denials, _overflow) = task_server::permission_deny_ledger().snapshot();
    let replica_denials: Vec<&str> = denials
        .iter()
        .filter(|d| d.reason.contains("files-replica") || d.reason.contains("files/replica"))
        .map(|d| d.reason.as_str())
        .collect();
    assert!(
        replica_denials.is_empty(),
        "an admitted device was not recognised by the org gate — sync would be refused \
         under TASK_ENFORCE_PERMISSIONS=1:\n{}",
        replica_denials.join("\n")
    );
}
