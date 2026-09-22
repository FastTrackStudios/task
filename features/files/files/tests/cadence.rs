//! The cadence engine (issue #260) end to end over an in-process
//! `architect::LocalServer` — the spec's Testing Decisions primary seam
//! ("assert external behavior at the RPC seam ... advance a virtual
//! clock through debounce/quiescence, then assert what the service
//! reports"), with `TestClock` standing in for the wall clock so a
//! 30-minute quiescence window costs no wall-clock time at all.
//!
//! One acceptance criterion per test, plus the watcher (which is the
//! one part that has to touch a real filesystem to mean anything).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use chrono::TimeDelta;
use files::id::RootId;
use files::service::roots::AdoptRequest;
use files::service::version::VersionEvent;
use files::{
    CadenceConfig, FileRootInfo, FilesBackend, FilesEvent, RootFlavor, RootPath,
    RootsServiceClient, SyncServiceClient, TestClock, VersionServiceClient, ignore,
};
use uuid::Uuid;

/// The v2 lanes the cadence is observed through, mounted as the org
/// router mounts them. No gate stands in front, so every call is the
/// server's own.
fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new()
        .merge(files::roots_layer(backend.clone()))
        .merge(files::version_layer(backend.clone()))
        .merge(files::sync_layer(backend))
}

/// One client per lane.
struct Rpc {
    roots: RootsServiceClient,
    version: VersionServiceClient,
    sync: SyncServiceClient,
}

fn id(root: &FileRootInfo) -> RootId {
    RootId::new(root.id)
}

fn path(p: &str) -> RootPath {
    RootPath::parse(p).expect("valid root path")
}

/// A backend on a `TestClock`, its RPC clients, and a live collector of
/// every event the backend publishes — the whole harness every test
/// below runs on.
///
/// The collector follows the backend's hub in process
/// (`FilesBackend::subscribe_events`, the same stream
/// `TreeService::events` relays) rather than over a vox subscription:
/// subscribing in process is synchronous, so no event can slip past
/// before the subscriber is attached, and what is under test here is
/// the cadence, not the transport.
struct Harness {
    _data_dir: tempfile::TempDir,
    root_dir: std::path::PathBuf,
    backend: FilesBackend,
    client: Rpc,
    clock: Arc<TestClock>,
    events: Arc<Mutex<Vec<FilesEvent>>>,
    scope: std::sync::Arc<Scope>,
    _collector: tokio::task::JoinHandle<()>,
}

