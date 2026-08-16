//! `SyncService` — facets, ignoring, selective sync and devices, against
//! a real `FilesBackend`.
//!
//! In-process rather than over `LocalServer`, for the same reason
//! `roots_lane.rs` is: the lane has no `permits.rs` rows yet, so it is
//! not mounted on a router and cannot be reached over vox. These call the
//! trait directly, which is what the dispatcher would do.
//!
//! The tree staged below is the shape the real one has — a Pro Tools
//! session folder beside directories no tool created — because the whole
//! point of `files.facet.*` is that the first is recognised without
//! configuration and the second is reported rather than guessed at.

use files::FilesBackend;
use files_proto::id::{DeviceId, RootId};
use files_proto::model::RootFlavor;
use files_proto::service::legacy::FilesService;
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::service::sync::{FacetName, FacetSource, SyncService};
use files_proto::{FilesFault, RootPath};

/// An album root: one session with its media, some footage, and a
/// directory that is a convention of this job rather than of any tool.
async fn album(name: &str) -> (tempfile::TempDir, FilesBackend, RootId) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join(name);

    for sub in [
        "01 Song/Audio Files",
        "01 Song/Bounced Files",
        "Proxies",
        "Project Assembly",
    ] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("01 Song/01 Song.RPP"), b"reaper project").unwrap();
    std::fs::write(dir.join("01 Song/Audio Files/kick.wav"), b"boom boom boom").unwrap();
    std::fs::write(dir.join("01 Song/Bounced Files/rough.wav"), b"rough mix").unwrap();
    std::fs::write(dir.join("Proxies/reel.mov"), b"footage bytes here").unwrap();
    std::fs::write(dir.join("Project Assembly/click.wav"), b"click track").unwrap();
    // Junk both layers must swallow.
    std::fs::write(dir.join(".DS_Store"), b"finder").unwrap();
    std::fs::write(dir.join("01 Song/01 Song.rpp-bak"), b"reaper backup").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: name.to_string(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .expect("adopt");

    // Selective sync can only release bytes the store already holds, so
    // a root that has never been checkpointed can never be stubbed.
    FilesService::checkpoint_now(&backend, root.id, Some("staged".into()))
        .await
        .expect("checkpoint");

    (data_dir, backend, RootId::new(root.id))
}

/// Is this path a pointer stub right now?
async fn is_stub(backend: &FilesBackend, root: RootId, path: &str) -> bool {
    let p = RootPath::parse(path).expect("path");
    let parent = p.parent().unwrap_or_else(RootPath::root);
    let name = p.name().expect("a named path").to_string();
    FilesService::browse(backend, root.get(), parent.to_string())
        .await
        .expect("browse")
        .into_iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{path} is missing from the listing entirely"))
        .stub
}

// t[verify files.sync.selective]
#[tokio::test(flavor = "multi_thread")]
async fn a_mix_engineer_takes_sessions_and_leaves_footage_as_stubs() {
    let (_tmp, backend, root) = album("mix").await;

    let sub = backend
        .subscribe(root, vec![FacetName("sessions".into())])
        .await
        .expect("subscribe");
    assert_eq!(sub.facets, vec![FacetName("sessions".into())]);

    assert!(
        !is_stub(&backend, root, "01 Song/Audio Files/kick.wav").await,
        "a session's media is resident — one that streams in on first play glitches"
    );
    assert!(
        is_stub(&backend, root, "Proxies/reel.mov").await,
        "footage is not subscribed, so it is a stub"
    );
    assert!(
        is_stub(&backend, root, "01 Song/Bounced Files/rough.wav").await,
        "and so are the mixes — a facet is subscribed to, not implied by its neighbours"
    );

    // The whole rule: unsubscribed is a stub, never absent.
    for path in ["Proxies/reel.mov", "01 Song/Bounced Files/rough.wav"] {
        let disk = std::path::Path::new(&FilesService::get_root(&backend, root.get())
            .await
            .expect("root")
            .path)
            .join(path);
        assert!(disk.exists(), "{path} must still be present, sized and browsable");
    }
}

