//! Everything a person does in a mounted replica reaches the other
//! machine on its own.
//!
//! The mount is a composed view of replica roots, and writing in it
//! writes into the replica's live tree. So the contract under test is:
//! perform an ordinary filesystem operation in that tree, let the
//! *cadence* run — no `checkpoint` typed by anybody — and the peer that
//! pulls sees exactly the same tree.
//!
//! Run at the sync seam like the rest of `files-sync`'s tests: two
//! backends, each serving [`files_sync::SyncService`] over an in-process
//! `LocalServer`, reconciling through a client rather than reaching into
//! the other's backend. The replica runs on a [`TestClock`] so a
//! quiescence window costs no wall-clock time, and its watcher is real —
//! the hint that opens a session has to come from the filesystem or the
//! test is not testing the path the mount uses.

use std::sync::Arc;
use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::{CadenceConfig, FilesBackend, FilesService as _, RootFlavor, TestClock};
use files_sync::{SyncHost, SyncServiceClient, layer as sync_service_layer, reconcile};

struct Agent {
    dir: tempfile::TempDir,
    backend: FilesBackend,
    _local: LocalServer,
    client: SyncServiceClient,
    clock: Option<Arc<TestClock>>,
}

impl Agent {
    fn tree(&self) -> std::path::PathBuf {
        self.dir.path().join("session")
    }
}

async fn agent(clock: Option<Arc<TestClock>>) -> Agent {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = match &clock {
        Some(clock) => FilesBackend::with_cadence(
            dir.path(),
            dir.path().join("vault"),
            CadenceConfig::default(),
            clock.clone(),
        )
        .expect("backend"),
        None => FilesBackend::new(dir.path(), dir.path().join("vault")).expect("backend"),
    };
    let router = LayerRouter::new().merge(sync_service_layer(SyncHost::new(backend.clone())));
    let local = LocalServer::serve(router, Scope::new());
    let client = local
        .establish::<SyncServiceClient>()
        .await
        .expect("sync client");
    Agent {
        dir,
        backend,
        _local: local,
        client,
        clock,
    }
}