impl Harness {
    async fn start(config: CadenceConfig) -> Self {
        let data_dir = tempfile::tempdir().expect("data tempdir");
        // Roots are confined to the org's own files area (PR #280
        // review), so the "existing folder" is staged under it.
        let root_dir = data_dir.path().join("El Artisa");
        std::fs::create_dir(&root_dir).unwrap();

        let clock = Arc::new(TestClock::default());
        // A vault root beside the data dir: Named / Project Version
        // entities are vault pages (issue #261), and the cadence tests
        // never write one — but the backend needs somewhere to look.
        let vault_root = data_dir.path().join("vault");
        std::fs::create_dir_all(&vault_root).unwrap();
        let backend =
            FilesBackend::with_cadence(data_dir.path(), &vault_root, config, clock.clone())
                .expect("backend");
        let scope = Scope::new();
        let local = LocalServer::serve(router(backend.clone()), scope.clone());
        let client = Rpc {
            roots: local.establish().await.expect("establish roots client"),
            version: local.establish().await.expect("establish version client"),
            sync: local.establish().await.expect("establish sync client"),
        };

        // Subscribe before mutating — attached by the time this returns.
        let mut rx = backend.subscribe_events();
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = events.clone();
        let collector = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(ev) => sink.lock().expect("event sink poisoned").push(ev),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        panic!("the collector fell {n} events behind")
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        });

        // `local` is kept alive by the client handles; dropping it here
        // is fine (the served router lives on the scope).
        drop(local);

        Self {
            _data_dir: data_dir,
            root_dir,
            backend,
            client,
            clock,
            events,
            scope,
            _collector: collector,
        }
    }

    /// Adopt the session folder — structure only, as the old
    /// `create_root` was, so no capture happens before a test asks for
    /// one — and wait out the walk behind the return.
    async fn create_root(&self) -> FileRootInfo {
        let root = self
            .client
            .roots
            .adopt(AdoptRequest {
                path: self.root_dir.to_str().unwrap().to_string(),
                name: "El Artisa".to_string(),
                flavor: RootFlavor::Media,
                hash_content: false,
            })
            .await
            .expect("adopt rpc");
        self.backend.settled(id(&root)).await;
        root
    }

    fn write(&self, rel: &str, content: &[u8]) {
        let path = self.root_dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, content).unwrap();
    }

    /// Write a file and tell the service about it, exactly as a watcher
    /// (or a sync daemon) would.
    async fn write_and_hint(&self, root_id: Uuid, rel: &str, content: &[u8]) -> u32 {
        self.write(rel, content);
        self.client
            .version
            .hint_activity(RootId::new(root_id), vec![path(rel)])
            .await
            .expect("hint_activity rpc")
    }

    /// Let the event collector catch up, then count events of a kind.
    async fn counts(&self) -> (usize, usize) {
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
        let events = self.events.lock().expect("event sink poisoned");
        let snapshots = events
            .iter()
            .filter(|e| matches!(e, FilesEvent::Version(VersionEvent::Snapshotted(_))))
            .count();
        let checkpoints = events
            .iter()
            .filter(|e| matches!(e, FilesEvent::Version(VersionEvent::Checkpointed(_))))
            .count();
        (snapshots, checkpoints)
    }

    async fn shutdown(self) {
        self.backend.shutdown().await;
        self.scope.close().await;
    }
}

/// Acceptance criterion 1: "A simulated recording storm (files every
/// few minutes) yields auto-snapshots but exactly one Session
/// checkpoint at quiescence."
#[tokio::test(flavor = "multi_thread")]
async fn a_recording_storm_yields_snapshots_and_exactly_one_checkpoint() {
    let harness = Harness::start(CadenceConfig::default()).await;
    let root = harness.create_root().await;

    // A tracking day: a take lands every 3 minutes for 45 minutes, then
    // everyone goes home. The driver ticks every minute throughout.
    for minute in 0..150u32 {
        if minute <= 42 && minute % 3 == 0 {
            harness
                .write_and_hint(
                    root.id,
                    "Audio Files/gtr.wav",
                    format!("take at minute {minute}").as_bytes(),
                )
                .await;
        }
        harness.backend.tick().await;
        harness.clock.advance(TimeDelta::minutes(1));
    }

    let (snapshots, checkpoints) = harness.counts().await;
    assert_eq!(
        checkpoints, 1,
        "exactly one Session checkpoint, minted at quiescence — not one per tick"
    );
    assert!(
        snapshots >= 4,
        "the storm should have been captured by auto-snapshots along the way, got {snapshots}"
    );

    // The snapshots are real, listed, and recoverable...
    let listed = harness
        .client
        .version
        .snapshots(id(&root), None)
        .await
        .expect("snapshots rpc");
    assert_eq!(listed.len(), snapshots, "every snapshot is listed");

    // ...and none of them is a version: the file's chain holds exactly
    // the one checkpoint (glossary: an auto-snapshot is "never a chain
    // entry").
    let chain = harness
        .client
        .version
        .chain(id(&root), path("Audio Files/gtr.wav"))
        .await
        .expect("chain rpc");
    assert_eq!(
        chain.len(),
        1,
        "snapshots must not surface as version-chain entries: {chain:?}"
    );

    // A quiescent root stays quiescent: more ticks, no more captures.
    for _ in 0..10 {
        harness.backend.tick().await;
        harness.clock.advance(TimeDelta::minutes(10));
    }
    let (_, checkpoints_after) = harness.counts().await;
    assert_eq!(checkpoints_after, 1, "a silent root mints no checkpoints");

    harness.shutdown().await;
}

