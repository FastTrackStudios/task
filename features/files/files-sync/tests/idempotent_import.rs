//! `files.peering.idempotent-import`: a pull that changes nothing writes
//! nothing.
//!
//! The prod incident behind this file (2026-10-21, kanban t_fe92ad56):
//! the server's device-sync sweep pulls every root from THEBATTLESHIP's
//! files daemon once a minute. The daemon's replica is usually *behind*
//! the server — its head is an ancestor of the server's. Importing that
//! head changed nothing (jj keeps only the tips of the visible set), but
//! `import_remote_head` committed the transaction anyway: one jj
//! operation, one index link and one repo reload per root per pass. The
//! oldest prod roots carried 76k operations behind ~23 distinct views,
//! and the server's heap grew with the sweep until it was OOM-killed.
//!
//! The seam is the sweep's own call, `reconcile_at(.., Depth::Content)`,
//! against a peer's `SyncService`; what is observed is the store's op
//! log on disk — the thing prod accumulated.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use architect::{LayerRouter, LocalServer, Scope};
use files::service::roots::{AdoptRequest, RootsService as _};
use files::service::version::VersionService as _;
use files::{CadenceConfig, FilesBackend, RootFlavor, RootId, TestClock};
use files_sync::{
    Depth, NoObserver, SyncHost, SyncServiceClient, layer as sync_service_layer, reconcile,
    reconcile_at,
};

/// One host's root: its backend (on a `TestClock`, so no cadence fires
/// behind the test's back) and the tree it holds.
struct Host {
    _dir: tempfile::TempDir,
    backend: FilesBackend,
    root_id: uuid::Uuid,
    tree: PathBuf,
}

impl Host {
    fn write(&self, rel: &str, body: &str) {
        let p = self.tree.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.tree.join(rel)).ok()
    }

    async fn checkpoint(&self) -> String {
        self.backend
            .checkpoint(RootId::new(self.root_id), None)
            .await
            .expect("checkpoint")
            .commit_id
    }

    /// The jj repo behind this root's tree.
    fn store(&self) -> PathBuf {
        self.tree.join(files_proto::consts::STORE_DIR)
    }

    /// The op log as prod measured it: operation files on disk.
    fn ops_on_disk(&self) -> usize {
        count(&self.store().join("op_store").join("operations"))
    }

    /// The op heads — one file per head, named by operation id.
    fn op_heads(&self) -> Vec<String> {
        let mut heads: Vec<String> = std::fs::read_dir(self.store().join("op_heads").join("heads"))
            .expect("op heads dir")
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        heads.sort();
        heads
    }

    /// Serve this host's `SyncService`, as its daemon or server does.
    async fn serve(&self) -> (LocalServer, SyncServiceClient) {
        let router =
            LayerRouter::new().merge(sync_service_layer(SyncHost::new(self.backend.clone())));
        let local = LocalServer::serve(router, Scope::new());
        let client = local
            .establish::<SyncServiceClient>()
            .await
            .expect("sync client");
        (local, client)
    }
}

fn count(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .count()
}

/// The server: a media root with pages, checkpointed.
async fn server() -> Host {
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
    let host = Host {
        _dir: dir,
        backend,
        root_id: root.id,
        tree,
    };
    host.write("Session.rpp", "<REAPER_PROJECT v1>\n");
    host.write("Stems/kick.wav", "kick v1\n");
    host.checkpoint().await;
    host
}

/// A device that adopted the root as a replica and pulled the server's
/// line — THEBATTLESHIP's files daemon.
async fn replica_of(server: &Host) -> Host {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = Arc::new(TestClock::default());
    let backend = FilesBackend::with_cadence(
        dir.path(),
        dir.path().join("vault"),
        CadenceConfig::default(),
        clock.clone(),
    )
    .expect("backend");
    let tree = dir.path().join("replica");
    backend
        .adopt_replica(
            server.root_id,
            "Periphery",
            tree.to_str().unwrap(),
            RootFlavor::Media,
        )
        .expect("adopt replica");
    let replica = Host {
        _dir: dir,
        backend,
        root_id: server.root_id,
        tree,
    };
    let (_host, client) = server.serve().await;
    reconcile(&replica.backend, &client, server.root_id)
        .await
        .expect("replica pulls the server");
    assert_eq!(
        replica.read("Session.rpp").as_deref(),
        Some("<REAPER_PROJECT v1>\n")
    );
    replica
}