/// A primary holding one committed file, and a replica that has adopted
/// the root, is watched the way a signed-in agent watches one, and has
/// pulled once.
async fn rig() -> (Agent, Agent, uuid::Uuid) {
    let primary = agent(None).await;
    let tree = primary.tree();
    std::fs::create_dir(&tree).unwrap();
    std::fs::write(tree.join("seed.txt"), b"seed").unwrap();
    let root = primary
        .backend
        .create_root(
            tree.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root");
    primary
        .backend
        .checkpoint_now(root.id, None)
        .await
        .expect("seed checkpoint");

    let replica = agent(Some(Arc::new(TestClock::default()))).await;
    replica.backend.enable_watching().await;
    let replica_tree = replica.tree();
    replica
        .backend
        .adopt_replica(
            root.id,
            "session",
            replica_tree.to_str().unwrap(),
            RootFlavor::Media,
        )
        .expect("adopt replica");
    reconcile(&replica.backend, &primary.client, root.id)
        .await
        .expect("first pull");
    (primary, replica, root.id)
}

/// Let the watcher's hint arrive, then run the cadence past quiescence
/// — the automatic path, with no explicit capture anywhere.
async fn settle(agent: &Agent, root_id: uuid::Uuid) {
    let opened = tokio::time::timeout(Duration::from_secs(60), async {
        while !agent.backend.cadence().session_open(root_id) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await;
    assert!(
        opened.is_ok(),
        "the filesystem change never reached the cadence engine as a hint"
    );
    agent
        .clock
        .as_ref()
        .expect("settle is for the TestClock agent")
        .advance_minutes(31);
    let captured = agent.backend.tick().await;
    assert!(
        !captured.is_empty(),
        "quiescence passed with an open session and nothing was captured"
    );
}

/// Everything the replica now holds, as `path -> bytes`, ignoring the
/// root's own bookkeeping.
fn tree_of(agent: &Agent) -> std::collections::BTreeMap<String, Vec<u8>> {
    fn walk(
        base: &std::path::Path,
        dir: &std::path::Path,
        out: &mut std::collections::BTreeMap<String, Vec<u8>>,
    ) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with(".fts-") {
                continue;
            }
            if path.is_dir() {
                walk(base, &path, out);
            } else if let Ok(bytes) = std::fs::read(&path) {
                let rel = path
                    .strip_prefix(base)
                    .expect("inside the tree")
                    .to_string_lossy()
                    .into_owned();
                out.insert(rel, bytes);
            }
        }
    }
    let mut out = std::collections::BTreeMap::new();
    walk(&agent.tree(), &agent.tree(), &mut out);
    out
}

/// The whole matrix in one pass, each operation settled by the cadence
/// and pulled by the peer before the next begins, so a failure names the
/// operation that broke rather than the end state.
///
/// Create, modify in place, delete, rename within the root, a rename
/// that changes only case, and a directory appearing and going away.
/// Case is its own case because a tree keyed by path treats `Notes.md`
/// and `notes.md` as two entries while a case-insensitive filesystem
/// does not, and a rename between them is where that disagreement shows.
#[tokio::test(flavor = "multi_thread")]
async fn every_ordinary_edit_in_a_replica_reaches_the_peer_unaided() {
    let (primary, replica, root_id) = rig().await;
    let tree = replica.tree();

    let pull_back = async |label: &str| {
        reconcile(&primary.backend, &replica.client, root_id)
            .await
            .unwrap_or_else(|e| panic!("{label}: pull back failed: {e}"));
    };

    // ── create ─────────────────────────────────────────────────────
    std::fs::write(tree.join("created.txt"), b"hello").unwrap();
    settle(&replica, root_id).await;
    pull_back("create").await;
    assert_eq!(
        std::fs::read(primary.tree().join("created.txt")).ok(),
        Some(b"hello".to_vec()),
        "a created file did not reach the peer"
    );

    // ── modify in place ────────────────────────────────────────────
    std::fs::write(tree.join("created.txt"), b"hello, again").unwrap();
    settle(&replica, root_id).await;
    pull_back("modify").await;
    assert_eq!(
        std::fs::read(primary.tree().join("created.txt")).ok(),
        Some(b"hello, again".to_vec()),
        "an in-place edit did not reach the peer"
    );

    // ── directory with a file in it ────────────────────────────────
    std::fs::create_dir(tree.join("folder")).unwrap();
    std::fs::write(tree.join("folder").join("inside.txt"), b"nested").unwrap();
    settle(&replica, root_id).await;
    pull_back("mkdir").await;
    assert_eq!(
        std::fs::read(primary.tree().join("folder").join("inside.txt")).ok(),
        Some(b"nested".to_vec()),
        "a new directory's contents did not reach the peer"
    );

    // ── rename within the root ─────────────────────────────────────
    std::fs::rename(tree.join("created.txt"), tree.join("renamed.txt")).unwrap();
    settle(&replica, root_id).await;
    pull_back("rename").await;
    assert_eq!(
        std::fs::read(primary.tree().join("renamed.txt")).ok(),
        Some(b"hello, again".to_vec()),
        "a rename did not carry the content to its new name"
    );
    assert!(
        !primary.tree().join("created.txt").exists(),
        "a rename left the old name behind on the peer"
    );

    // ── rename that changes only case ──────────────────────────────
    std::fs::rename(tree.join("renamed.txt"), tree.join("Renamed.txt")).unwrap();
    settle(&replica, root_id).await;
    pull_back("case rename").await;
    let names: Vec<String> = std::fs::read_dir(primary.tree())
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.eq_ignore_ascii_case("renamed.txt"))
        .collect();
    assert_eq!(
        names,
        vec!["Renamed.txt".to_string()],
        "a case-only rename left the peer with the wrong name (or both)"
    );

    // ── delete a file, then a whole directory ──────────────────────
    std::fs::remove_file(tree.join("Renamed.txt")).unwrap();
    settle(&replica, root_id).await;
    pull_back("delete").await;
    assert!(
        !primary.tree().join("Renamed.txt").exists(),
        "a deletion did not reach the peer"
    );

    std::fs::remove_dir_all(tree.join("folder")).unwrap();
    settle(&replica, root_id).await;
    pull_back("rmdir").await;
    assert!(
        !primary.tree().join("folder").join("inside.txt").exists(),
        "removing a directory did not reach the peer"
    );

    // The two trees agree, and neither side is in dispute.
    assert_eq!(
        tree_of(&primary),
        tree_of(&replica),
        "the peer's tree is not what the replica holds"
    );
    assert!(
        replica
            .backend
            .divergences(root_id)
            .await
            .unwrap()
            .is_empty(),
        "ordinary editing put the replica in dispute with itself"
    );
}