/// Acceptance criterion 2: "A project-file save marks a save point
/// visible in the chain metadata."
#[tokio::test(flavor = "multi_thread")]
async fn a_project_file_save_marks_a_save_point_visible_in_the_chain() {
    let harness = Harness::start(CadenceConfig::default()).await;
    let root = harness.create_root().await;

    // An ordinary media write is not a save point; saving the REAPER
    // project is.
    harness
        .write_and_hint(root.id, "Audio Files/gtr.wav", b"take 1")
        .await;
    harness.clock.advance(TimeDelta::minutes(2));
    harness
        .write_and_hint(root.id, "El Artisa.rpp", b"<REAPER_PROJECT> take 1")
        .await;

    // Mid-session: the save point rides the nearest auto-snapshot.
    harness.clock.advance(TimeDelta::minutes(11));
    harness.backend.tick().await;
    let snapshots = harness
        .client
        .version
        .snapshots(id(&root), None)
        .await
        .expect("snapshots rpc");
    assert_eq!(snapshots.len(), 1, "one auto-snapshot so far");
    assert_eq!(
        snapshots[0]
            .save_points
            .iter()
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>(),
        ["El Artisa.rpp"],
        "only the project-file save marks a save point"
    );

    // At quiescence the session's checkpoint carries the same save
    // point, and it is visible on the chain entry that checkpoint
    // produced — the criterion's "chain metadata".
    harness.clock.advance(TimeDelta::minutes(31));
    harness.backend.tick().await;

    let chain = harness
        .client
        .version
        .chain(id(&root), path("El Artisa.rpp"))
        .await
        .expect("chain rpc");
    assert_eq!(chain.len(), 1, "one checkpointed state: {chain:?}");
    assert_eq!(
        chain[0]
            .save_points
            .iter()
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>(),
        ["El Artisa.rpp"],
        "the save point is chain metadata: {:?}",
        chain[0]
    );

    // The same metadata rides the audio file's chain entry too — a save
    // point belongs to the session, not to one path.
    let audio_chain = harness
        .client
        .version
        .chain(id(&root), path("Audio Files/gtr.wav"))
        .await
        .expect("chain rpc");
    assert_eq!(audio_chain[0].save_points.len(), 1);

    harness.shutdown().await;
}