// t[verify files.sync.selective]
#[tokio::test(flavor = "multi_thread")]
async fn unsubscribing_returns_the_space_without_losing_the_file() {
    let (_tmp, backend, root) = album("resub").await;

    backend
        .subscribe(root, vec![FacetName("sessions".into())])
        .await
        .expect("subscribe");
    assert!(!is_stub(&backend, root, "01 Song/Audio Files/kick.wav").await);

    backend.subscribe(root, Vec::new()).await.expect("unsubscribe");
    assert!(
        is_stub(&backend, root, "01 Song/Audio Files/kick.wav").await,
        "content leaving the subscription becomes a stub at the moment it leaves"
    );

    let back = SyncService::hydrate(
        &backend,
        root,
        vec![RootPath::parse("01 Song/Audio Files/kick.wav").unwrap()],
        true,
    )
    .await
    .expect("hydrate");
    assert_eq!(back.len(), 1);
    assert!(!is_stub(&backend, root, "01 Song/Audio Files/kick.wav").await);
}

// t[verify files.device.control]
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_beats_an_unsubscribed_facet() {
    let (_tmp, backend, root) = album("pinned").await;

    backend
        .subscribe(root, vec![FacetName("sessions".into())])
        .await
        .expect("subscribe");
    assert!(is_stub(&backend, root, "Proxies/reel.mov").await);

    let reel = RootPath::parse("Proxies/reel.mov").unwrap();
    let sub = backend.pin(root, vec![reel.clone()], true).await.expect("pin");
    assert_eq!(sub.pinned, vec![reel.clone()]);
    assert!(
        !is_stub(&backend, root, "Proxies/reel.mov").await,
        "a pin is the manual override — it must not be overridable by a facet the device does not take"
    );

    // And it stays pinned across an unrelated subscription change: the
    // pin is part of the subscription, not a cache hint.
    backend
        .subscribe(root, vec![FacetName("sessions".into())])
        .await
        .expect("resubscribe");
    assert!(!is_stub(&backend, root, "Proxies/reel.mov").await);

    backend.pin(root, vec![reel], false).await.expect("unpin");
    assert!(
        is_stub(&backend, root, "Proxies/reel.mov").await,
        "unpinning returns it to the subscription"
    );
}

// t[verify files.facet.tool-layout]
// t[verify files.facet.project-override]
#[tokio::test(flavor = "multi_thread")]
async fn unmapped_directories_are_reported_rather_than_guessed() {
    let (_tmp, backend, root) = album("facets").await;
    let bindings = backend.facets(root).await.expect("facets");

    let find = |path: &str| {
        bindings
            .iter()
            .find(|b| b.path.as_str() == path)
            .unwrap_or_else(|| panic!("{path} was not reported at all: {bindings:?}"))
            .clone()
    };

    // The capability knows Pro Tools' and Reaper's layouts. Nobody
    // configured this.
    let audio = find("01 Song/Audio Files");
    assert_eq!(audio.source, FacetSource::ToolLayout);
    assert_eq!(audio.facet, Some(FacetName("sessions".into())));
    assert!(audio.atomic, "a session brings the media it references");

    let bounces = find("01 Song/Bounced Files");
    assert_eq!(bounces.facet, Some(FacetName("mixes".into())));
    assert!(!bounces.atomic);

    // Conventions of this particular job. Reported for a decision —
    // never hidden, and never guessed at.
    for path in ["Proxies", "Project Assembly"] {
        let b = find(path);
        assert_eq!(b.source, FacetSource::Unmapped, "{path}");
        assert_eq!(b.facet, None, "{path}: guessing is how footage lands on the wrong tier");
    }

    // A classified directory's children are not decisions anyone owes.
    assert!(
        !bindings.iter().any(|b| b.path.as_str().starts_with("01 Song/Audio Files/")),
        "a tool directory classifies its whole subtree"
    );
}