/// The other direction, with local work in flight: a change pulled from
/// the peer lands without touching an unsaved local edit to a different
/// file in the same root.
///
/// This is the case that makes a sync agent trustworthy or not. The
/// person is mid-edit on one file when somebody else's change to another
/// arrives; the arriving change must appear and the unsaved one must
/// survive, uncaptured, to be captured on the next quiescence.
#[tokio::test(flavor = "multi_thread")]
async fn an_arriving_change_does_not_clobber_unsaved_local_work() {
    let (primary, replica, root_id) = rig().await;

    // The person starts editing `mine.txt` and has not stopped, so the
    // cadence has not captured it.
    std::fs::write(replica.tree().join("mine.txt"), b"half a thought").unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        while !replica.backend.cadence().session_open(root_id) {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("the local edit never reached the cadence engine");

    // Meanwhile the peer commits a change to a different file.
    std::fs::write(primary.tree().join("theirs.txt"), b"from the studio").unwrap();
    primary
        .backend
        .checkpoint_now(root_id, None)
        .await
        .expect("peer checkpoint");

    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("pull while editing");

    assert_eq!(
        std::fs::read(replica.tree().join("theirs.txt")).ok(),
        Some(b"from the studio".to_vec()),
        "the peer's change did not arrive in the mount"
    );
    assert_eq!(
        std::fs::read(replica.tree().join("mine.txt")).ok(),
        Some(b"half a thought".to_vec()),
        "the arriving change clobbered unsaved local work"
    );

    // And the unsaved edit still captures and travels once editing stops.
    settle(&replica, root_id).await;
    reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull back");
    assert_eq!(
        std::fs::read(primary.tree().join("mine.txt")).ok(),
        Some(b"half a thought".to_vec()),
        "the local edit never reached the peer after quiescence"
    );
}

/// Both sides editing the same path is a conflict, reported as one, and
/// `resolve` settles it. No last-writer-wins is invented here: the
/// design's answer to two people editing one file is to say so.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_path_edited_on_both_sides_is_a_conflict_that_resolve_settles() {
    let (primary, replica, root_id) = rig().await;

    std::fs::write(replica.tree().join("seed.txt"), b"replica's take").unwrap();
    settle(&replica, root_id).await;
    std::fs::write(primary.tree().join("seed.txt"), b"primary's take").unwrap();
    primary
        .backend
        .checkpoint_now(root_id, None)
        .await
        .expect("primary capture");

    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("pull the peer's line");
    let divergent = replica.backend.divergences(root_id).await.unwrap();
    assert_eq!(
        divergent.len(),
        1,
        "two edits to one path should be reported as exactly one dispute: {divergent:?}"
    );
    assert_eq!(divergent[0].path, "seed.txt");

    let keep = divergent[0].sides[0].commit_id.clone();
    replica
        .backend
        .resolve_divergence(
            root_id,
            "seed.txt".into(),
            files::DivergenceChoice::Pick { commit_id: keep },
        )
        .await
        .expect("resolve");
    assert!(
        replica
            .backend
            .divergences(root_id)
            .await
            .unwrap()
            .is_empty(),
        "resolve left the root in dispute"
    );
}