/// Acceptance criterion 3: "Ignore-set patterns (e.g. rpp-bak) never
/// enter the store (El Artisa fixture shape as the test case)." The
/// fixture's shape is a REAPER session directory whose every save left
/// another `.rpp-bak` behind, next to the peak caches REAPER rebuilds
/// from the audio itself.
#[tokio::test(flavor = "multi_thread")]
async fn ignored_patterns_never_enter_the_store() {
    let harness = Harness::start(CadenceConfig::default()).await;
    let root = harness.create_root().await;

    harness.write("El Artisa.rpp", b"<REAPER_PROJECT>");
    harness.write("El Artisa.rpp-bak", b"<REAPER_PROJECT> older");
    harness.write("El Artisa-2026-08-01.rpp-bak", b"<REAPER_PROJECT> oldest");
    harness.write("Audio Files/gtr.wav", b"guitar");
    harness.write("Audio Files/gtr.reapeaks", b"peak cache");
    harness.write("Audio Files/.DS_Store", b"finder junk");

    // A fresh root has no *edited* patterns — the flavor seed is not a
    // stored pattern, it is what the stored ones layer onto (issue
    // #273's design, adopted here). Its effect is asserted below: none
    // of this junk reaches the store.
    let ignore = harness
        .client
        .sync
        .ignore_set(id(&root))
        .await
        .expect("ignore_set rpc");
    assert!(ignore.project.is_empty(), "nothing edited yet: {ignore:?}");

    // Backup churn is not activity: hinting only ignored paths opens no
    // session at all.
    let accepted = harness
        .client
        .version
        .hint_activity(
            id(&root),
            vec![path("El Artisa.rpp-bak"), path("Audio Files/gtr.reapeaks")],
        )
        .await
        .expect("hint_activity rpc");
    assert_eq!(accepted, 0, "ignored hints are dropped");
    assert!(!harness.backend.cadence().session_open(root.id));

    let checkpoint = harness
        .client
        .version
        .checkpoint(id(&root), None)
        .await
        .expect("checkpoint_now rpc");
    assert_eq!(
        checkpoint.changed_paths,
        vec![
            "Audio Files/gtr.wav".to_string(),
            "El Artisa.rpp".to_string(),
        ],
        "only the work is versioned, never the scaffolding"
    );

    for junk in [
        "El Artisa.rpp-bak",
        "El Artisa-2026-08-01.rpp-bak",
        "Audio Files/gtr.reapeaks",
        "Audio Files/.DS_Store",
    ] {
        let chain = harness
            .client
            .version
            .chain(id(&root), path(junk))
            .await
            .expect("chain rpc");
        assert!(chain.is_empty(), "{junk} must never enter the store");
    }

    // The set is editable per root, and the edit takes effect on the
    // next capture.
    let stored = harness
        .client
        .sync
        .set_project_ignores(id(&root), vec!["*.wav".to_string(), " *.wav ".to_string()])
        .await
        .expect("set_ignore_set rpc");
    assert_eq!(
        stored.project,
        vec!["*.wav".to_string()],
        "normalized on the way in"
    );

    harness.write("Audio Files/bass.wav", b"bass");
    harness.write("El Artisa.rpp", b"<REAPER_PROJECT> v2");
    let after = harness
        .client
        .version
        .checkpoint(id(&root), None)
        .await
        .expect("checkpoint_now rpc");
    assert!(
        !after.changed_paths.iter().any(|p| p.ends_with("bass.wav")),
        "a newly ignored pattern keeps its files out: {:?}",
        after.changed_paths
    );

    // ...and, critically, does *not* reach back and delete what it
    // already versioned. `gtr.wav` was captured before `*.wav` was
    // ignored; the scan no longer enumerates it, but that means
    // "invisible to versioning", not "the user deleted it" (PR #283
    // review — `set_ignore_set`'s own contract).
    assert!(
        !after
            .changed_paths
            .contains(&"Audio Files/gtr.wav".to_string()),
        "an already-versioned file must not be removed by a new ignore pattern: {:?}",
        after.changed_paths
    );
    let survivor = harness
        .client
        .version
        .chain(id(&root), path("Audio Files/gtr.wav"))
        .await
        .expect("chain rpc");
    assert_eq!(
        survivor.len(),
        1,
        "the already-versioned file keeps its last versioned state: {survivor:?}"
    );

    // A "pattern" that is really several rules is rejected rather than
    // quietly smuggling a `!` re-include past the flavor seed.
    assert!(
        harness
            .client
            .sync
            .set_project_ignores(id(&root), vec!["*.wav\n!keep.wav".to_string()])
            .await
            .is_err(),
        "a pattern carrying a line break must be rejected"
    );

    harness.shutdown().await;
}