// t[verify files.facet.project-override]
#[tokio::test(flavor = "multi_thread")]
async fn the_project_maps_what_no_tool_made_and_it_becomes_subscribable() {
    let (_tmp, backend, root) = album("mapped").await;

    let binding = backend
        .map_facet(
            root,
            RootPath::parse("Project Assembly").unwrap(),
            FacetName("assembly".into()),
        )
        .await
        .expect("map_facet");
    assert_eq!(binding.source, FacetSource::Project);
    assert_eq!(binding.facet, Some(FacetName("assembly".into())));

    backend
        .subscribe(root, vec![FacetName("assembly".into())])
        .await
        .expect("subscribe");
    assert!(
        !is_stub(&backend, root, "Project Assembly/click.wav").await,
        "a project mapping is subscribable exactly like a tool layout"
    );
    assert!(is_stub(&backend, root, "01 Song/Audio Files/kick.wav").await);
}

#[tokio::test(flavor = "multi_thread")]
async fn an_empty_facet_name_is_refused() {
    let (_tmp, backend, root) = album("badfacet").await;
    let err = backend
        .map_facet(
            root,
            RootPath::parse("Proxies").unwrap(),
            FacetName("  ".into()),
        )
        .await
        .expect_err("an empty name is not a name");
    assert!(matches!(err, FilesFault::Invalid(_)));
}

// t[verify files.ignore.layers]
// t[verify files.ignore.retained]
#[tokio::test(flavor = "multi_thread")]
async fn the_three_ignore_layers_are_reported_and_only_one_is_settable() {
    let (_tmp, backend, root) = album("ignores").await;

    let set = SyncService::ignore_set(&backend, root)
        .await
        .expect("ignore_set");
    assert!(set.platform.iter().any(|p| p == ".DS_Store"));
    assert!(
        set.capability.iter().any(|p| p == "*.rpp-bak"),
        "a music capability carries Reaper's leavings: {:?}",
        set.capability
    );
    assert!(set.project.is_empty());

    let updated = backend
        .set_project_ignores(root, vec!["*.tmp".to_string()])
        .await
        .expect("set_project_ignores");
    assert_eq!(updated.project, vec!["*.tmp".to_string()]);
    assert!(
        updated.platform.iter().any(|p| p == ".DS_Store"),
        "the project layer cannot defeat the platform one"
    );

    // Ignored is not deleted: the junk is still exactly where it was.
    let path = FilesService::get_root(&backend, root.get())
        .await
        .expect("root")
        .path;
    assert!(std::path::Path::new(&path).join(".DS_Store").exists());
    assert!(std::path::Path::new(&path).join("01 Song/01 Song.rpp-bak").exists());
}

// t[verify files.device.control]
#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_device_stays_known_and_stops_transferring() {
    let (_tmp, backend, _root) = album("devices").await;

    let devices = backend.devices().await.expect("devices");
    let me = devices.first().expect("this process registers itself").id;
    assert!(!devices.iter().any(|d| d.revoked && d.id == me));

    let revoked = backend.revoke_device(me).await.expect("revoke");
    assert!(revoked.revoked);
    assert!(
        revoked.transfer.paused,
        "a revoked device transfers nothing while it waits to be told to wipe"
    );

    // Still listed. A deleted row is a device that reconnects as a
    // stranger and is let back in.
    assert!(
        backend
            .devices()
            .await
            .unwrap()
            .iter()
            .any(|d| d.id == me && d.revoked),
        "revocation is a state, not a deletion"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_device_is_a_typed_fault() {
    let (_tmp, backend, _root) = album("ghostdev").await;
    let ghost = DeviceId::generate();
    match backend.revoke_device(ghost).await.expect_err("no such device") {
        FilesFault::DeviceNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected DeviceNotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_root_is_a_typed_fault() {
    let (_tmp, backend, _root) = album("ghostroot").await;
    let ghost = RootId::generate();
    match backend.facets(ghost).await.expect_err("no such root") {
        FilesFault::RootNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected RootNotFound, got {other:?}"),
    }
}

/// A device that has said nothing takes nothing — and "nothing" means
/// every path is a stub, not that any path is missing.
#[tokio::test(flavor = "multi_thread")]
async fn a_silent_device_has_an_empty_subscription_rather_than_none() {
    let (_tmp, backend, root) = album("silent").await;
    let sub = backend.subscription(root).await.expect("subscription");
    assert_eq!(sub.root_id, root);
    assert!(sub.facets.is_empty());
    assert!(sub.pinned.is_empty());
}
