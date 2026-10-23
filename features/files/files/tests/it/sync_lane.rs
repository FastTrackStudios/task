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
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::service::sync::{FacetName, FacetSource, SyncService};
use files_proto::service::tree::TreeService;
use files_proto::service::version::VersionService;
use files_proto::{FilesFault, RootPath};

/// A collaborative album's project page: music production and video
/// production at once, which is what makes both tools' layouts apply.
const BOTH: &str = "---\ntype: project\ntitle: Album\ncapabilities: [music-production, video-production]\n---\n\n# Album\n";

/// An album root: one session with its media, some footage, and a
/// directory that is a convention of this job rather than of any tool.
/// Its project declares both kinds of work.
async fn album(name: &str) -> (tempfile::TempDir, FilesBackend, RootId) {
    album_declaring(name, Some(BOTH)).await
}

/// [`album`], with `page` as its `project.md` — or none at all.
async fn album_declaring(
    name: &str,
    page: Option<&str>,
) -> (tempfile::TempDir, FilesBackend, RootId) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join(name);

    for sub in [
        "01 Song/Audio Files",
        "01 Song/Bounced Files",
        "Proxies",
        "Project Assembly",
        // A second unmapped directory, so the "reported for a decision"
        // case does not rest on one name that a future tool table might
        // legitimately claim.
        "Client Notes",
    ] {
        std::fs::create_dir_all(dir.join(sub)).unwrap();
    }
    std::fs::write(dir.join("01 Song/01 Song.RPP"), b"reaper project").unwrap();
    std::fs::write(dir.join("01 Song/Audio Files/kick.wav"), b"boom boom boom").unwrap();
    std::fs::write(dir.join("01 Song/Bounced Files/rough.wav"), b"rough mix").unwrap();
    std::fs::write(dir.join("Proxies/reel.mov"), b"footage bytes here").unwrap();
    std::fs::write(dir.join("Project Assembly/click.wav"), b"click track").unwrap();
    std::fs::write(dir.join("Client Notes/brief.md"), b"# brief").unwrap();
    if let Some(page) = page {
        std::fs::write(dir.join("project.md"), page).unwrap();
    }
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
    let root = RootId::new(root.id);
    backend.settled(root).await;

    // Selective sync can only release bytes the store already holds, so
    // a root that has never been checkpointed can never be stubbed.
    VersionService::checkpoint(&backend, root, Some("staged".into()))
        .await
        .expect("checkpoint");

    (data_dir, backend, root)
}