/// The incident: the server is ahead of the replica, and the sweep pulls
/// the replica again and again. Not one of those pulls may write.
// t[verify files.peering.idempotent-import]
#[tokio::test(flavor = "multi_thread")]
async fn pulling_a_peer_that_is_behind_writes_no_operation() {
    let server = server().await;
    let replica = replica_of(&server).await;

    // The server moves on; the replica's head is now an ancestor of the
    // server's — the ordinary shape on prod.
    server.write("Session.rpp", "<REAPER_PROJECT v2>\n");
    server.checkpoint().await;
    let (_peer_host, peer) = replica.serve().await;
    let offered = replica.backend.sync_heads(server.root_id).expect("heads");
    let ours = server.backend.sync_heads(server.root_id).expect("heads");
    assert_eq!(offered.len(), 1, "{offered:?}");
    assert_ne!(offered, ours, "the replica is behind the server");

    let ops_before = server.ops_on_disk();
    let heads_before = server.op_heads();
    for pass in 1..=5 {
        let report = reconcile_at(
            &server.backend,
            &peer,
            server.root_id,
            Depth::Content,
            &NoObserver,
        )
        .await
        .expect("sweep pull");
        assert_eq!(report.heads_imported, 0, "pass {pass}: nothing new");
        assert!(
            report.materialized.written.is_empty() && report.materialized.removed.is_empty(),
            "pass {pass}: a pull from a peer behind us touched the tree: {:?}",
            report.materialized
        );
        assert_eq!(
            server.op_heads(),
            heads_before,
            "pass {pass}: the op head moved — a pull that changed nothing wrote an operation"
        );
        assert_eq!(
            server.ops_on_disk(),
            ops_before,
            "pass {pass}: the op log grew on a pull that changed nothing"
        );
    }
    assert_eq!(
        server.backend.sync_heads(server.root_id).expect("heads"),
        ours,
        "the server's line is untouched"
    );
    assert_eq!(
        server.read("Session.rpp").as_deref(),
        Some("<REAPER_PROJECT v2>\n")
    );
}

/// Same rule from the other side: a peer offering the very head we
/// already hold (both converged) costs nothing either.
// t[verify files.peering.idempotent-import]
#[tokio::test(flavor = "multi_thread")]
async fn pulling_a_converged_peer_writes_no_operation() {
    let server = server().await;
    let replica = replica_of(&server).await;
    let (_peer_host, peer) = replica.serve().await;
    assert_eq!(
        replica.backend.sync_heads(server.root_id).unwrap(),
        server.backend.sync_heads(server.root_id).unwrap()
    );

    let ops_before = server.ops_on_disk();
    let heads_before = server.op_heads();
    for _ in 0..3 {
        reconcile_at(
            &server.backend,
            &peer,
            server.root_id,
            Depth::Content,
            &NoObserver,
        )
        .await
        .expect("sweep pull");
    }
    assert_eq!(server.op_heads(), heads_before);
    assert_eq!(server.ops_on_disk(), ops_before);
}

/// And the rule does not swallow real work: a peer AHEAD of us still
/// lands, with exactly the operation that records it — and the next
/// pass, now behind-or-equal, writes nothing again.
// t[verify files.peering.idempotent-import]
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_with_new_work_still_lands_once() {
    let server = server().await;
    let replica = replica_of(&server).await;

    replica.write("Stems/snare.wav", "snare from the laptop\n");
    let laptop_head = replica.checkpoint().await;
    let (_peer_host, peer) = replica.serve().await;

    let ops_before = server.ops_on_disk();
    let report = reconcile_at(
        &server.backend,
        &peer,
        server.root_id,
        Depth::Content,
        &NoObserver,
    )
    .await
    .expect("pull new work");
    assert_eq!(report.heads_imported, 1);
    assert_eq!(
        server.read("Stems/snare.wav").as_deref(),
        Some("snare from the laptop\n")
    );
    assert!(
        server
            .backend
            .sync_heads(server.root_id)
            .unwrap()
            .contains(&laptop_head),
        "the laptop's head is visible on the server"
    );
    let ops_after_import = server.ops_on_disk();
    assert!(
        ops_after_import > ops_before,
        "importing a new head is an operation"
    );

    let heads = server.op_heads();
    for _ in 0..3 {
        reconcile_at(
            &server.backend,
            &peer,
            server.root_id,
            Depth::Content,
            &NoObserver,
        )
        .await
        .expect("pull again");
    }
    assert_eq!(server.op_heads(), heads, "nothing new, nothing written");
    assert_eq!(server.ops_on_disk(), ops_after_import);
}
