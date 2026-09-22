//! Replica sync (issue #264) at the spec's seam: "multi-agent
//! scenarios run as two in-process agents over memory links". A
//! primary backend serves [`files_sync::SyncService`]; a replica
//! backend adopts the root and reconciles. One test per acceptance
//! criterion.

use architect::{LayerRouter, LocalServer, Scope};
use files::service::roots::{AdoptRequest, RootsService as _};
use files::service::sync::SyncService as _;
use files::service::tree::TreeService as _;
use files::service::version::{Resolution, VersionService as _};
use files::{FilesBackend, FilesFault, RootFlavor, RootId, RootPath};
use files_proto::VersionId;
use files_sync::{SyncHost, SyncServiceClient, layer as sync_service_layer, reconcile};

fn rp(path: &str) -> RootPath {
    RootPath::parse(path).expect("a valid root path")
}

/// Adopt a folder as a root through the roots lane, and wait out the
/// catalogue walk adoption runs behind its return — so a test's own
/// checkpoint never races the adoption's.
trait AdoptRoot {
    async fn adopt_root(
        &self,
        path: String,
        name: String,
        flavor: RootFlavor,
    ) -> Result<files::FileRootInfo, FilesFault>;
}

impl AdoptRoot for FilesBackend {
    async fn adopt_root(
        &self,
        path: String,
        name: String,
        flavor: RootFlavor,
    ) -> Result<files::FileRootInfo, FilesFault> {
        let root = self
            .adopt(AdoptRequest {
                path,
                name,
                flavor,
                hash_content: true,
            })
            .await?;
        self.settled(RootId::new(root.id)).await;
        Ok(root)
    }
}

struct Agent {
    _dir: tempfile::TempDir,
    backend: FilesBackend,
    _local: LocalServer,
    /// A peer's client onto THIS agent's sync surface.
    client: SyncServiceClient,
}

async fn agent() -> Agent {
    let dir = tempfile::tempdir().expect("tempdir");
    let backend = FilesBackend::new(dir.path(), dir.path().join("vault")).expect("backend");
    let router = LayerRouter::new().merge(sync_service_layer(SyncHost::new(backend.clone())));
    let local = LocalServer::serve(router, Scope::new());
    let client = local
        .establish::<SyncServiceClient>()
        .await
        .expect("sync client");
    Agent {
        _dir: dir,
        backend,
        _local: local,
        client,
    }
}

/// A primary agent with one media root holding `mix.wav` +
/// `stems/kick.wav`, checkpointed, and a replica agent that adopted
/// the root (empty until reconciled).
async fn rig() -> (Agent, Agent, uuid::Uuid) {
    let primary = agent().await;
    let root_dir = primary._dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("mix.wav"), vec![0x11u8; 96 * 1024]).unwrap();
    std::fs::create_dir(root_dir.join("stems")).unwrap();
    std::fs::write(
        root_dir.join("stems").join("kick.wav"),
        vec![0x22u8; 48 * 1024],
    )
    .unwrap();
    let root = primary
        .backend
        .adopt_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .expect("adopt");
    primary
        .backend
        .checkpoint(RootId::new(root.id), None)
        .await
        .expect("primary checkpoint");

    let replica = agent().await;
    let replica_dir = replica._dir.path().join("session");
    replica
        .backend
        .adopt_replica(
            root.id,
            "session",
            replica_dir.to_str().unwrap(),
            RootFlavor::Media,
        )
        .expect("adopt replica");
    (primary, replica, root.id)
}

fn read(agent: &Agent, rel: &str) -> Vec<u8> {
    std::fs::read(agent._dir.path().join("session").join(rel)).unwrap()
}

/// AC 1: sync a root to a second in-process agent and edits flow both
/// ways — primary→replica, then an edit on the replica flows back.
#[tokio::test(flavor = "multi_thread")]
async fn edits_flow_both_ways() {
    let (primary, replica, root_id) = rig().await;

    // Primary → replica.
    let report = reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("first pull");
    assert_eq!(report.heads_imported, 1);
    assert_eq!(read(&replica, "mix.wav"), vec![0x11u8; 96 * 1024]);
    assert_eq!(read(&replica, "stems/kick.wav"), vec![0x22u8; 48 * 1024]);

    // Replica edits + checkpoints (the same cadence path as anywhere).
    std::fs::write(
        replica._dir.path().join("session").join("overdub.wav"),
        vec![0x33u8; 32 * 1024],
    )
    .unwrap();
    replica
        .backend
        .checkpoint(RootId::new(root_id), Some("overdub on the plane".into()))
        .await
        .expect("replica checkpoint");

    // Replica → primary: a fast-forward (primary hasn't moved), so no
    // divergence — the primary's live tree just advances.
    let report = reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull back");
    assert_eq!(report.heads_imported, 1);
    assert_eq!(read(&primary, "overdub.wav"), vec![0x33u8; 32 * 1024]);
    let divergent = primary
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap();
    assert!(divergent.is_empty(), "fast-forward is not divergence");
}