/// Acceptance criteria 4 and 5: "Checkpoint certification runs a full
/// stat-scan; a file changing mid-hash is requeued, not corrupted" and
/// "In-flight writes ride into the next snapshot without failing the
/// current one."
#[tokio::test(flavor = "multi_thread")]
async fn a_file_changing_mid_hash_is_requeued_and_rides_into_the_next_capture() {
    let harness = Harness::start(CadenceConfig::default()).await;
    let root = harness.create_root().await;

    harness.write("Renders/mix.wav", b"render v1");
    let first = harness
        .client
        .version
        .checkpoint(id(&root), Some("v1".to_string()))
        .await
        .expect("checkpoint_now rpc");
    assert!(first.requeued_paths.is_empty(), "nothing was moving");
    let settled = harness
        .client
        .version
        .chain(id(&root), path("Renders/mix.wav"))
        .await
        .expect("chain rpc");
    assert_eq!(settled.len(), 1);
    let settled_file_id = settled[0].file_id.clone();

    // Now a bounce is in progress: every time the scan reads the file,
    // the "renderer" has already appended more to it. The hook fires
    // between the pre-read stat and the read itself, which is exactly
    // what a mid-hash write looks like to the certifying scan.
    let render_path = harness.root_dir.join("Renders/mix.wav");
    let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let writes_seen = writes.clone();
    harness
        .backend
        .set_mid_hash_hook(Some(Arc::new(move |path: &std::path::Path| {
            if path == render_path {
                let n = writes_seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::fs::write(path, format!("render v2, {} more blocks", n + 1)).unwrap();
            }
        })));

    // A quiet file lands in the same capture — the in-flight one must
    // not fail it.
    harness.write("Session/notes.txt", b"bounce running");
    let during = harness
        .client
        .version
        .checkpoint(id(&root), Some("mid-bounce".to_string()))
        .await
        .expect("a capture with an in-flight write must still succeed");
    assert_eq!(
        during.requeued_paths,
        vec!["Renders/mix.wav".to_string()],
        "the moving file is requeued, by name"
    );
    assert_eq!(
        during.changed_paths,
        vec!["Session/notes.txt".to_string()],
        "everything that was settled still got captured"
    );
    assert!(
        writes.load(std::sync::atomic::Ordering::SeqCst) > 1,
        "certification retried the moving file before giving up"
    );

    // Not corrupted: the file's newest version is still the last state
    // that was actually coherent, not a torn read of the bounce.
    let chain = harness
        .client
        .version
        .chain(id(&root), path("Renders/mix.wav"))
        .await
        .expect("chain rpc");
    assert_eq!(chain.len(), 1, "no torn version was committed: {chain:?}");
    assert_eq!(chain[0].file_id, settled_file_id);

    // The bounce finishes. The next capture picks the file up — it rode
    // into it rather than being lost.
    harness.backend.set_mid_hash_hook(None);
    let after = harness
        .client
        .version
        .checkpoint(id(&root), Some("bounce done".to_string()))
        .await
        .expect("checkpoint_now rpc");
    assert!(after.requeued_paths.is_empty());
    assert!(
        after.changed_paths.contains(&"Renders/mix.wav".to_string()),
        "the requeued file rides into the next capture: {:?}",
        after.changed_paths
    );
    let chain = harness
        .client
        .version
        .chain(id(&root), path("Renders/mix.wav"))
        .await
        .expect("chain rpc");
    assert_eq!(chain.len(), 2, "and becomes a version of its own");
    assert_ne!(chain[0].file_id, settled_file_id);

    harness.shutdown().await;
}

