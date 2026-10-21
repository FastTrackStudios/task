//! `files.peering.idempotent-import`, the cleanup half: the no-op
//! operations sweeps wrote before the rule existed (76k on the oldest
//! prod roots) can be dropped from a root's op log without losing a
//! version, a head or a reference GC relies on.
//!
//! The no-op operations are written here exactly as the old
//! `import_remote_head` wrote them — a transaction that re-adds a head
//! the view already reaches, committed anyway — interleaved with real
//! checkpoints and an auto-snapshot, so compaction has to re-parent the
//! real operations rather than just trim a tail.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::TimeDelta;
use files::service::roots::{AdoptRequest, RootsService as _};
use files::service::version::VersionService as _;
use files::{CadenceConfig, Captured, FilesBackend, RootFlavor, RootId, RootPath, TestClock};
use jj_lib::backend::CommitId;
use jj_lib::repo::Repo as _;

struct Rig {
    dir: tempfile::TempDir,
    backend: FilesBackend,
    clock: Arc<TestClock>,
    root_id: uuid::Uuid,
}

impl Rig {
    fn tree(&self) -> PathBuf {
        self.dir.path().join("project")
    }

    fn store(&self) -> PathBuf {
        self.tree().join(files_proto::consts::STORE_DIR)
    }

    fn ops_on_disk(&self) -> usize {
        count(&self.store().join("op_store").join("operations"))
    }

    fn index_links_on_disk(&self) -> usize {
        count(&self.store().join("index").join("op_links"))
    }

    fn write(&self, rel: &str, body: &str) {
        let p = self.tree().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    async fn checkpoint(&self) -> String {
        self.backend
            .checkpoint(RootId::new(self.root_id), None)
            .await
            .expect("checkpoint")
            .commit_id
    }

    async fn snapshot(&self, rel: &str, body: &str) -> String {
        self.write(rel, body);
        self.backend
            .hint_activity(
                RootId::new(self.root_id),
                vec![RootPath::parse(rel).unwrap()],
            )
            .await
            .expect("hint");
        self.clock.advance(TimeDelta::minutes(11));
        self.backend
            .tick()
            .await
            .iter()
            .find_map(|c| match c {
                Captured::Snapshot(s) => Some(s.snapshot_id.clone()),
                Captured::Checkpoint(_) => None,
            })
            .expect("an auto-snapshot")
    }

    /// `n` operations of the shape the old sweep wrote: re-add a head the
    /// view already reaches, and commit the unchanged view anyway.
    ///
    /// Through the backend's own repo handle: a second open of the store
    /// in this process would wait on the chunk store's lock.
    fn write_noop_imports(&self, n: usize) {
        self.backend
            .with_repo(self.root_id, |repo| {
                let mut repo = repo.clone();
                for _ in 0..n {
                    let head: CommitId = repo.view().heads().iter().next().unwrap().clone();
                    let commit =
                        pollster::block_on(repo.store().get_commit_async(&head)).expect("head");
                    let mut tx = repo.start_transaction();
                    pollster::block_on(tx.repo_mut().add_head(&commit)).expect("add head");
                    repo =
                        pollster::block_on(tx.commit("sync: import remote head")).expect("commit");
                }
            })
            .expect("with_repo");
    }

    async fn chain(&self, rel: &str) -> Vec<String> {
        self.backend
            .chain(RootId::new(self.root_id), RootPath::parse(rel).unwrap())
            .await
            .expect("chain")
            .into_iter()
            .map(|e| e.commit_id)
            .collect()
    }
}

fn count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .count()
}

async fn rig() -> Rig {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = Arc::new(TestClock::default());
    let backend = FilesBackend::with_cadence(
        dir.path(),
        dir.path().join("vault"),
        CadenceConfig::default(),
        clock.clone(),
    )
    .expect("backend");
    let tree = dir.path().join("project");
    std::fs::create_dir_all(&tree).unwrap();
    let root = backend
        .adopt(AdoptRequest {
            path: tree.to_string_lossy().into_owned(),
            name: "Periphery".into(),
            flavor: RootFlavor::Media,
            hash_content: false,
        })
        .await
        .expect("adopt");
    backend.settled(RootId::new(root.id)).await;
    Rig {
        dir,
        backend,
        clock,
        root_id: root.id,
    }
}