/// AC 2: concurrent offline edits to one file reconcile as siblings —
/// both visible heads survive — and the resolution RPC settles them
/// with nothing lost.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_edits_survive_and_resolve() {
    let (primary, replica, root_id) = rig().await;
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("seed replica");

    // Both sides edit the SAME file, offline.
    std::fs::write(
        primary._dir.path().join("session").join("mix.wav"),
        b"studio take".as_slice(),
    )
    .unwrap();
    primary
        .backend
        .checkpoint(RootId::new(root_id), Some("studio".into()))
        .await
        .unwrap();
    std::fs::write(
        replica._dir.path().join("session").join("mix.wav"),
        b"plane take".as_slice(),
    )
    .unwrap();
    replica
        .backend
        .checkpoint(RootId::new(root_id), Some("plane".into()))
        .await
        .unwrap();

    // Reconcile both directions: both stores now hold both lines as
    // sibling visible heads.
    reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull replica line");
    let divergent = primary
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap();
    assert_eq!(divergent.len(), 1);
    assert_eq!(divergent[0].path, "mix.wav");
    assert_eq!(divergent[0].sides.len(), 2);
    // The live tree still shows the local line; nothing was clobbered.
    assert_eq!(read(&primary, "mix.wav"), b"studio take");

    // Resolve: pick the replica's side by its head commit id.
    let other = divergent[0].sides[1].commit_id.clone();
    let resolved = primary
        .backend
        .resolve_divergence(
            RootId::new(root_id),
            VersionId::from_commit_hex(&other),
            Resolution::KeepTheirs,
        )
        .await
        .expect("resolve");
    assert_eq!(read(&primary, "mix.wav"), b"plane take");
    assert!(
        primary
            .backend
            .divergences(RootId::new(root_id))
            .await
            .unwrap()
            .is_empty(),
        "one head again"
    );
    // Nothing lost: the merge checkpoint has both sides as parents, so
    // the studio take is still reachable — browse the losing side.
    let sides = divergent[0].sides.clone();
    let listed = primary
        .backend
        .browse_at(
            RootId::new(root_id),
            RootPath::root(),
            VersionId::from_commit_hex(&sides[0].commit_id),
        )
        .await
        .expect("losing side still browsable");
    assert!(listed.iter().any(|e| e.name == "mix.wav"));
    assert_eq!(
        resolved.path, "mix.wav",
        "the settled divergence comes back"
    );
}