/// The watcher half: hints reach the cadence engine from a real
/// filesystem event, with no explicit `hint_activity` call. Runs on the
/// real clock's *events* (the cadence itself is still `TestClock`) —
/// the assertion is only that a write opens a session.
#[tokio::test(flavor = "multi_thread")]
async fn the_watcher_hints_activity_into_the_cadence() {
    let harness = Harness::start(CadenceConfig::default()).await;
    let root = harness.create_root().await;
    harness.backend.watch_root(root.id).expect("watch_root");

    // Ignored churn first: a `.rpp-bak` storm must not open a session
    // even when the watcher faithfully reports it.
    harness.write("El Artisa.rpp-bak", b"backup");
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !harness.backend.cadence().session_open(root.id),
        "the watcher's hints still go through the Ignore set"
    );

    harness.write("Audio Files/gtr.wav", b"real work");
    let opened = tokio::time::timeout(Duration::from_secs(10), async {
        while !harness.backend.cadence().session_open(root.id) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(
        opened.is_ok(),
        "a real write should have reached the cadence engine as a hint"
    );

    harness.backend.unwatch_root(root.id);
    harness.shutdown().await;
}

/// PR #283 review finding 2: an explicit `checkpoint_now` takes the
/// session out of the cadence engine *before* writing the capture that
/// needs its save points. If that capture fails, the session has to go
/// back — otherwise the root loses both its save points and its pending
/// quiescence checkpoint, and simply goes silent with uncaptured work.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_checkpoint_preserves_the_session() {
    let harness = Harness::start(CadenceConfig::default()).await;
    let root = harness.create_root().await;

    harness
        .write_and_hint(root.id, "Audio Files/gtr.wav", b"take 1")
        .await;
    harness
        .write_and_hint(root.id, "El Artisa.rpp", b"<REAPER_PROJECT>")
        .await;
    assert!(harness.backend.cadence().session_open(root.id));

    // Make the capture fail for real: the file vanishes between the
    // scan that enumerated it and the read that would hash it, so the
    // open errors and the whole capture is abandoned.
    let doomed = harness.root_dir.join("Audio Files/gtr.wav");
    harness
        .backend
        .set_mid_hash_hook(Some(Arc::new(move |path: &std::path::Path| {
            if path == doomed {
                let _ = std::fs::remove_file(path);
            }
        })));
    let failed = harness.client.version.checkpoint(id(&root), None).await;
    assert!(
        failed.is_err(),
        "the capture should have failed: {failed:?}"
    );

    // The session survived the failure, save points and all.
    assert!(
        harness.backend.cadence().session_open(root.id),
        "a failed checkpoint must not end the session"
    );

    // The bounce recovers; quiescence still mints the session's
    // checkpoint, still carrying the save point marked before the
    // failure.
    harness.backend.set_mid_hash_hook(None);
    harness.write("Audio Files/gtr.wav", b"take 1");
    harness.clock.advance(TimeDelta::minutes(31));
    harness.backend.tick().await;

    let chain = harness
        .client
        .version
        .chain(id(&root), path("El Artisa.rpp"))
        .await
        .expect("chain rpc");
    assert_eq!(chain.len(), 1, "quiescence still checkpointed: {chain:?}");
    assert_eq!(
        chain[0]
            .save_points
            .iter()
            .map(|s| s.path.as_str())
            .collect::<Vec<_>>(),
        ["El Artisa.rpp"],
        "the save points marked before the failed checkpoint survived it"
    );

    harness.shutdown().await;
}

/// PR #283 review finding 3: a write hinted *while* a checkpoint's
/// certifying scan is running is in no capture — the scan enumerated the
/// tree before it. Ending the session anyway would strand that write
/// with no future checkpoint ever falling due for it.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_during_a_capture_keeps_the_session_alive() {
    let harness = Harness::start(CadenceConfig::default()).await;
    let root = harness.create_root().await;

    harness
        .write_and_hint(root.id, "Audio Files/gtr.wav", b"take 1")
        .await;
    harness.clock.advance(TimeDelta::minutes(31));

    // The last save of the day lands mid-capture: the hook fires inside
    // the checkpoint, after its scan enumerated the tree.
    let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let once = fired.clone();
    let root_dir = harness.root_dir.clone();
    let cadence = harness.backend.cadence().clone();
    let clock = harness.clock.clone();
    let root_id = root.id;
    harness
        .backend
        .set_mid_hash_hook(Some(Arc::new(move |_: &std::path::Path| {
            if once.swap(true, std::sync::atomic::Ordering::SeqCst) {
                return;
            }
            std::fs::write(root_dir.join("Audio Files/vox.wav"), b"the last take").unwrap();
            clock.advance(TimeDelta::minutes(1));
            cadence.note_activity(
                root_id,
                &["Audio Files/vox.wav".to_string()],
                &files::ignore::RootFilter::new(
                    ignore::seed(RootFlavor::Media).unwrap(),
                    RootFlavor::Media,
                ),
            );
        })));

    harness.backend.tick().await;
    assert!(
        fired.load(std::sync::atomic::Ordering::SeqCst),
        "the mid-capture write never happened — the test proves nothing"
    );
    harness.backend.set_mid_hash_hook(None);

    assert!(
        harness.backend.cadence().session_open(root.id),
        "a write the capture could not have seen must keep the session open"
    );
    assert!(
        harness
            .client
            .version
            .chain(id(&root), path("Audio Files/vox.wav"))
            .await
            .expect("chain rpc")
            .is_empty(),
        "…precisely because that capture did not include it"
    );

    // And quiescence, measured from the mid-capture write, checkpoints
    // it — rather than the change staying unversioned indefinitely.
    harness.clock.advance(TimeDelta::minutes(31));
    harness.backend.tick().await;
    let chain = harness
        .client
        .version
        .chain(id(&root), path("Audio Files/vox.wav"))
        .await
        .expect("chain rpc");
    assert_eq!(
        chain.len(),
        1,
        "the orphaned write is checkpointed by the session that stayed open: {chain:?}"
    );

    harness.shutdown().await;
}