/// A fork whose two heads agree about every path settles itself, and a
/// fork with a real disagreement does not.
///
/// Two machines that make the same edit — or, before idle captures
/// stopped minting empty commits, two machines whose cadences merely
/// came round — end up with two heads holding identical trees. Nothing
/// reports it: `divergences` lists paths the heads disagree about, and
/// these heads agree about all of them. So the root looks settled while
/// every later reconcile inherits two heads and builds siblings on them.
/// Settling it needs no judgement, so the agent does it.
#[tokio::test(flavor = "multi_thread")]
async fn a_fork_whose_sides_agree_settles_itself() {
    let (primary, replica, root_id) = rig().await;

    // The same edit, made independently on both machines.
    std::fs::write(replica.tree().join("agreed.txt"), b"same bytes").unwrap();
    settle(&replica, root_id).await;
    std::fs::write(primary.tree().join("agreed.txt"), b"same bytes").unwrap();
    primary
        .backend
        .checkpoint_now(root_id, None)
        .await
        .expect("primary capture");

    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("pull the peer's line");

    // Two heads, and nothing to report: the trees agree.
    assert_eq!(
        replica.backend.sync_heads(root_id).expect("heads").len(),
        2,
        "the same edit on both sides should leave two heads to settle"
    );
    assert!(
        replica
            .backend
            .divergences(root_id)
            .await
            .unwrap()
            .is_empty(),
        "heads that agree about every path have nothing to report"
    );

    assert!(
        replica
            .backend
            .settle_identical_heads(root_id)
            .await
            .expect("settle"),
        "a fork whose sides agree should have been settled"
    );
    assert_eq!(
        replica.backend.sync_heads(root_id).expect("heads").len(),
        1,
        "settling should leave one head"
    );
    assert_eq!(
        std::fs::read(replica.tree().join("agreed.txt")).ok(),
        Some(b"same bytes".to_vec()),
        "settling changed the content"
    );

    // Nothing left to settle, and it says so rather than churning.
    assert!(
        !replica
            .backend
            .settle_identical_heads(root_id)
            .await
            .expect("settle again"),
        "a single-head root has no fork to settle"
    );
}

/// The other half of the guard: a fork where the sides genuinely
/// disagree is left for a person, never silently collapsed.
#[tokio::test(flavor = "multi_thread")]
async fn a_fork_with_a_real_disagreement_is_left_alone() {
    let (primary, replica, root_id) = rig().await;

    std::fs::write(replica.tree().join("seed.txt"), b"replica's take").unwrap();
    settle(&replica, root_id).await;
    std::fs::write(primary.tree().join("seed.txt"), b"primary's take").unwrap();
    primary
        .backend
        .checkpoint_now(root_id, None)
        .await
        .expect("primary capture");
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("pull the peer's line");

    assert!(
        !replica
            .backend
            .settle_identical_heads(root_id)
            .await
            .expect("settle"),
        "a real disagreement must not be settled without a person"
    );
    assert_eq!(
        replica.backend.divergences(root_id).await.unwrap().len(),
        1,
        "the disagreement should still be reported"
    );
}

/// Two machines that changed different files rejoin without asking.
///
/// This is the ordinary shape of two-way sync, not an edge case: a pull
/// leaves the peer's head beside our own, and whatever either side does
/// next grows its own line from there. Nobody disagreed about anything —
/// one side renamed a file, the other added one somewhere else — so
/// there is nothing to ask a person and the lines should rejoin.
///
/// Left unmerged they never do, and a listing answers from whichever
/// line it reaches first: which is how a rename shows as both names at
/// once, and a deleted file comes back on the next refresh.
#[tokio::test(flavor = "multi_thread")]
async fn two_lines_that_changed_different_files_merge_themselves() {
    let (primary, replica, root_id) = rig().await;

    // The replica renames the seeded file. The primary, knowing nothing
    // of that, adds one of its own.
    std::fs::rename(
        replica.tree().join("seed.txt"),
        replica.tree().join("renamed.txt"),
    )
    .unwrap();
    settle(&replica, root_id).await;

    std::fs::write(primary.tree().join("theirs.txt"), b"theirs").unwrap();
    primary
        .backend
        .checkpoint_now(root_id, None)
        .await
        .expect("primary capture");

    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("pull the peer's line");
    assert_eq!(
        replica.backend.sync_heads(root_id).expect("heads").len(),
        2,
        "the two machines' work should arrive as two lines"
    );

    assert!(
        replica
            .backend
            .settle_identical_heads(root_id)
            .await
            .expect("settle"),
        "two lines that touched different files should have merged"
    );
    assert_eq!(
        replica.backend.sync_heads(root_id).expect("heads").len(),
        1,
        "the merge should leave one line"
    );

    // Both sides' work survives, and the rename is a rename — not the
    // old name resurrected beside the new one.
    replica
        .backend
        .materialize_head(root_id)
        .expect("materialize the merge");
    let tree = tree_of(&replica);
    assert!(
        tree.contains_key("renamed.txt"),
        "the replica's rename was lost: {:?}",
        tree.keys().collect::<Vec<_>>()
    );
    assert!(
        !tree.contains_key("seed.txt"),
        "the old name came back: {:?}",
        tree.keys().collect::<Vec<_>>()
    );
    assert!(
        tree.contains_key("theirs.txt"),
        "the peer's file was lost: {:?}",
        tree.keys().collect::<Vec<_>>()
    );
}