// t[verify files.peering.idempotent-import]
#[tokio::test(flavor = "multi_thread")]
async fn compaction_drops_only_the_operations_that_recorded_nothing() {
    let rig = rig().await;
    let id = rig.root_id;

    rig.write("Session.rpp", "v1\n");
    rig.write("Stems/kick.wav", "kick\n");
    let c1 = rig.checkpoint().await;
    rig.write_noop_imports(40);
    rig.write("Session.rpp", "v2\n");
    let c2 = rig.checkpoint().await;
    rig.write_noop_imports(40);
    let s1 = rig.snapshot("Session.rpp", "v3 (snapshot)\n").await;
    rig.write_noop_imports(20);

    let heads_before = rig.backend.sync_heads(id).expect("heads");
    let chain_before = rig.chain("Session.rpp").await;
    let ops_before = rig.ops_on_disk();
    assert!(chain_before.contains(&c1) && chain_before.contains(&c2));

    let report = rig.backend.compact_op_log(id, Some(0)).expect("compact");
    assert_eq!(report.skipped, None, "{report:?}");
    assert_eq!(report.ops_dropped, 100, "{report:?}");
    assert_eq!(report.ops_after, report.ops_before - 100);
    assert!(
        report.ops_rewritten > 0,
        "the real operations after a no-op are re-parented"
    );
    assert!(
        rig.ops_on_disk() <= ops_before - 100,
        "the dropped operations are gone from disk: {} -> {}",
        ops_before,
        rig.ops_on_disk()
    );
    assert!(report.index_links_swept > 0, "{report:?}");

    // Nothing anybody can see moved.
    assert_eq!(rig.backend.sync_heads(id).expect("heads"), heads_before);
    assert_eq!(rig.chain("Session.rpp").await, chain_before);
    for commit in [&c1, &c2, &s1] {
        assert!(
            rig.backend.sync_has_object(id, commit).expect("has"),
            "{commit} survives compaction"
        );
    }

    // A second pass finds nothing to do and writes nothing.
    let again = rig
        .backend
        .compact_op_log(id, Some(0))
        .expect("compact again");
    assert_eq!(again.ops_dropped, 0, "{again:?}");
    assert_eq!(again.ops_before, report.ops_after);

    // GC over the compacted log still protects every version: the index
    // at the new head reaches what the old one did.
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    rig.backend
        .collect(RootId::new(id), Some(0))
        .await
        .expect("gc");
    for commit in [&c1, &c2, &s1] {
        assert!(
            rig.backend.sync_has_object(id, commit).expect("has"),
            "{commit} survives GC after compaction"
        );
        let (_parents, tree) = rig
            .backend
            .sync_commit_meta(id, commit)
            .expect("commit meta");
        rig.backend
            .sync_tree_meta(id, &tree)
            .expect("its tree is still readable");
    }
    assert_eq!(rig.chain("Session.rpp").await, chain_before);

    // The root keeps working: a new checkpoint lands on the compacted log.
    rig.write("Session.rpp", "v4\n");
    let c4 = rig.checkpoint().await;
    assert!(rig.chain("Session.rpp").await.contains(&c4));

    // And a fresh process opens it: this one lets go of its stores first.
    let heads = rig.backend.sync_heads(id).expect("heads");
    let Rig { dir, backend, .. } = rig;
    backend.shutdown().await;
    drop(backend);
    let reopened = FilesBackend::new(dir.path(), dir.path().join("vault")).expect("reopen");
    assert_eq!(reopened.sync_heads(id).expect("heads after reopen"), heads);
    assert!(reopened.sync_has_object(id, &c1).expect("has"));
    reopened.shutdown().await;
}

/// A log with nothing to drop is left exactly as it was.
// t[verify files.peering.idempotent-import]
#[tokio::test(flavor = "multi_thread")]
async fn compaction_of_a_clean_log_writes_nothing() {
    let rig = rig().await;
    rig.write("Session.rpp", "v1\n");
    rig.checkpoint().await;
    rig.write("Session.rpp", "v2\n");
    rig.checkpoint().await;

    let ops = rig.ops_on_disk();
    let links = rig.index_links_on_disk();
    let report = rig
        .backend
        .compact_op_log(rig.root_id, Some(0))
        .expect("compact");
    assert_eq!(report.ops_dropped, 0, "{report:?}");
    assert_eq!(report.ops_rewritten, 0);
    assert_eq!(rig.ops_on_disk(), ops);
    assert_eq!(rig.index_links_on_disk(), links);
}
