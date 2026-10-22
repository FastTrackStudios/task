//! Chapter — a session travels with its media, and with nothing else.
//!
//! `files.sync.selective` ends on a promise: "a session arrives with the
//! media it references, because one that streams in on first play will
//! glitch." Everything before it is about facets; this chapter is about
//! that sentence, on the seeded album's own REAPER sessions.
//!
//! Three machines and three stages:
//!
//! 1. **The studio is up.** ACME's server holds *Example Album* — the
//!    committed tree from `examples/studio`, its project page declaring
//!    music production, plus a folder of footage that belongs to no song.
//!    Two laptops pull it. The mix laptop selects one session, Track
//!    One, by pinning its `.RPP`; the producer's laptop takes only the
//!    mixes. What each disk keeps is the assertion.
//! 2. **The studio is gone.** The server goes off the network. The mix
//!    laptop records a second vocal take into Track One and saves; the
//!    producer's laptop pulls it straight from the mix laptop and
//!    selects the session — and the new take comes with it, because the
//!    session *says* it uses it, while the footage stays a stub.
//! 3. **The producer's laptop restarts.** The selection is still there,
//!    and a fresh pass over the restarted node keeps the same files.
//!
//! The capability half is asserted on the way through: the album's
//! project declares music production only, so Resolve's `Proxies` is a
//! folder nobody's tool made — reported for a decision, not taken to be
//! somebody's proxies.
//!
//! # No second kind of location
//!
//! The card that asked for this chapter asked for a mixed-storage stage
//! "if a second location kind is available in the suite". It is not:
//! every root here is a directory on a device's disk, and
//! `files-storage`'s location kinds (removable drives, object stores)
//! are not wired into replica pulls or hydration yet. A stage that
//! pretended otherwise would be testing a fixture.

use std::time::Duration;

use files::RootId;
use files::path::RootPath;
use files::service::sync::{FacetName, FacetSource, SyncService};
use files::service::tree::TreeService;
use files::service::version::VersionService;

use integration::device::Laptop;
use integration::orgs::adopt;
use integration::server::Server;

/// The album's directory, in the server's files and in the commit.
const ALBUM: &str = "Example Album";
const SESSION: &str = "Track One/Track One.RPP";
/// Footage that belongs to no song — what a laptop selecting a session
/// must not be handed.
const FOOTAGE: &str = "Footage/BTS/studio-cam.mov";
const FOOTAGE_BYTES: usize = 64 * 1024;

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a root path")
}

/// The committed *Example Album*, copied byte for byte.
fn committed_album() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/studio/acme-audio/Projects")
        .join(ALBUM)
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read the committed tree") {
        let entry = entry.expect("entry");
        let dest = to.join(entry.file_name());
        if entry.file_type().expect("type").is_dir() {
            copy_tree(&entry.path(), &dest);
        } else {
            std::fs::copy(entry.path(), dest).expect("copy");
        }
    }
}

/// The album as ACME's studio holds it: the committed tree, the page
/// the seeder declares it with, and footage nobody's song uses.
fn studio_album(files: &std::path::Path) {
    let album = files.join(ALBUM);
    copy_tree(&committed_album(), &album);

    // The same declaration `admin demo` writes for this project, taken
    // from the seed rather than restated, so the chapter and the planted
    // world cannot disagree about what the album is.
    let declared = task_server::example_org::DECLARED
        .iter()
        .find(|d| d.org == "acme-audio" && d.dir == ALBUM)
        .expect("the seed declares Example Album");
    std::fs::write(
        album.join("project.md"),
        format!(
            "---\ntype: project\ntitle: {ALBUM}\ncapabilities: [{}]\n---\n\n# {ALBUM}\n",
            declared.capabilities.join(", ")
        ),
    )
    .expect("declare the album");

    for (path, bytes) in [
        (FOOTAGE, vec![7u8; FOOTAGE_BYTES]),
        ("Proxies/teaser.mov", b"a teaser cut".to_vec()),
    ] {
        let full = album.join(path);
        std::fs::create_dir_all(full.parent().expect("parent")).expect("mkdir");
        std::fs::write(full, bytes).expect("write");
    }
}

/// Is `path` a stub on `backend` right now?
async fn is_stub(backend: &files::FilesBackend, root: RootId, path: &str) -> bool {
    let p = rp(path);
    let parent = p.parent().unwrap_or_else(RootPath::root);
    let name = p.name().expect("a named path").to_string();
    TreeService::browse(backend, root, parent)
        .await
        .expect("browse")
        .into_iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{path} vanished from the listing"))
        .stub
}