/// Is this path a pointer stub right now?
async fn is_stub(backend: &FilesBackend, root: RootId, path: &str) -> bool {
    let p = RootPath::parse(path).expect("path");
    let parent = p.parent().unwrap_or_else(RootPath::root);
    let name = p.name().expect("a named path").to_string();
    TreeService::browse(backend, root, parent)
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
        let disk = RootsService::get(&backend, root)
            .await
            .expect("root")
            .local_tree()
            .expect("a placed root")
            .join(path);
        assert!(
            disk.exists(),
            "{path} must still be present, sized and browsable"
        );
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

    backend
        .subscribe(root, Vec::new())
        .await
        .expect("unsubscribe");
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
    let sub = backend
        .pin(root, vec![reel.clone()], true)
        .await
        .expect("pin");
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

    // Resolve's layouts apply because this album's project says so: a
    // collaborative album is music production and video production at
    // once, its page declares both, and `Proxies` is a tool directory
    // rather than a convention of this job.
    let proxies = find("Proxies");
    assert_eq!(proxies.source, FacetSource::ToolLayout);
    assert_eq!(proxies.facet, Some(FacetName("proxies".into())));

    // Conventions of this particular job — named by nobody's tool.
    // Reported for a decision, never hidden and never guessed at.
    for path in ["Project Assembly", "Client Notes"] {
        let b = find(path);
        assert_eq!(b.source, FacetSource::Unmapped, "{path}");
        assert_eq!(
            b.facet, None,
            "{path}: guessing is how footage lands on the wrong tier"
        );
    }

    // A classified directory's children are not decisions anyone owes.
    assert!(
        !bindings
            .iter()
            .any(|b| b.path.as_str().starts_with("01 Song/Audio Files/")),
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
    let path = RootsService::get(&backend, root).await.expect("root").path;
    assert!(
        std::path::Path::new(path.as_deref().expect("a placed root"))
            .join(".DS_Store")
            .exists()
    );
    assert!(
        std::path::Path::new(path.as_deref().expect("a placed root"))
            .join("01 Song/01 Song.rpp-bak")
            .exists()
    );
}

// t[verify files.device.control]
// t[verify storage.tier.observed] — device registrations are
// machine-observed facts in a store, not markdown: a revoked device stays
// *known*, which is a record nobody authored and nobody would want in a
// note
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
    match backend
        .revoke_device(ghost)
        .await
        .expect_err("no such device")
    {
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

/// `files.device.control` says a pin survives a restart, in as many
/// words: the whole point of pinning a reel is that it is still there
/// when the laptop comes back up on the plane.
// t[verify files.device.control]
#[tokio::test(flavor = "multi_thread")]
async fn a_pin_survives_a_restart() {
    let (tmp, backend, root) = album("restart").await;
    let reel = RootPath::parse("Proxies/reel.mov").unwrap();

    backend
        .subscribe(root, vec![FacetName("sessions".into())])
        .await
        .expect("subscribe");
    backend
        .pin(root, vec![reel.clone()], true)
        .await
        .expect("pin");
    drop(backend);

    // The state file is the proof. A second backend in THIS process
    // shares the lane's in-memory cache, so reading through it alone
    // would answer from memory and say nothing about durability — and
    // `files.device.control` is a claim about what survives a real
    // restart, which only the disk can settle.
    let on_disk = std::fs::read_to_string(tmp.path().join("sync.json"))
        .expect("the lane must have written its state to the org's data dir");
    assert!(
        on_disk.contains("reel.mov"),
        "a pin must reach the disk, not just the cache: {on_disk}"
    );

    // And it still reads back through the lane.
    let second = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("restart");
    let sub = second.subscription(root).await.expect("subscription");
    assert!(
        sub.pinned.contains(&reel),
        "a pin a human set must outlive the process that recorded it: {sub:?}"
    );
    assert_eq!(
        sub.facets,
        vec![FacetName("sessions".into())],
        "and so must the subscription the pin overrides"
    );
}

/// One org's devices are not another's.
///
/// The lane state used to be a process-wide `static`, but `FilesBackend`
/// is constructed once per org — so `devices()` was answering with every
/// org's machines, and revoking one org's laptop revoked it everywhere.
///
/// The same laptop is enrolled in both orgs (the same endpoint, which is
/// the same machine), so the only thing separating its two rows is the
/// org each lives in.
#[tokio::test(flavor = "multi_thread")]
async fn one_orgs_devices_are_not_anothers() {
    let (_a_tmp, a, _a_root) = album("alpha").await;
    let (_b_tmp, b, _b_root) = album("beta").await;
    let endpoint = "4444444444444444444444444444444444444444444444444444444444444444";

    let me = a
        .enroll_device(endpoint.into(), "laptop".into())
        .await
        .expect("enrol in alpha")
        .id;
    let there = b
        .enroll_device(endpoint.into(), "laptop".into())
        .await
        .expect("enrol in beta")
        .id;
    a.revoke_device(me).await.expect("revoke in alpha");
    a.set_transfer_policy(
        me,
        files_proto::service::sync::TransferPolicy {
            max_bytes_per_sec: Some(1),
            paused: true,
        },
    )
    .await
    .expect("throttle in alpha");

    let elsewhere = b.devices().await.expect("beta devices");
    let there = elsewhere
        .iter()
        .find(|d| d.id == there)
        .expect("the same machine talks to both orgs");
    assert!(
        !there.revoked,
        "revoking a device in one org must not revoke it in another: {elsewhere:?}"
    );
    assert_eq!(
        there.transfer.max_bytes_per_sec, None,
        "nor must one org's throttle be another's"
    );
    assert!(
        b.admits(&files_domain::HostId(endpoint.into())).is_some(),
        "and beta still admits it"
    );
    assert!(
        a.devices()
            .await
            .unwrap()
            .iter()
            .any(|d| d.id == me && d.revoked),
        "and the org that did revoke it still has"
    );
}

/// Pairing, from the org's side: a member's machine hands over the
/// endpoint id it minted, and the org admits it.
///
/// The assertion that matters is the second one. A device row is a
/// record; the *admitted set* is what a pull is actually gated on, so
/// enrolment that wrote a row and left admission alone would produce a
/// device the org can see, believes it enrolled, and refuses.
// t[verify files.device.identity]
#[tokio::test]
async fn enrolling_a_device_admits_the_endpoint_it_presents() {
    let (_dir, backend, _root) = album("Album").await;
    let endpoint = "8badf00d8badf00d8badf00d8badf00d8badf00d8badf00d8badf00d8badf00d";

    let device = backend
        .enroll_device(endpoint.into(), "cody's macbook".into())
        .await
        .expect("enrol");
    assert_eq!(device.endpoint.as_deref(), Some(endpoint));
    assert!(!device.revoked);
    assert!(
        backend
            .admits(&files_domain::HostId(endpoint.into()))
            .is_some(),
        "the enrolled machine was recorded but never admitted, so it cannot sync"
    );
}

/// Enrolling twice is one machine, not two.
///
/// A laptop that reinstalls the app mints the same endpoint from the same
/// key file. Two rows for it would mean revoking either one leaves it
/// syncing through the other — a revocation that silently does nothing.
#[tokio::test]
async fn re_enrolling_the_same_machine_keeps_one_row() {
    let (_dir, backend, _root) = album("Album").await;
    let endpoint = "1111111111111111111111111111111111111111111111111111111111111111";

    let first = backend
        .enroll_device(endpoint.into(), "laptop".into())
        .await
        .unwrap();
    let again = backend
        .enroll_device(endpoint.into(), "cody's laptop".into())
        .await
        .unwrap();

    assert_eq!(first.id, again.id, "one machine became two devices");
    assert_eq!(again.name, "cody's laptop", "re-enrolling renames it");
    let enrolled: Vec<_> = backend
        .devices()
        .await
        .unwrap()
        .into_iter()
        .filter(|d| d.endpoint.as_deref() == Some(endpoint))
        .collect();
    assert_eq!(enrolled.len(), 1, "{enrolled:?}");
}

/// Revoking cuts the machine off in the only place that gates a pull.
///
/// `files.device.control` says a revoked device is refused sync. The row
/// flag is what a person sees; dismissing its endpoint is what refuses
/// it — and before enrolment carried an endpoint there was nothing to
/// dismiss, so the flag was the whole of it.
// t[verify files.device.control]
#[tokio::test]
async fn revoking_a_device_stops_admitting_its_endpoint() {
    let (_dir, backend, _root) = album("Album").await;
    let endpoint = "2222222222222222222222222222222222222222222222222222222222222222";
    let device = backend
        .enroll_device(endpoint.into(), "laptop".into())
        .await
        .unwrap();

    backend.revoke_device(device.id).await.expect("revoke");

    assert_eq!(
        backend.admits(&files_domain::HostId(endpoint.into())),
        None,
        "a revoked device is still admitted, so it still syncs"
    );
    // And letting it back in is the same call that let it in first —
    // by the same authority, which is a person signed into the org.
    backend
        .enroll_device(endpoint.into(), "laptop".into())
        .await
        .unwrap();
    assert!(
        backend
            .admits(&files_domain::HostId(endpoint.into()))
            .is_some()
    );
}

// ── Legacy `sync.json`: one "this device" row per dead process ──────

/// A transport that is never used. `devices()` only needs the backend to
/// know its own endpoint, and `with_remotes` is how an endpoint is bound.
#[derive(Debug)]
struct NoRemotes;

#[async_trait::async_trait]
impl files::lane::federation::RemoteFiles for NoRemotes {
    async fn browse_offered(
        &self,
        _origin: &files_proto::service::federation::EndpointId,
        _secret: &str,
        _path: &RootPath,
    ) -> Result<Vec<files_proto::model::BrowseEntry>, FilesFault> {
        Err(FilesFault::Io("no remotes in this test".into()))
    }

    async fn read_offered(
        &self,
        _origin: &files_proto::service::federation::EndpointId,
        _secret: &str,
        _path: &RootPath,
    ) -> Result<files_proto::service::media::ByteTicket, FilesFault> {
        Err(FilesFault::Io("no remotes in this test".into()))
    }

    async fn open_relay(
        &self,
        _origin: &files_proto::service::federation::EndpointId,
        _secret: &str,
        _token: &str,
    ) -> Result<files_proto::service::federation::RelayManifest, FilesFault> {
        Err(FilesFault::Io("no remotes in this test".into()))
    }

    async fn fetch_relay(
        &self,
        _origin: &files_proto::service::federation::EndpointId,
        _manifest: &files_proto::service::federation::RelayManifest,
        _range: Option<(u64, u64)>,
        _dest: &mut (dyn tokio::io::AsyncWrite + Unpin + Send),
    ) -> Result<(), FilesFault> {
        Err(FilesFault::Io("no remotes in this test".into()))
    }
}

/// `sync.json` as a server wrote it before this node's id was persisted:
/// the same rows, and no `local` key.
#[derive(facet::Facet)]
#[repr(C)]
struct LegacySyncFile {
    subscriptions: Vec<files_proto::service::sync::Subscription>,
    devices: Vec<files_proto::service::sync::DeviceInfo>,
    mappings: Vec<LegacyMapping>,
}

#[derive(facet::Facet)]
#[repr(C)]
struct LegacyMapping {
    root: RootId,
    facets: std::collections::BTreeMap<String, FacetName>,
}

fn device_row(
    name: &str,
    endpoint: &str,
    last_seen: chrono::DateTime<chrono::Utc>,
) -> files_proto::service::sync::DeviceInfo {
    files_proto::service::sync::DeviceInfo {
        id: DeviceId::generate(),
        name: name.into(),
        endpoint: Some(endpoint.into()),
        last_seen,
        transfer: files_proto::service::sync::TransferPolicy {
            max_bytes_per_sec: None,
            paused: false,
        },
        revoked: false,
    }
}

/// How many device rows `sync.json` on disk holds, and its `local` value
/// as raw JSON text (`null` when absent).
fn on_disk(dir: &std::path::Path) -> (usize, String) {
    let text = std::fs::read_to_string(dir.join("sync.json")).expect("sync.json");
    let json: serde_json::Value = serde_json::from_str(&text).expect("sync.json parses");
    let rows = json["devices"].as_array().expect("devices").len();
    (rows, json["local"].to_string())
}

/// Before this node's id was persisted, every server process wrote a new
/// "this device" row for itself, and prod orgs were left with 189 of them.
/// Loading such a file must collapse them to one: the newest becomes
/// `local`, the rest go with their subscriptions, and peers are untouched.
// t[verify files.device.identity]
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_file_collapses_to_one_row_for_this_node() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let own = "1542542863b2aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let peer_endpoint = "929cb07ebbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    let base = chrono::Utc::now() - chrono::Duration::days(30);

    let legacy: Vec<_> = (0..5)
        .map(|i| device_row("this device", own, base + chrono::Duration::hours(i)))
        .collect();
    let newest = legacy.last().expect("rows").id;
    let peer = device_row("THEBATTLESHIP", peer_endpoint, base);
    let root = RootId::new(uuid::Uuid::new_v4());
    let sub = |device: DeviceId| files_proto::service::sync::Subscription {
        device,
        root_id: root,
        facets: vec![FacetName("sessions".into())],
        pinned: Vec::new(),
    };
    let file = LegacySyncFile {
        // One subscription held by a duplicate (dropped with it), one by
        // the peer (kept).
        subscriptions: vec![sub(legacy[0].id), sub(peer.id)],
        devices: legacy.iter().cloned().chain([peer.clone()]).collect(),
        mappings: Vec::new(),
    };
    std::fs::write(
        tmp.path().join("sync.json"),
        facet_json::to_string(&file).expect("legacy file serialises"),
    )
    .unwrap();

    let open = || {
        FilesBackend::new(tmp.path(), tmp.path().join("vault"))
            .expect("backend")
            .with_remotes(own, std::sync::Arc::new(NoRemotes))
    };
    let backend = open();
    let devices = backend.devices().await.expect("devices");

    let mine: Vec<_> = devices
        .iter()
        .filter(|d| d.endpoint.as_deref() == Some(own))
        .collect();
    assert_eq!(
        mine.len(),
        1,
        "a legacy file must collapse to one row for this node, not keep every dead process's: {mine:?}"
    );
    assert_eq!(
        mine[0].id, newest,
        "the newest legacy row is the one adopted as this node"
    );
    let kept = devices
        .iter()
        .find(|d| d.id == peer.id)
        .expect("a peer's row is not this node's to retire");
    assert_eq!(
        (kept.name.as_str(), kept.endpoint.as_deref(), kept.revoked),
        (peer.name.as_str(), peer.endpoint.as_deref(), peer.revoked),
        "and it is left as it was"
    );
    assert_eq!(devices.len(), 2, "{devices:?}");

    let (rows, local) = on_disk(tmp.path());
    assert!(
        local.contains(&newest.0.to_string()),
        "the adopted row must be persisted as `local`, or the next process adopts again: {local}"
    );
    assert_eq!(rows, 2, "the file itself must shrink");
    let text = std::fs::read_to_string(tmp.path().join("sync.json")).unwrap();
    assert!(
        !text.contains(&legacy[0].id.0.to_string()),
        "a retired row's subscription must go with it: {text}"
    );
    assert!(
        text.contains(&peer.id.0.to_string()),
        "the peer's row and subscription stay"
    );

    // A restart: a second backend over the same directory. It shares this
    // process's lane cache, so the disk checks above are what prove the
    // file; this proves a second open neither grows nor re-mints the row.
    drop(backend);
    let again = open().devices().await.expect("devices after restart");
    let mine: Vec<_> = again
        .iter()
        .filter(|d| d.endpoint.as_deref() == Some(own))
        .map(|d| d.id)
        .collect();
    assert_eq!(
        mine,
        vec![newest],
        "a restart must not grow or re-mint this node's row"
    );
    let (rows, local) = on_disk(tmp.path());
    assert_eq!(rows, 2);
    assert!(local.contains(&newest.0.to_string()), "{local}");
}

// ── Capabilities bound to the root, and a session as one unit ───────

const MUSIC: &str = "---\ntype: project\ntitle: Album\ncapabilities:\n  - music-production\n---\n";

fn binding(bindings: &[files_proto::service::sync::FacetBinding], path: &str) -> FacetSource {
    bindings
        .iter()
        .find(|b| b.path.as_str() == path)
        .unwrap_or_else(|| panic!("{path} was not reported: {bindings:?}"))
        .source
}

/// A root holds the work its own project page declares, not every kind
/// of work a media root could hold.
///
/// The same tree under three pages: a music project does not recognise
/// Resolve's `Proxies` (it reports it, for a decision), a project that
/// declared both does, and a tree with no page recognises nothing at
/// all — not even Pro Tools' `Audio Files` — because nobody said which
/// tool wrote it.
// t[verify project.capability.conventions]
// t[verify files.facet.tool-layout]
#[tokio::test(flavor = "multi_thread")]
async fn a_roots_conventions_are_the_ones_its_project_declares() {
    let (_m, music, music_root) = album_declaring("music", Some(MUSIC)).await;
    let facets = music.facets(music_root).await.expect("facets");
    assert_eq!(
        binding(&facets, "01 Song/Audio Files"),
        FacetSource::ToolLayout
    );
    assert_eq!(
        binding(&facets, "Proxies"),
        FacetSource::Unmapped,
        "a music project is not assumed to be a video one too"
    );
    let ignores = SyncService::ignore_set(&music, music_root).await.unwrap();
    assert!(ignores.capability.iter().any(|p| p == "*.rpp-bak"));

    let (_b, both, both_root) = album("both").await;
    let facets = both.facets(both_root).await.expect("facets");
    assert_eq!(binding(&facets, "Proxies"), FacetSource::ToolLayout);

    let (_n, none, none_root) = album_declaring("undeclared", None).await;
    let facets = none.facets(none_root).await.expect("facets");
    for dir in ["01 Song", "Proxies", "Project Assembly"] {
        assert_eq!(binding(&facets, dir), FacetSource::Unmapped, "{dir}");
    }
    assert!(
        SyncService::ignore_set(&none, none_root)
            .await
            .unwrap()
            .capability
            .is_empty(),
        "no declared work, no tool's droppings hidden"
    );
}

/// A REAPER album: two songs, footage, and a samples folder the first
/// song's session reaches into. Its project declares music production.
async fn reaper_album(name: &str) -> (tempfile::TempDir, FilesBackend, RootId) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join(name);
    let files: &[(&str, &[u8])] = &[
        ("project.md", MUSIC.as_bytes()),
        (
            "Track One/Track One.RPP",
            b"<REAPER_PROJECT 0.1 \"7.09\" 1\n  <TRACK\n    <ITEM\n      <SOURCE WAVE\n        FILE \"Audio Files/Vox_1.wav\"\n      >\n    >\n    <ITEM\n      <SOURCE WAVE\n        FILE \"../Shared Samples/clap.wav\"\n      >\n    >\n    <ITEM\n      <SOURCE WAVE\n        FILE \"Audio Files/Vox_2.wav\"\n      >\n    >\n    <ITEM\n      <SOURCE WAVE\n        FILE \"/Volumes/Old Drive/stray.wav\"\n      >\n    >\n  >\n>\n".as_slice(),
        ),
        ("Track One/Audio Files/Vox_1.wav", b"track one vocal".as_slice()),
        ("Track One/Audio Files/Unused take.wav", b"never placed in the session".as_slice()),
        ("Track Two/Track Two.RPP", b"<REAPER_PROJECT 0.1 \"7.09\" 1\n  FILE \"Audio Files/Vox_1.wav\"\n>\n".as_slice()),
        ("Track Two/Audio Files/Vox_1.wav", b"track two vocal".as_slice()),
        ("Shared Samples/clap.wav", b"clap".as_slice()),
        ("Footage/cam-a.mov", b"unrelated footage".as_slice()),
    ];
    for (path, bytes) in files {
        let full = dir.join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, bytes).unwrap();
    }
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
    let root = RootId::new(root.id);
    backend.settled(root).await;
    VersionService::checkpoint(&backend, root, Some("staged".into()))
        .await
        .expect("checkpoint");
    (data_dir, backend, root)
}