/// The same path, changed differently on both sides, is still a dispute.
///
/// The merge above must not become a silent winner-picker: when both
/// machines wrote the same file, nothing in the history says which is
/// right, and the pair stays for `resolve` and a person.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_file_changed_on_both_sides_is_not_merged_away() {
    let (primary, replica, root_id) = rig().await;

    std::fs::write(replica.tree().join("seed.txt"), b"ours").unwrap();
    settle(&replica, root_id).await;
    std::fs::write(primary.tree().join("seed.txt"), b"theirs").unwrap();
    primary
        .backend
        .checkpoint_now(root_id, None)
        .await
        .expect("primary capture");

    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("pull the peer's line");

    assert!(
        !replica
            .backend
            .settle_identical_heads(root_id)
            .await
            .expect("settle"),
        "a real disagreement must not be merged away"
    );
    assert_eq!(
        replica.backend.sync_heads(root_id).expect("heads").len(),
        2,
        "the dispute should still be two lines"
    );
    assert!(
        !replica
            .backend
            .divergences(root_id)
            .await
            .unwrap()
            .is_empty(),
        "the disputed path should be reported"
    );
}

/// Three lines drain a pair at a time.
///
/// Two machines diverging while a third already had is not exotic: the
/// roots this was first run against were each carrying three. Folding
/// the first pair leaves one fewer, and the pass after that takes the
/// next, so a pile-up drains over a few beats without any one merge
/// having to reason about three ancestries at once.
#[tokio::test(flavor = "multi_thread")]
async fn three_lines_drain_a_pair_at_a_time() {
    let (primary, replica, root_id) = rig().await;

    // One line from the replica's own work.
    std::fs::write(replica.tree().join("ours.txt"), b"ours").unwrap();
    settle(&replica, root_id).await;

    // Two more from the primary, each landing as its own head: a
    // checkpoint, pulled, then another checkpoint, pulled again, with
    // the replica growing its own line in between.
    std::fs::write(primary.tree().join("theirs-one.txt"), b"one").unwrap();
    primary
        .backend
        .checkpoint_now(root_id, None)
        .await
        .expect("first primary capture");
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("pull the first line");

    std::fs::write(replica.tree().join("ours-again.txt"), b"again").unwrap();
    settle(&replica, root_id).await;
    std::fs::write(primary.tree().join("theirs-two.txt"), b"two").unwrap();
    primary
        .backend
        .checkpoint_now(root_id, None)
        .await
        .expect("second primary capture");
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("pull the second line");

    let heads = replica.backend.sync_heads(root_id).expect("heads").len();
    assert!(
        heads >= 2,
        "expected a pile-up to settle, got {heads} line(s)"
    );

    // Each pass folds one pair. Enough passes and one line is left.
    for _ in 0..heads {
        if replica.backend.sync_heads(root_id).expect("heads").len() == 1 {
            break;
        }
        assert!(
            replica
                .backend
                .settle_identical_heads(root_id)
                .await
                .expect("settle"),
            "a pass that had lines to fold should have folded some"
        );
    }
    assert_eq!(
        replica.backend.sync_heads(root_id).expect("heads").len(),
        1,
        "the lines never drained to one"
    );

    // Nobody's work was dropped on the way.
    replica
        .backend
        .materialize_head(root_id)
        .expect("materialize");
    let tree = tree_of(&replica);
    for expected in [
        "ours.txt",
        "ours-again.txt",
        "theirs-one.txt",
        "theirs-two.txt",
    ] {
        assert!(
            tree.contains_key(expected),
            "{expected} was lost folding three lines: {:?}",
            tree.keys().collect::<Vec<_>>()
        );
    }
}