/// Assert which of `paths` are resident and which are stubs.
async fn keeps(laptop: &Laptop, root: RootId, resident: &[&str], stubs: &[&str], who: &str) {
    for path in resident {
        assert!(
            !is_stub(&laptop.backend, root, path).await,
            "{who}: {path} should be resident"
        );
    }
    for path in stubs {
        assert!(
            is_stub(&laptop.backend, root, path).await,
            "{who}: {path} should be a stub"
        );
    }
}

/// Pull `root` from `origin` onto `laptop` over the replica lane.
async fn pull(laptop: &Laptop, origin: &iroh::Endpoint, root: RootId) {
    let peer = laptop.dial_replica(origin).await;
    files_sync::reconcile(&laptop.backend, &peer, root.get())
        .await
        .expect("pull the album");
}

/// The whole chapter, in order — see the module note.
// t[verify files.sync.selective] — a selected session arrives with
// exactly the media it references, over the wire and between two devices
// with no host
// t[verify files.device.control] — the selection is a pin, and it
// survives the device's restart
// t[verify files.facet.project-override] — a folder the declared tools
// did not make is reported, not guessed at
// t[verify project.capability.conventions] — the root holds the work its
// own project page declares, on every device that synced it
// t[verify files.topology.multi-server] — the new take moves laptop to
// laptop with the server off the network
#[tokio::test(flavor = "multi_thread")]
async fn a_selected_session_brings_its_media_with_or_without_the_studio() {
    // ── 1. The studio is up ────────────────────────────────────────
    let studio = Server::start("ACME Audio", "acme-audio", studio_album).await;
    let root = adopt(&studio, ALBUM).await;
    integration::scenario::pin(&studio, root, "the album as the studio left it").await;

    let mix = Laptop::empty_peer(root).await;
    let mut producer = Laptop::empty_peer(root).await;
    for (laptop, name) in [(&mix, "mix laptop"), (&producer, "producer's laptop")] {
        // Paired the way the app pairs: the org admits the machine's
        // endpoint, and that endpoint is the whole credential.
        SyncService::enroll_device(
            &studio.backend,
            laptop.endpoint.id().to_string(),
            name.into(),
        )
        .await
        .expect("enrol");
        pull(laptop, &studio.endpoint, root).await;
    }

    // The mix laptop selects one session.
    mix.backend
        .pin(root, vec![rp(SESSION)], true)
        .await
        .expect("select Track One");
    keeps(
        &mix,
        root,
        &[
            SESSION,
            "Track One/Audio Files/Vox_1.wav",
            "Track One/Audio Files/Drums_1.wav",
            "project.md",
        ],
        &[
            "Track Two/Track Two.RPP",
            "Track Two/Audio Files/Vox_1.wav",
            "Track One/Renders/Track One.wav",
            "Inbox/rough.wav",
            FOOTAGE,
            "Proxies/teaser.mov",
        ],
        "mix laptop",
    )
    .await;
    assert!(
        std::fs::metadata(mix.tree.join(FOOTAGE)).map_or(0, |m| m.len()) < FOOTAGE_BYTES as u64,
        "the footage still costs its full size on the mix laptop's disk"
    );

    let unit = mix
        .backend
        .session_media(root, rp(SESSION))
        .await
        .expect("what Track One brings");
    assert!(
        unit.parsed,
        "a REAPER session is read, not guessed from its folder"
    );
    assert_eq!(
        unit.media,
        vec![
            rp("Track One/Audio Files/Drums_1.wav"),
            rp("Track One/Audio Files/Vox_1.wav")
        ]
    );
    assert!(
        unit.missing.is_empty() && unit.outside.is_empty(),
        "{unit:?}"
    );

    // The capabilities came with the root: the laptop read them from the
    // page it pulled, and they are music production only.
    let facets = mix.backend.facets(root).await.expect("facets");
    let source = |path: &str| {
        facets
            .iter()
            .find(|b| b.path.as_str() == path)
            .unwrap_or_else(|| panic!("{path} not reported: {facets:?}"))
            .source
    };
    assert_eq!(source("Track One/Audio Files"), FacetSource::ToolLayout);
    assert_eq!(source("Track One/Renders"), FacetSource::ToolLayout);
    for unmapped in ["Proxies", "Footage", "Inbox"] {
        assert_eq!(
            source(unmapped),
            FacetSource::Unmapped,
            "{unmapped}: a music project's folder nobody's tool made is reported, never guessed"
        );
    }

    // The producer takes the mixes and no session.
    producer
        .backend
        .subscribe(root, vec![FacetName("mixes".into())])
        .await
        .expect("take the mixes");
    keeps(
        &producer,
        root,
        &["Track One/Renders/Track One.wav", "project.md"],
        &[SESSION, "Track One/Audio Files/Vox_1.wav", FOOTAGE],
        "producer's laptop",
    )
    .await;

    // ── 2. The studio is gone ──────────────────────────────────────
    studio.go_offline().await;
    let reached = tokio::time::timeout(
        Duration::from_secs(5),
        architect::iroh_link::connect(&mix.endpoint, studio.endpoint.id()),
    )
    .await;
    assert!(
        !matches!(reached, Ok(Ok(_))),
        "the studio is supposed to be off the network"
    );

    // A second vocal take, recorded into Track One on the mix laptop
    // and placed in the session, then saved.
    std::fs::write(
        mix.tree.join("Track One/Audio Files/Vox_2.wav"),
        b"second vocal take, recorded on the plane",
    )
    .expect("record");
    let session_path = mix.tree.join(SESSION);
    let edited = std::fs::read_to_string(&session_path)
        .expect("read the session")
        .replacen(
            "  <TRACK\n    NAME Drums",
            "  <TRACK\n    NAME Vox 2\n    <ITEM\n      <SOURCE WAVE\n        FILE \"Audio Files/Vox_2.wav\"\n      >\n    >\n  >\n  <TRACK\n    NAME Drums",
            1,
        );
    assert!(
        edited.contains("Vox_2.wav"),
        "fixture: the session did not take the edit"
    );
    std::fs::write(&session_path, edited).expect("save the session");
    VersionService::checkpoint(&mix.backend, root, Some("second vocal take".into()))
        .await
        .expect("save point on the mix laptop");

    // The producer pulls from the mix laptop directly, and selects the
    // session.
    mix.backend
        .admit_host(producer.host_id(), files_domain::Hosting::working());
    pull(&producer, &mix.endpoint, root).await;
    producer
        .backend
        .pin(root, vec![rp(SESSION)], true)
        .await
        .expect("the producer selects Track One");
    keeps(
        &producer,
        root,
        &[
            SESSION,
            "Track One/Audio Files/Vox_1.wav",
            "Track One/Audio Files/Vox_2.wav",
            "Track One/Audio Files/Drums_1.wav",
            "Track One/Renders/Track One.wav",
        ],
        &[
            "Track Two/Audio Files/Vox_1.wav",
            FOOTAGE,
            "Inbox/rough.wav",
        ],
        "producer's laptop, no studio",
    )
    .await;
    assert_eq!(
        std::fs::read(producer.tree.join("Track One/Audio Files/Vox_2.wav")).expect("the new take"),
        b"second vocal take, recorded on the plane",
        "the take recorded with no studio reached the other laptop"
    );

    // ── 3. The producer's laptop restarts ──────────────────────────
    // The state file is the proof that the selection was written down:
    // a backend reopened in this process shares the lane's in-memory
    // cache, so only the disk can say what a real restart would find.
    let state = std::fs::read_to_string(producer.data_dir().join("sync.json"))
        .expect("the lane's state file");
    assert!(
        state.contains(SESSION.rsplit('/').next().expect("name")),
        "the selection never reached the disk: {state}"
    );
    producer.reboot().await;
    let restarted = &producer.backend;
    let sub = restarted
        .subscription(root)
        .await
        .expect("own subscription");
    assert!(sub.pinned.contains(&rp(SESSION)), "{sub:?}");
    assert_eq!(sub.facets, vec![FacetName("mixes".into())]);
    // A fresh pass over the restarted node: the references are read off
    // the session again, so the same files stay.
    restarted
        .pin(root, Vec::new(), true)
        .await
        .expect("re-apply after the restart");
    for (path, stub) in [
        ("Track One/Audio Files/Vox_2.wav", false),
        ("Track One/Audio Files/Vox_1.wav", false),
        (FOOTAGE, true),
        ("Track Two/Audio Files/Vox_1.wav", true),
    ] {
        assert_eq!(
            is_stub(restarted, root, path).await,
            stub,
            "after the restart: {path}"
        );
    }
}