/// Selecting one session — pinning its file — keeps the session and
/// exactly what it references: its own take, and a sample in a folder
/// no tool layout names. The other song, the unused take beside it and
/// the footage stay stubs, and the project page stays put.
// t[verify files.sync.selective]
// t[verify files.device.control]
#[tokio::test(flavor = "multi_thread")]
async fn selecting_a_session_keeps_it_and_its_media_and_nothing_else() {
    let (_tmp, backend, root) = reaper_album("select").await;
    let session = RootPath::parse("Track One/Track One.RPP").unwrap();

    backend
        .pin(root, vec![session.clone()], true)
        .await
        .expect("select the session");

    for kept in [
        "Track One/Track One.RPP",
        "Track One/Audio Files/Vox_1.wav",
        "Shared Samples/clap.wav",
        "project.md",
    ] {
        assert!(
            !is_stub(&backend, root, kept).await,
            "{kept} must be resident"
        );
    }
    for released in [
        "Track One/Audio Files/Unused take.wav",
        "Track Two/Track Two.RPP",
        "Track Two/Audio Files/Vox_1.wav",
        "Footage/cam-a.mov",
    ] {
        assert!(
            is_stub(&backend, root, released).await,
            "{released} must be a stub"
        );
    }

    let unit = backend
        .session_media(root, session.clone())
        .await
        .expect("session_media");
    assert!(unit.parsed);
    assert_eq!(
        unit.media,
        vec![
            RootPath::parse("Shared Samples/clap.wav").unwrap(),
            RootPath::parse("Track One/Audio Files/Vox_1.wav").unwrap(),
        ]
    );
    assert_eq!(
        unit.missing,
        vec![RootPath::parse("Track One/Audio Files/Vox_2.wav").unwrap()],
        "a take the session names but the root lacks is reported, not discovered in the DAW"
    );
    assert_eq!(
        unit.outside,
        vec!["/Volumes/Old Drive/stray.wav".to_string()]
    );

    // Deselecting returns it all to the subscription, which is empty.
    backend
        .pin(root, vec![session], false)
        .await
        .expect("deselect");
    assert!(is_stub(&backend, root, "Shared Samples/clap.wav").await);
    assert!(is_stub(&backend, root, "Track One/Track One.RPP").await);
    assert!(
        !is_stub(&backend, root, "project.md").await,
        "the page is never released"
    );
}