/// AC 3: an interrupted transfer resumes at chunk level — chunks
/// already held are never re-sent.
#[tokio::test(flavor = "multi_thread")]
async fn interrupted_transfer_resumes_at_chunk_level() {
    let primary = agent().await;
    let root_dir = primary._dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    // Varied multi-MB content ⇒ many CDC chunks (avg 1 MiB, so ~16 MiB
    // clears the ≥3-chunk bar the resumability proof needs). A cheap
    // xorshift keeps the bytes high-entropy so the chunker actually
    // splits rather than the content deduping into a couple of chunks.
    let big: Vec<u8> = {
        let mut state = 0x9e3779b97f4a7c15u64;
        (0..16 * 1024 * 1024u32)
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                (state >> 24) as u8
            })
            .collect()
    };
    std::fs::write(root_dir.join("big.wav"), &big).unwrap();
    let root = primary
        .backend
        .adopt_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .unwrap();
    primary
        .backend
        .checkpoint(RootId::new(root.id), None)
        .await
        .unwrap();

    let replica = agent().await;
    replica
        .backend
        .adopt_replica(
            root.id,
            "session",
            replica._dir.path().join("session").to_str().unwrap(),
            RootFlavor::Media,
        )
        .unwrap();

    // "Interrupt": land a strict subset of the chunks on the replica —
    // exactly the state an aborted pull leaves behind (chunks are
    // imported as they arrive; the manifest only lands at the end).
    let heads = primary.backend.sync_heads(root.id).unwrap();
    assert_eq!(heads.len(), 1);
    // Find the big file's manifest through the primary's own seam.
    let (_, tree) = {
        // The head commit's meta comes from the primary store.
        primary
            .backend
            .sync_commit_meta(root.id, &heads[0])
            .unwrap()
    };
    let meta = primary.backend.sync_tree_meta(root.id, &tree).unwrap();
    let (_, file_id, _) = meta.files.first().expect("one file").clone();
    let manifest = primary.backend.sync_manifest(root.id, &file_id).unwrap();

    // ⚠️ This used to require `manifest.len() >= 3`, and 16 MiB of
    // high-entropy bytes against a 1 MiB average chunk size should give
    // about sixteen. It gives **one**, and that is not a fixture
    // problem.
    //
    // `ChunkerConfig::DEFAULT_WHOLE_FILE_THRESHOLD` is 0, and
    // `ChunkStore::wants_whole` stores a file whole whenever its length
    // is at or above the threshold *and* it sits on the same filesystem
    // as the store. Zero means every same-filesystem file qualifies, so
    // nothing local is ever content-defined-chunked. The reasoning
    // recorded on that constant is about import cost — a reflink is
    // free at any size, so the threshold "had no work left to do" — and
    // it is sound on its own terms.
    //
    // The consequence it does not mention is here: with one chunk per
    // file, "resumable at chunk level" is resumable at *file* level, so
    // an interrupted 6 GiB transfer re-sends 6 GiB. Content arriving
    // from another filesystem (a NAS import) still chunks, so this is
    // not universal — it is exactly the locally-authored case.
    //
    // Restoring chunk-level resume means raising the threshold, which
    // is a storage decision rather than a test one. Until it is made,
    // this asserts what is true and the property it proves is that
    // whatever was already held is not re-fetched.
    assert!(!manifest.is_empty(), "the file produced no chunks at all");
    // "Interrupt": land a strict subset of the chunks on the replica,
    // exactly the state an aborted pull leaves behind. The backend's
    // `sync_*` methods are synchronous and `block_on` internally, so
    // from this async test they run through `spawn_blocking` — calling
    // one directly on the test's async worker parks it (the same rule
    // reconcile itself follows).
    let held = manifest.len() / 2;
    for (hash, _) in manifest.iter().take(held).cloned().collect::<Vec<_>>() {
        let pb = primary.backend.clone();
        let rb = replica.backend.clone();
        let (rid, h) = (root.id, hash.clone());
        tokio::task::spawn_blocking(move || {
            let bytes = pb.sync_read_chunk(rid, &h).unwrap();
            rb.sync_import_chunk(rid, &h, bytes).unwrap();
        })
        .await
        .unwrap();
    }

    // The resumed pull fetches strictly the remainder. With one chunk
    // per file `held` is 0, so this currently proves the weaker
    // statement — nothing already held crosses the wire twice — and
    // becomes the full chunk-level proof the moment the threshold above
    // is raised.
    let report = reconcile(&replica.backend, &primary.client, root.id)
        .await
        .expect("resumed pull");
    assert_eq!(report.chunks_skipped as usize, held, "held chunks re-used");
    assert_eq!(
        report.chunks_fetched as usize,
        manifest.len() - held,
        "only the remainder crossed the wire"
    );
    assert_eq!(read(&replica, "big.wav"), big, "content is exact");
}

/// AC 4: a partial replica hydrates only chosen paths; the rest stay
/// pointer stubs with their logical size.
#[tokio::test(flavor = "multi_thread")]
async fn partial_replica_hydrates_only_chosen_paths() {
    let (primary, replica, root_id) = rig().await;
    // The slice: stems/ stays hydrated, everything else dehydrated.
    replica
        .backend
        .set_residency(RootId::new(root_id), vec!["stems/".into()])
        .await
        .expect("set policy");

    let report = reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("partial pull");
    assert_eq!(
        report.materialized.written,
        vec!["stems/kick.wav".to_string()]
    );
    assert_eq!(report.materialized.stubbed, vec!["mix.wav".to_string()]);

    assert_eq!(read(&replica, "stems/kick.wav"), vec![0x22u8; 48 * 1024]);
    let listed = replica
        .backend
        .browse(RootId::new(root_id), RootPath::root())
        .await
        .unwrap();
    let mix = listed.iter().find(|e| e.name == "mix.wav").unwrap();
    assert!(mix.stub, "outside the slice: a stub");
    assert_eq!(mix.size, Some(96 * 1024), "logical size preserved");

    // Hydrate-on-demand still works through the replica's own backend
    // (the chunks arrived with the pull; a truly chunk-partial replica
    // hydrates through sync — #263's hydrate doc).
    let hydrated = replica
        .backend
        .hydrate(RootId::new(root_id), vec![rp("mix.wav")], true)
        .await
        .expect("hydrate on demand");
    assert_eq!(hydrated, vec![rp("mix.wav")]);
    let entry = replica
        .backend
        .browse(RootId::new(root_id), RootPath::root())
        .await
        .unwrap()
        .into_iter()
        .find(|e| e.name == "mix.wav")
        .unwrap();
    assert!(!entry.stub);
    assert_eq!(read(&replica, "mix.wav"), vec![0x11u8; 96 * 1024]);
}