/// PR #283 review finding 4: the cadence journal holds labels, not
/// content. A corrupt one costs a root its save points and snapshot
/// listing — never its ability to checkpoint, browse, or derive a chain.
#[tokio::test(flavor = "multi_thread")]
async fn a_corrupt_journal_costs_labels_not_the_root() {
    let harness = Harness::start(CadenceConfig::default()).await;
    let root = harness.create_root().await;

    harness.write("El Artisa.rpp", b"<REAPER_PROJECT>");
    harness
        .client
        .version
        .checkpoint(id(&root), Some("v1".to_string()))
        .await
        .expect("checkpoint_now rpc");

    // Truncated mid-write by a crash or a full disk.
    let journal = harness.root_dir.join(".fts-files/cadence.json");
    assert!(journal.exists(), "the capture wrote a journal");
    std::fs::write(&journal, b"{\"checkpoint_head\": \"deadbe").unwrap();

    // Every RPC still works.
    let chain = harness
        .client
        .version
        .chain(id(&root), path("El Artisa.rpp"))
        .await
        .expect("chain must survive a corrupt journal");
    assert_eq!(chain.len(), 1);
    assert!(
        chain[0].save_points.is_empty(),
        "the labels are what was lost"
    );
    assert!(
        harness
            .client
            .version
            .snapshots(id(&root), None)
            .await
            .expect("snapshots must survive a corrupt journal")
            .is_empty()
    );

    harness.write("El Artisa.rpp", b"<REAPER_PROJECT> v2");
    let recovered = harness
        .client
        .version
        .checkpoint(id(&root), Some("v2".to_string()))
        .await
        .expect("checkpoint must survive a corrupt journal");
    assert_eq!(recovered.changed_paths, vec!["El Artisa.rpp".to_string()]);

    // …and the next capture leaves a valid journal behind.
    let bytes = std::fs::read(&journal).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).expect("journal is valid again");
    assert_eq!(
        parsed["checkpoint_head"].as_str(),
        Some(recovered.commit_id.as_str())
    );

    harness.shutdown().await;
}

/// A replica adopted *after* watching was switched on is watched too.
///
/// `enable_watching` walks the roots that exist when it runs and sets a
/// flag for the ones that arrive later. `create_root` and the marked-root
/// adoption both honour that flag; `adopt_replica` did not — so on a sync
/// agent, every root taken from a peer after startup had no watcher, its
/// local edits never became cadence activity, and nothing it held was
/// ever captured. The pull worked; the push had nothing to send. A
/// restart hid it, because then the replica existed when `enable_watching`
/// ran.
#[tokio::test(flavor = "multi_thread")]
async fn a_replica_adopted_after_watching_starts_is_watched() {
    let harness = Harness::start(CadenceConfig::default()).await;
    harness.backend.enable_watching().await;

    // The replica lands in its own tree, the way a pull places one.
    let replica_dir = harness._data_dir.path().join("taken-from-a-peer");
    std::fs::create_dir_all(&replica_dir).unwrap();
    let root_id = Uuid::new_v4();
    harness
        .backend
        .adopt_replica(
            root_id,
            "taken-from-a-peer",
            replica_dir.to_str().unwrap(),
            RootFlavor::Media,
        )
        .expect("adopt replica");

    // No explicit `watch_root`: the flag is what has to carry it.
    std::fs::write(replica_dir.join("overdub.wav"), b"work done offline").unwrap();
    let opened = tokio::time::timeout(Duration::from_secs(10), async {
        while !harness.backend.cadence().session_open(root_id) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(
        opened.is_ok(),
        "an edit in a replica adopted after startup never reached the cadence engine, \
         so it would never be captured and never sync back"
    );

    harness.backend.unwatch_root(root_id);
    harness.shutdown().await;
}