/// Subscribing to `sessions` keeps every session file — not only the
/// tool directories beside them — and each brings its own media.
// t[verify files.sync.selective]
#[tokio::test(flavor = "multi_thread")]
async fn the_sessions_facet_keeps_session_files_and_what_they_reference() {
    let (_tmp, backend, root) = reaper_album("facet").await;
    backend
        .subscribe(root, vec![FacetName("sessions".into())])
        .await
        .expect("subscribe");
    for kept in [
        "Track One/Track One.RPP",
        "Track Two/Track Two.RPP",
        "Shared Samples/clap.wav",
    ] {
        assert!(
            !is_stub(&backend, root, kept).await,
            "{kept} must be resident"
        );
    }
    assert!(is_stub(&backend, root, "Footage/cam-a.mov").await);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_is_not_a_declared_session_is_refused() {
    let (_tmp, backend, root) = reaper_album("refuse").await;
    let err = backend
        .session_media(root, RootPath::parse("Footage/cam-a.mov").unwrap())
        .await
        .expect_err("footage is not a session");
    assert!(matches!(err, FilesFault::Invalid(_)), "{err:?}");
    let err = backend
        .session_media(root, RootPath::parse("Track Nine/Track Nine.RPP").unwrap())
        .await
        .expect_err("no such session");
    assert!(matches!(err, FilesFault::PathNotFound(_)), "{err:?}");
}

/// A selected session is still selected after a restart, and a fresh
/// pass over the restarted node keeps the same files — the references
/// are read again from the session on disk, not remembered.
// t[verify files.device.control]
#[tokio::test(flavor = "multi_thread")]
async fn a_selected_session_survives_a_restart() {
    let (tmp, backend, root) = reaper_album("restart").await;
    let session = RootPath::parse("Track One/Track One.RPP").unwrap();
    backend
        .pin(root, vec![session.clone()], true)
        .await
        .expect("select");
    // Closed the way a process exit closes it, so the next backend can
    // open the same stores.
    backend.shutdown().await;
    drop(backend);

    let on_disk = std::fs::read_to_string(tmp.path().join("sync.json")).expect("sync.json");
    assert!(on_disk.contains("Track One.RPP"), "{on_disk}");

    let second = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("restart");
    let sub = second.subscription(root).await.expect("subscription");
    assert!(sub.pinned.contains(&session), "{sub:?}");
    // An empty pin is a re-application of what is stored.
    second.pin(root, Vec::new(), true).await.expect("re-apply");
    assert!(!is_stub(&second, root, "Shared Samples/clap.wav").await);
    assert!(is_stub(&second, root, "Footage/cam-a.mov").await);
}