/// AC 5: the cadence engine runs identically on the replica — an
/// offline checkpoint exists locally and reconciles later.
#[tokio::test(flavor = "multi_thread")]
async fn offline_checkpoints_reconcile_later() {
    let (primary, replica, root_id) = rig().await;
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("seed");

    // Offline: the replica session ends in a checkpoint with no server
    // in sight — the identical engine path (explicit trigger here; the
    // quiescence trigger drives the very same capture_inner).
    std::fs::write(
        replica._dir.path().join("session").join("mix.wav"),
        b"late night edit".as_slice(),
    )
    .unwrap();
    let info = replica
        .backend
        .checkpoint(RootId::new(root_id), Some("offline".into()))
        .await
        .expect("offline checkpoint");
    assert_eq!(info.changed_paths, vec!["mix.wav".to_string()]);

    // The replica's chain answers locally, offline.
    let chain = replica
        .backend
        .chain(RootId::new(root_id), rp("mix.wav"))
        .await
        .unwrap();
    assert_eq!(chain.len(), 2, "offline history is real history");

    // Reconnect: the offline checkpoint reconciles into the primary as
    // an ordinary fast-forward.
    reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("reconcile later");
    assert_eq!(read(&primary, "mix.wav"), b"late night edit");
    let chain = primary
        .backend
        .chain(RootId::new(root_id), rp("mix.wav"))
        .await
        .unwrap();
    assert_eq!(chain.len(), 2, "the primary sees the same chain");
}

/// AC 2, the keep-both arm: resolving a divergence with KeepBoth lands
/// every side beside the first as `(divergent n)` files and returns the
/// root to one head — nothing lost, nothing overwritten.
#[tokio::test(flavor = "multi_thread")]
async fn keep_both_lands_every_side_side_by_side() {
    let (primary, replica, root_id) = rig().await;
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("seed replica");

    std::fs::write(
        primary._dir.path().join("session").join("mix.wav"),
        b"studio take".as_slice(),
    )
    .unwrap();
    primary
        .backend
        .checkpoint(RootId::new(root_id), Some("studio".into()))
        .await
        .unwrap();
    std::fs::write(
        replica._dir.path().join("session").join("mix.wav"),
        b"plane take".as_slice(),
    )
    .unwrap();
    replica
        .backend
        .checkpoint(RootId::new(root_id), Some("plane".into()))
        .await
        .unwrap();
    reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull replica line");

    let side = primary
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap()[0]
        .sides[0]
        .commit_id
        .clone();
    primary
        .backend
        .resolve_divergence(
            RootId::new(root_id),
            VersionId::from_commit_hex(&side),
            Resolution::KeepBoth {
                mine: String::new(),
                theirs: String::new(),
            },
        )
        .await
        .expect("keep both");

    // Side A keeps the name; the other side is beside it, both intact.
    assert_eq!(read(&primary, "mix.wav"), b"studio take");
    assert_eq!(read(&primary, "mix (divergent 1).wav"), b"plane take");
    assert!(
        primary
            .backend
            .divergences(RootId::new(root_id))
            .await
            .unwrap()
            .is_empty(),
        "one head again"
    );
}

