//! `files.peering.replication`: an empty replica never deletes.
//!
//! The prod incident behind this file (2026-10-20/21, kanban t_235917c2):
//! the server's device-sync sweep pulled each vault root from a laptop
//! daemon that held an EMPTY replica of it. Its `heads` answered the jj
//! root commit (`000…0`), the server imported that as a view head, and
//! `materialize_head` resolved the vault's never-checkpointed line to the
//! empty tree — deleting every file the auto-snapshot had already
//! captured. The 30-minute quiescence checkpoint then recorded the empty
//! tree as commit `000…0`, which made the loss look intended.
//!
//! The seam is the same one the sweep uses: a server backend (on a
//! `TestClock`, so the 10-minute debounce costs nothing) runs
//! `files_sync::reconcile_at(.., Depth::Content)` against a peer's
//! `SyncService`.

use std::sync::Arc;

use architect::{LayerRouter, LocalServer, Scope};
use chrono::TimeDelta;
use files::service::roots::{AdoptRequest, RootsService as _};
use files::service::version::VersionService as _;
use files::{CadenceConfig, Captured, FilesBackend, RootFlavor, RootId, RootPath, TestClock};
use files_sync::{
    Depth, NoObserver, SyncHost, SyncServiceClient, layer as sync_service_layer, reconcile,
    reconcile_at,
};

const ROOT_COMMIT: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The vault's pages, as the org had them before the wipe.
const PAGES: &[(&str, &str)] = &[
    ("Home.md", "# Days to Praise\n"),
    ("Songs/Way Maker.md", "---\nkey: E\n---\nlyrics\n"),
    ("Setlists/2026-10-26.md", "- Way Maker\n"),
    ("Records/contacts/atlas.md", "atlas@example.org\n"),
];

struct Server {
    dir: tempfile::TempDir,
    backend: FilesBackend,
    clock: Arc<TestClock>,
    root_id: uuid::Uuid,
}

impl Server {
    fn vault(&self) -> std::path::PathBuf {
        self.dir.path().join("vault-root")
    }

    fn write(&self, rel: &str, body: &str) {
        let p = self.vault().join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, body).unwrap();
    }

    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self.vault().join(rel)).ok()
    }

    /// Write and tell the cadence, as the server's own vault write path
    /// (MCP `write_note`) does.
    async fn write_and_hint(&self, rel: &str, body: &str) {
        self.write(rel, body);
        self.backend
            .hint_activity(
                RootId::new(self.root_id),
                vec![RootPath::parse(rel).unwrap()],
            )
            .await
            .expect("hint");
    }

    /// Advance past the snapshot debounce and run the cadence once.
    async fn snapshot(&self) -> String {
        self.clock.advance(TimeDelta::minutes(11));
        let captured = self.backend.tick().await;
        captured
            .iter()
            .find_map(|c| match c {
                Captured::Snapshot(s) => Some(s.snapshot_id.clone()),
                Captured::Checkpoint(_) => None,
            })
            .unwrap_or_else(|| panic!("the debounce must produce an auto-snapshot: {captured:?}"))
    }
}

/// A server whose vault root has been adopted (no capture) and whose
/// pages have been written and auto-snapshotted — and never checkpointed.
/// That is exactly days-to-praise at 23:35:35Z.
async fn server_with_snapshotted_vault() -> Server {
    let dir = tempfile::tempdir().expect("tempdir");
    let clock = Arc::new(TestClock::default());
    let backend = FilesBackend::with_cadence(
        dir.path(),
        dir.path().join("vault"),
        CadenceConfig::default(),
        clock.clone(),
    )
    .expect("backend");
    let vault = dir.path().join("vault-root");
    std::fs::create_dir_all(&vault).unwrap();
    let root = backend
        .adopt(AdoptRequest {
            path: vault.to_string_lossy().into_owned(),
            name: "days-to-praise".into(),
            flavor: RootFlavor::Media,
            hash_content: false,
        })
        .await
        .expect("adopt");
    backend.settled(RootId::new(root.id)).await;
    let server = Server {
        dir,
        backend,
        clock,
        root_id: root.id,
    };
    for (rel, body) in PAGES {
        server.write_and_hint(rel, body).await;
    }
    server.snapshot().await;
    server
}

struct Peer {
    _dir: tempfile::TempDir,
    backend: FilesBackend,
    _local: LocalServer,
    client: SyncServiceClient,
}

impl Peer {
    fn read(&self, rel: &str) -> Option<String> {
        std::fs::read_to_string(self._dir.path().join("replica").join(rel)).ok()
    }
}

/// A peer that adopted the root as a replica and holds nothing — the
/// THEBATTLESHIP `fts-files-daemon` replica of each org vault.
async fn empty_replica_of(root_id: uuid::Uuid) -> Peer {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).expect("backend");
    let replica_dir = dir.path().join("replica");
    backend
        .adopt_replica(
            root_id,
            "days-to-praise",
            replica_dir.to_str().unwrap(),
            RootFlavor::Media,
        )
        .expect("adopt replica");
    let router = LayerRouter::new().merge(sync_service_layer(SyncHost::new(backend.clone())));
    let local = LocalServer::serve(router, Scope::new());
    let client = local
        .establish::<SyncServiceClient>()
        .await
        .expect("sync client");
    Peer {
        _dir: dir,
        backend,
        _local: local,
        client,
    }
}