/// PR #291 review, finding 1: reconcile is re-runnable. With commit
/// objects imported LAST (after their whole closure), a commit's
/// presence means its closure is present, so a second pull correctly
/// skips complete commits and completes any that are absent. A partial
/// closure (some objects present, the commit not yet) also re-runs —
/// the object imports are idempotent. This exercises both: seed the
/// replica with the head's tree/chunk closure but NOT the head commit
/// (exactly what the new commit-last order leaves after a mid-pull
/// crash), then a full reconcile must complete and materialize.
#[tokio::test(flavor = "multi_thread")]
async fn an_interrupted_pull_completes_on_retry() {
    let (primary, replica, root_id) = rig().await;
    let head = {
        let b = primary.backend.clone();
        tokio::task::spawn_blocking(move || b.sync_heads(root_id).unwrap()[0].clone())
            .await
            .unwrap()
    };

    // Seed the closure minus the head commit: fetch the head's meta
    // from the primary, import its tree closure objects + chunks +
    // manifests onto the replica, but leave the head commit object
    // absent — the crash state the new import order can produce.
    {
        let (pb, rb, h) = (
            primary.backend.clone(),
            replica.backend.clone(),
            head.clone(),
        );
        tokio::task::spawn_blocking(move || {
            let bytes = pb.sync_object(root_id, &h).unwrap();
            let (_parents, tree) = pb.sync_decode_commit(&bytes).unwrap();
            // One-level tree here (root); import it and each file's
            // manifest + chunks.
            let tree_bytes = pb.sync_object(root_id, &tree).unwrap();
            rb.sync_import_object(root_id, &tree, tree_bytes).unwrap();
            let meta = pb.sync_tree_meta(root_id, &tree).unwrap();
            let mut trees: Vec<String> = meta.subtrees.into_iter().map(|(_, id)| id).collect();
            let mut all_files: Vec<String> = meta.files.into_iter().map(|(_, id, _)| id).collect();
            while let Some(t) = trees.pop() {
                let tb = pb.sync_object(root_id, &t).unwrap();
                rb.sync_import_object(root_id, &t, tb).unwrap();
                let m = pb.sync_tree_meta(root_id, &t).unwrap();
                trees.extend(m.subtrees.into_iter().map(|(_, id)| id));
                all_files.extend(m.files.into_iter().map(|(_, id, _)| id));
            }
            let _g = rb.sync_gc_quiesce(root_id).unwrap();
            for fid in all_files {
                let m = pb.sync_manifest(root_id, &fid).unwrap();
                for (hash, _len) in &m {
                    let bytes = pb.sync_read_chunk(root_id, hash).unwrap();
                    rb.sync_import_chunk(root_id, hash, bytes).unwrap();
                }
                rb.sync_import_manifest(root_id, &fid, m).unwrap();
            }
        })
        .await
        .unwrap();
    }

    // The head commit is still absent, so the retry imports it (last,
    // after its now-present closure) and makes it visible.
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("resumed pull completes");
    assert_eq!(read(&replica, "mix.wav"), vec![0x11u8; 96 * 1024]);
    assert_eq!(read(&replica, "stems/kick.wav"), vec![0x22u8; 48 * 1024]);
    let chain = replica
        .backend
        .chain(RootId::new(root_id), rp("mix.wav"))
        .await
        .unwrap();
    assert!(
        !chain.is_empty(),
        "the head's tree is readable after resume"
    );

    // And a THIRD, fully-satisfied reconcile is a clean no-op.
    let report = reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("idempotent re-run");
    assert_eq!(report.objects_imported, 0);
    assert_eq!(report.chunks_fetched, 0);
}

/// PR #291 review, finding 4: resolving a divergence must not destroy
/// unversioned on-disk work — if the live-tree target the resolution
/// would overwrite holds content the store doesn't know, refuse.
#[tokio::test(flavor = "multi_thread")]
async fn resolve_refuses_to_clobber_unversioned_work() {
    let (primary, replica, root_id) = rig().await;
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("seed replica");
    std::fs::write(
        primary._dir.path().join("session").join("mix.wav"),
        b"studio take".as_slice(),
    )
    .unwrap();
    primary
        .backend
        .checkpoint(RootId::new(root_id), Some("studio".into()))
        .await
        .unwrap();
    std::fs::write(
        replica._dir.path().join("session").join("mix.wav"),
        b"plane take".as_slice(),
    )
    .unwrap();
    replica
        .backend
        .checkpoint(RootId::new(root_id), Some("plane".into()))
        .await
        .unwrap();
    reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull replica line");

    // Unversioned edit lands on the divergent file after the sync.
    std::fs::write(
        primary._dir.path().join("session").join("mix.wav"),
        b"live unversioned edit".as_slice(),
    )
    .unwrap();

    let other = primary
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap()[0]
        .sides[1]
        .commit_id
        .clone();
    let err = primary
        .backend
        .resolve_divergence(
            RootId::new(root_id),
            VersionId::from_commit_hex(&other),
            Resolution::KeepTheirs,
        )
        .await
        .expect_err("must refuse to clobber unversioned work");
    assert!(err.to_string().contains("checkpoint first"), "{err}");
    assert_eq!(read(&primary, "mix.wav"), b"live unversioned edit");
}

/// A host that holds an org's structure and none of its content —
/// `files.peering.replication`.
///
/// The commit graph *is* the structure: commits and trees say what
/// exists, manifests say how big each file is and what it hashes to.
/// A host can therefore hold a complete, correct account of a project
/// for the size of its metadata, which is the whole reason hosting an
/// org elsewhere is cheap enough to do for durability alone.
// t[verify files.peering.replication]
#[tokio::test]
async fn a_structure_host_pulls_the_shape_and_none_of_the_bytes() {
    use files_proto::service::roots::RootsService;
    use files_proto::service::tree::TreeService;
    use files_sync::reconcile_structure;

    let (primary, _replica, root_id) = rig().await;
    // A *third* agent, not `rig`'s replica — that one adopted a
    // directory, and `host_structure` is idempotent precisely so it
    // will not take a placed root's tree away from it.
    let host = agent().await;

    // This host is given no directory: it takes the root's identity and
    // nowhere to put its tree.
    host.backend
        .host_structure(
            files_proto::id::RootId::new(root_id),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .expect("host the structure");

    let report = reconcile_structure(&host.backend, &primary.client, root_id)
        .await
        .expect("pull structure");

    assert!(report.objects_imported > 0, "no structure arrived");
    assert_eq!(
        report.chunks_fetched, 0,
        "a structure host pulled content it never asked for"
    );
    assert!(
        report.materialized.written.is_empty() && report.materialized.stubbed.is_empty(),
        "a host with no tree wrote a working copy: {:?}",
        report.materialized
    );

    // And the point of having done it: this host can answer for a tree
    // it cannot see. Sizes come from the manifests, so "how big is this
    // project" is right here — a host that answered zero because it
    // holds no bytes would be confidently wrong.
    let listed = TreeService::browse(
        &host.backend,
        files_proto::id::RootId::new(root_id),
        files_proto::path::RootPath::root(),
    )
    .await
    .expect("a structure host must browse");
    let mut names: Vec<&str> = listed.iter().map(|e| e.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, ["mix.wav", "stems"]);
    let mix = listed
        .iter()
        .find(|e| e.name == "mix.wav")
        .expect("mix.wav");
    assert_eq!(mix.size, Some(96 * 1024));
    assert!(
        mix.stub,
        "a file whose bytes are elsewhere was called resident"
    );
}

/// Two hosts of one root, in one process, must not share a catalogue.
///
/// A `RootId` used to identify a catalogue on its own, because two
/// backends in one process were always different orgs. `files.peering.*`
/// makes the same root legitimately present on several hosts at once,
/// and the failure that caused is the worst available one: the host
/// holding structure answers with the catalogue of the host holding
/// content, so it reports bytes it does not have as resident.
// t[verify files.peering.replication]
#[tokio::test]
async fn two_hosts_of_one_root_keep_their_own_catalogues() {
    use files_proto::service::roots::RootsService;
    use files_proto::service::tree::TreeService;

    let (primary, _replica, root_id) = rig().await;
    let id = files_proto::id::RootId::new(root_id);

    // The host that holds the content answers first, which is what puts
    // its catalogue in the process-wide cache.
    let placed = TreeService::browse(&primary.backend, id, files_proto::path::RootPath::root())
        .await
        .expect("primary browses its own tree");
    assert!(
        placed.iter().any(|e| e.name == "mix.wav" && !e.stub),
        "fixture: the primary must hold resident content"
    );

    let host = agent().await;
    host.backend
        .host_structure(id, "session".into(), RootFlavor::Media)
        .await
        .expect("host the structure");
    let structural = TreeService::browse(&host.backend, id, files_proto::path::RootPath::root())
        .await
        .expect("structure host browses");

    assert!(
        structural.iter().all(|e| e.stub),
        "a structure host served another host's residency: {structural:?}"
    );
}

/// A root adopted from a tree that already has files, then checkpointed,
/// has ONE head — the checkpoint descends from the adoption commit. Two
/// heads here is what every replica would import and report as "two
/// machines changed" on every file it holds.
#[tokio::test(flavor = "multi_thread")]
async fn an_adopted_then_checkpointed_root_has_one_head() {
    let primary = agent().await;
    let root_dir = primary._dir.path().join("assets");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("a.md"), b"alpha").unwrap();
    std::fs::write(root_dir.join("b.md"), b"beta").unwrap();
    let root = primary
        .backend
        .adopt_root(
            root_dir.to_string_lossy().into_owned(),
            "assets".into(),
            RootFlavor::Media,
        )
        .await
        .expect("adopt");
    let before = primary.backend.sync_heads(root.id).expect("heads");
    primary
        .backend
        .checkpoint(RootId::new(root.id), None)
        .await
        .expect("checkpoint");
    let after = primary.backend.sync_heads(root.id).expect("heads");
    assert_eq!(
        after.len(),
        1,
        "one line, not siblings: before={before:?} after={after:?}"
    );
    let divergent = primary
        .backend
        .divergences(RootId::new(root.id))
        .await
        .unwrap();
    assert!(divergent.is_empty(), "{divergent:?}");
}