/// The incident, end to end: snapshot, sweep pull from an empty replica,
/// quiescence checkpoint. Nothing the org wrote may disappear.
// t[verify files.peering.empty-replica-never-deletes]
#[tokio::test(flavor = "multi_thread")]
async fn a_pull_from_an_empty_replica_never_deletes_snapshotted_pages() {
    let server = server_with_snapshotted_vault().await;
    let peer = empty_replica_of(server.root_id).await;

    // What the empty replica offers is at most the root commit — the
    // wire shape prod saw. (A fixed peer offers nothing at all.)
    let offered = peer
        .backend
        .sync_heads(server.root_id)
        .expect("replica heads");
    assert!(
        offered.iter().all(|h| h == ROOT_COMMIT),
        "an empty replica holds no commits: {offered:?}"
    );

    // The device-sync sweep: Depth::Content, as apps/server/src/device_sync.rs.
    let report = reconcile_at(
        &server.backend,
        &peer.client,
        server.root_id,
        Depth::Content,
        &NoObserver,
    )
    .await
    .expect("sweep pull");

    for (rel, body) in PAGES {
        assert_eq!(
            server.read(rel).as_deref(),
            Some(*body),
            "{rel} was deleted by a pull from an empty replica (removed: {:?})",
            report.materialized.removed
        );
    }
    assert!(
        report.materialized.removed.is_empty(),
        "a peer holding nothing removes nothing: {:?}",
        report.materialized.removed
    );
    assert_eq!(report.heads_imported, 0, "nothing new came from the peer");
    assert!(
        !server
            .backend
            .sync_heads(server.root_id)
            .expect("server heads")
            .iter()
            .any(|h| h == ROOT_COMMIT),
        "the root commit must never be imported as a head"
    );

    // A later write, snapshot and sweep (prod: Atlas's Scripture copies).
    server
        .write_and_hint("Scripture/Psalm 23.md", "The Lord is my shepherd\n")
        .await;
    server.snapshot().await;
    reconcile_at(
        &server.backend,
        &peer.client,
        server.root_id,
        Depth::Content,
        &NoObserver,
    )
    .await
    .expect("second sweep");
    assert_eq!(
        server.read("Scripture/Psalm 23.md").as_deref(),
        Some("The Lord is my shepherd\n"),
        "a snapshotted note survives the next sweep"
    );

    // Quiescence: the session's checkpoint must hold the pages, not the
    // empty tree recorded as commit 000…0.
    server.clock.advance(TimeDelta::minutes(31));
    let captured = server.backend.tick().await;
    let checkpoint = captured
        .iter()
        .find_map(|c| match c {
            Captured::Checkpoint(c) => Some(c.commit_id.clone()),
            Captured::Snapshot(_) => None,
        })
        .unwrap_or_else(|| panic!("quiescence must checkpoint: {captured:?}"));
    assert_ne!(
        checkpoint, ROOT_COMMIT,
        "the checkpoint recorded the empty tree"
    );
    for (rel, body) in PAGES {
        assert_eq!(
            server.read(rel).as_deref(),
            Some(*body),
            "{rel} after checkpoint"
        );
    }

    // And a pull after the checkpoint is still harmless.
    let report = reconcile_at(
        &server.backend,
        &peer.client,
        server.root_id,
        Depth::Content,
        &NoObserver,
    )
    .await
    .expect("post-checkpoint sweep");
    assert!(report.materialized.removed.is_empty());
    assert_eq!(
        server.read("Scripture/Psalm 23.md").as_deref(),
        Some("The Lord is my shepherd\n")
    );
}

/// A peer that answers `heads` with the root commit and nothing else —
/// an old daemon's empty replica, whatever this crate's own `sync_heads`
/// does today. The pull side must refuse it on its own.
#[derive(Clone, architect::HasDispatcher)]
struct RootCommitPeer;

impl files_sync::SyncService for RootCommitPeer {
    async fn roots(&self) -> Result<Vec<files_sync::WireRoot>, files_sync::SyncError> {
        Ok(Vec::new())
    }
    async fn heads(&self, _root_id: uuid::Uuid) -> Result<Vec<String>, files_sync::SyncError> {
        Ok(vec![ROOT_COMMIT.to_string()])
    }
    async fn object(
        &self,
        _root_id: uuid::Uuid,
        id: String,
    ) -> Result<Vec<u8>, files_sync::SyncError> {
        Err(files_sync::SyncError::NotFound(id))
    }
    async fn manifest(
        &self,
        _root_id: uuid::Uuid,
        file_id: String,
    ) -> Result<files_sync::WireManifest, files_sync::SyncError> {
        Err(files_sync::SyncError::NotFound(file_id))
    }
    async fn chunks(
        &self,
        _root_id: uuid::Uuid,
        _hashes: Vec<String>,
    ) -> Result<Vec<files_sync::WireChunk>, files_sync::SyncError> {
        Ok(Vec::new())
    }
    async fn chunk_ranges(
        &self,
        _root_id: uuid::Uuid,
        hash: String,
        _from_chunk: u64,
        _chunks: u64,
    ) -> Result<Vec<u8>, files_sync::SyncError> {
        Err(files_sync::SyncError::NotFound(hash))
    }
}