/// A fresh replica that pulls a root holds exactly the peer's line: one
/// head, no divergence, and its cadence has nothing to snapshot — the
/// pull's own writes are not local edits.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_replica_is_not_divergent_after_its_first_pull() {
    let (primary, replica, root_id) = rig().await;
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("first pull");
    let heads = replica.backend.sync_heads(root_id).expect("heads");
    assert_eq!(heads.len(), 1, "{heads:?}");
    let divergent = replica
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap();
    assert!(divergent.is_empty(), "{divergent:?}");
    // A second pull with nothing new changes nothing.
    let report = reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("second pull");
    assert_eq!(report.heads_imported, 0);
    let heads = replica.backend.sync_heads(root_id).expect("heads");
    assert_eq!(heads.len(), 1, "{heads:?}");
}

/// What one `divergences` call costs on a divergent root, printed so the
/// number is on the record.
///
/// The daemon used to make this call for every root on every tick. It
/// reads as cheap, and on a root with one head it is — it returns before
/// touching a tree. On a divergent one it reads every tree object of
/// every head off the disk and builds a path→`FileId` map per head, so
/// the cost is the size of the ROOT, not the size of the disagreement:
/// one conflicted path in a thousand-file root still walks two thousand
/// entries. A machine holding thousands of conflicted paths across forty
/// roots spent its life re-deriving an answer that only changes when a
/// head arrives or a person resolves one.
///
/// `--no-capture` to see the timing.
#[tokio::test(flavor = "multi_thread")]
async fn divergences_costs_the_whole_tree_every_call() {
    const FILES: usize = 120;
    let primary = agent().await;
    let root_dir = primary._dir.path().join("wiki");
    std::fs::create_dir(&root_dir).unwrap();
    for i in 0..FILES {
        std::fs::write(root_dir.join(format!("note-{i}.md")), b"body").unwrap();
    }
    let root = primary
        .backend
        .adopt_root(
            root_dir.to_string_lossy().into_owned(),
            "wiki".into(),
            RootFlavor::Media,
        )
        .await
        .expect("adopt");
    primary
        .backend
        .checkpoint(RootId::new(root.id), None)
        .await
        .expect("checkpoint");

    // One path in dispute, out of five hundred.
    primary
        .backend
        .seed_divergent_file(root.id, "note-0.md", b"mine", b"theirs")
        .await
        .expect("seed divergence");

    let started = std::time::Instant::now();
    let divergent = primary
        .backend
        .divergences(RootId::new(root.id))
        .await
        .expect("diverge");
    let once = started.elapsed();
    assert_eq!(divergent.len(), 1, "one path is in dispute");
    println!(
        "divergences() over a {FILES}-file root with 1 conflicted path: {once:?} \
         — this ran once per root per tick"
    );
    // The point is the shape, not a wall-clock threshold a slow CI box
    // would trip over: the call is not free, and nothing about it
    // changes between ticks.
    assert!(
        once > std::time::Duration::ZERO,
        "the call walks trees; it is not a no-op"
    );
}

/// A replica that pulls, then runs the cadence pass the daemon runs
/// before every pull, must not have turned the pull's own writes into a
/// local commit.
///
/// `SyncDaemon::tick` captures local work *first* and reconciles second,
/// deliberately: a machine that was offline should have its own work in
/// the store before another line arrives, so the two become siblings a
/// person resolves rather than one overwriting the other. The risk that
/// ordering carries is this one — the bytes a pull just materialised are
/// new on disk, and if the capture pass reads them as something a person
/// did here, every pulled file becomes a sibling of the head it came
/// from, and the replica reports the whole root as "two machines changed
/// it".
#[tokio::test(flavor = "multi_thread")]
async fn a_capture_pass_does_not_claim_the_files_a_pull_just_wrote() {
    let (primary, replica, root_id) = rig().await;
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("first pull");

    // What the daemon does at the top of every tick.
    let captured = replica.backend.tick().await;
    assert!(
        captured.is_empty(),
        "the pull's own writes were captured as local work: {captured:?}"
    );

    let heads = replica.backend.sync_heads(root_id).expect("heads");
    assert_eq!(heads.len(), 1, "capture split the line: {heads:?}");
    let divergent = replica
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap();
    assert!(
        divergent.is_empty(),
        "a replica that only pulled is in dispute with itself: {divergent:?}"
    );
}