// t[verify files.peering.empty-replica-never-deletes]
#[tokio::test(flavor = "multi_thread")]
async fn a_root_commit_head_from_the_wire_is_never_imported() {
    let server = server_with_snapshotted_vault().await;
    let router = LayerRouter::new().merge(sync_service_layer(RootCommitPeer));
    let local = LocalServer::serve(router, Scope::new());
    let client = local
        .establish::<SyncServiceClient>()
        .await
        .expect("sync client");

    let report = reconcile(&server.backend, &client, server.root_id)
        .await
        .expect("pull");
    assert!(report.materialized.removed.is_empty(), "{report:?}");
    for (rel, body) in PAGES {
        assert_eq!(server.read(rel).as_deref(), Some(*body), "{rel}");
    }
    assert!(
        !server
            .backend
            .sync_heads(server.root_id)
            .unwrap()
            .iter()
            .any(|h| h == ROOT_COMMIT)
    );
}

/// The same failure on a vault that HAS a checkpoint: an edit captured
/// only by an auto-snapshot is local work, and a pull whose line never
/// held that content must not revert it to the checkpoint's version.
// t[verify files.peering.empty-replica-never-deletes]
#[tokio::test(flavor = "multi_thread")]
async fn a_snapshotted_edit_after_a_checkpoint_survives_a_pull() {
    let server = server_with_snapshotted_vault().await;
    server
        .backend
        .checkpoint(RootId::new(server.root_id), None)
        .await
        .expect("checkpoint");
    server
        .write_and_hint("Home.md", "# Days to Praise\nedited\n")
        .await;
    server
        .write_and_hint("Notes/new.md", "only in a snapshot\n")
        .await;
    server.snapshot().await;

    // A peer that holds the checkpoint line (it pulled it earlier).
    let peer = empty_replica_of(server.root_id).await;
    let server_host = {
        let router =
            LayerRouter::new().merge(sync_service_layer(SyncHost::new(server.backend.clone())));
        LocalServer::serve(router, Scope::new())
    };
    let server_client = server_host
        .establish::<SyncServiceClient>()
        .await
        .expect("server client");
    reconcile(&peer.backend, &server_client, server.root_id)
        .await
        .expect("peer pulls the checkpoint");
    assert_eq!(peer.read("Home.md").as_deref(), Some("# Days to Praise\n"));

    // The sweep pulls it back.
    let report = reconcile(&server.backend, &peer.client, server.root_id)
        .await
        .expect("sweep pull");
    assert_eq!(
        server.read("Home.md").as_deref(),
        Some("# Days to Praise\nedited\n"),
        "a snapshotted edit was reverted by a pull (written: {:?})",
        report.materialized.written
    );
    assert_eq!(
        server.read("Notes/new.md").as_deref(),
        Some("only in a snapshot\n")
    );
    assert!(report.materialized.removed.is_empty(), "{report:?}");
}

/// The other half of the rule: a deletion the remote line really
/// recorded still lands on a replica.
// t[verify files.peering.empty-replica-never-deletes]
#[tokio::test(flavor = "multi_thread")]
async fn a_deletion_the_remote_line_recorded_still_propagates() {
    let server = server_with_snapshotted_vault().await;
    let id = RootId::new(server.root_id);
    server
        .backend
        .checkpoint(id, None)
        .await
        .expect("checkpoint");

    let peer = empty_replica_of(server.root_id).await;
    let host = LocalServer::serve(
        LayerRouter::new().merge(sync_service_layer(SyncHost::new(server.backend.clone()))),
        Scope::new(),
    );
    let client = host.establish::<SyncServiceClient>().await.expect("client");
    reconcile(&peer.backend, &client, server.root_id)
        .await
        .expect("first pull");
    assert!(peer.read("Setlists/2026-10-26.md").is_some());

    std::fs::remove_file(server.vault().join("Setlists/2026-10-26.md")).unwrap();
    std::fs::write(server.vault().join("Home.md"), "# Home v2\n").unwrap();
    server
        .backend
        .checkpoint(id, None)
        .await
        .expect("checkpoint 2");

    let report = reconcile(&peer.backend, &client, server.root_id)
        .await
        .expect("second pull");
    assert_eq!(
        report.materialized.removed,
        vec!["Setlists/2026-10-26.md".to_string()]
    );
    assert!(peer.read("Setlists/2026-10-26.md").is_none());
    assert_eq!(peer.read("Home.md").as_deref(), Some("# Home v2\n"));
}