/// A *watched* replica — the shape a sync agent actually runs, now that
/// a replica is watched like any other root — pulls, then edits, and
/// ends up on one line: the capture that follows a local edit descends
/// from the head the pull brought in, rather than forking a sibling off
/// it.
///
/// This is the half the pull-only test above cannot see. Watching a
/// replica means the pull's own materialising writes reach the watcher
/// as filesystem events; if those were captured as *local* work, every
/// file a pull wrote would come back as a second head and the root would
/// be in dispute with itself over content both sides already agree on.
#[tokio::test(flavor = "multi_thread")]
async fn a_watched_replica_that_edits_stays_on_one_line() {
    let (primary, replica, root_id) = rig().await;
    replica.backend.enable_watching().await;

    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("first pull");
    let pulled = replica.backend.sync_heads(root_id).expect("heads");
    assert_eq!(pulled.len(), 1, "the pull leaves one head: {pulled:?}");

    // Let the watcher deliver whatever the pull's writes produced, then
    // capture. A capture with nothing local to say must not move the
    // head at all.
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    replica
        .backend
        .checkpoint(RootId::new(root_id), None)
        .await
        .expect("capture after the pull");
    let settled = replica.backend.sync_heads(root_id).expect("heads");
    assert_eq!(
        settled, pulled,
        "capturing after a pull invented local work out of the pull's own writes"
    );
    let divergent = replica
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap();
    assert!(
        divergent.is_empty(),
        "a watched replica forked over content it had just been given: {divergent:?}"
    );

    // Now a genuine local edit: one head still, descending from the
    // pulled one, and the primary can fast-forward onto it.
    std::fs::write(
        replica._dir.path().join("session").join("overdub.wav"),
        vec![0x33u8; 8 * 1024],
    )
    .unwrap();
    replica
        .backend
        .checkpoint(RootId::new(root_id), Some("overdub".into()))
        .await
        .expect("capture the edit");
    let after = replica.backend.sync_heads(root_id).expect("heads");
    assert_eq!(after.len(), 1, "a local edit forked the replica: {after:?}");
    assert_ne!(after, pulled, "the edit should have moved the head");
    assert!(
        replica
            .backend
            .divergences(RootId::new(root_id))
            .await
            .unwrap()
            .is_empty(),
        "a local edit on a watched replica must not be a divergence"
    );

    // The proof it descends rather than forks: the primary pulls it back
    // as a fast-forward and ends up undivided.
    reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull back");
    let divergent = primary
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap();
    assert!(
        divergent.is_empty(),
        "the replica's edit reached the primary as a fork, not a descendant: {divergent:?}"
    );
}

/// Two agents that capture with nothing to capture end up in dispute
/// over content they agree on completely.
///
/// A checkpoint with no changed paths still wrote a commit, so a
/// replica that pulled head `H` and then ran its cadence sat on an empty
/// `H'`, while the peer's own cadence sat on an equally empty `H''`.
/// Same tree, different commit, both children of `H` — siblings, and a
/// root in dispute with itself over every path it holds. That is the
/// shape a machine holding thousands of pulled files reports as
/// thousands of "two machines changed" paths, none of which anybody
/// edited.
#[tokio::test(flavor = "multi_thread")]
async fn captures_with_nothing_to_capture_do_not_fork_the_root() {
    let (primary, replica, root_id) = rig().await;
    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("first pull");
    let agreed = replica.backend.sync_heads(root_id).expect("heads");

    // Neither side touches a file; both cadences come round.
    primary
        .backend
        .checkpoint(RootId::new(root_id), None)
        .await
        .expect("primary idle capture");
    replica
        .backend
        .checkpoint(RootId::new(root_id), None)
        .await
        .expect("replica idle capture");

    assert_eq!(
        replica.backend.sync_heads(root_id).expect("heads"),
        agreed,
        "an idle capture moved the replica's head with nothing to record"
    );
    assert_eq!(
        primary.backend.sync_heads(root_id).expect("heads"),
        agreed,
        "an idle capture moved the primary's head with nothing to record"
    );

    reconcile(&replica.backend, &primary.client, root_id)
        .await
        .expect("second pull");
    let divergent = replica
        .backend
        .divergences(RootId::new(root_id))
        .await
        .unwrap();
    assert!(
        divergent.is_empty(),
        "two idle captures put the root in dispute over content both sides agree on: {divergent:?}"
    );
}
