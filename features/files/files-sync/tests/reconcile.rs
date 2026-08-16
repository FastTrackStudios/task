//! Replica sync (issue #264) at the spec's seam: "multi-agent
//! scenarios run as two in-process agents over memory links". A
//! primary backend serves [`files_sync::SyncService`]; a replica
//! backend adopts the root and reconciles. One test per acceptance
//! criterion.

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, FilesService as _, RootFlavor};
use files_sync::{SyncHost, SyncServiceClient, layer as sync_service_layer, reconcile};

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
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root");
    primary
        .backend
        .checkpoint_now(root.id, None)
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
        .checkpoint_now(root_id, Some("overdub on the plane".into()))
        .await
        .expect("replica checkpoint");

    // Replica → primary: a fast-forward (primary hasn't moved), so no
    // divergence — the primary's live tree just advances.
    let report = reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull back");
    assert_eq!(report.heads_imported, 1);
    assert_eq!(read(&primary, "overdub.wav"), vec![0x33u8; 32 * 1024]);
    let divergent = primary.backend.divergences(root_id).await.unwrap();
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
        .checkpoint_now(root_id, Some("studio".into()))
        .await
        .unwrap();
    std::fs::write(
        replica._dir.path().join("session").join("mix.wav"),
        b"plane take".as_slice(),
    )
    .unwrap();
    replica
        .backend
        .checkpoint_now(root_id, Some("plane".into()))
        .await
        .unwrap();

    // Reconcile both directions: both stores now hold both lines as
    // sibling visible heads.
    reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull replica line");
    let divergent = primary.backend.divergences(root_id).await.unwrap();
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
            root_id,
            "mix.wav".into(),
            files::DivergenceChoice::Pick { commit_id: other },
        )
        .await
        .expect("resolve");
    assert_eq!(read(&primary, "mix.wav"), b"plane take");
    assert!(
        primary
            .backend
            .divergences(root_id)
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
        .browse_at(root_id, sides[0].commit_id.clone(), String::new())
        .await
        .expect("losing side still browsable");
    assert!(listed.iter().any(|e| e.name == "mix.wav"));
    assert!(!resolved.commit_id.is_empty());
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
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .unwrap();
    primary.backend.checkpoint_now(root.id, None).await.unwrap();

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
    assert!(
        manifest.len() >= 3,
        "need a multi-chunk file to prove resumability, got {} chunk(s)",
        manifest.len()
    );
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

    // The resumed pull fetches strictly the remainder.
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
        .set_hydration_policy(root_id, vec!["stems/".into()])
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
        .browse(root_id, String::new())
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
        .hydrate(root_id, "mix.wav".into())
        .await
        .expect("hydrate on demand");
    assert!(!hydrated.stub);
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
        .checkpoint_now(root_id, Some("offline".into()))
        .await
        .expect("offline checkpoint");
    assert_eq!(info.changed_paths, vec!["mix.wav".to_string()]);

    // The replica's chain answers locally, offline.
    let chain = replica
        .backend
        .chain(root_id, "mix.wav".into())
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
        .chain(root_id, "mix.wav".into())
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
        .checkpoint_now(root_id, Some("studio".into()))
        .await
        .unwrap();
    std::fs::write(
        replica._dir.path().join("session").join("mix.wav"),
        b"plane take".as_slice(),
    )
    .unwrap();
    replica
        .backend
        .checkpoint_now(root_id, Some("plane".into()))
        .await
        .unwrap();
    reconcile(&primary.backend, &replica.client, root_id)
        .await
        .expect("pull replica line");

    primary
        .backend
        .resolve_divergence(root_id, "mix.wav".into(), files::DivergenceChoice::KeepBoth)
        .await
        .expect("keep both");

    // Side A keeps the name; the other side is beside it, both intact.
    assert_eq!(read(&primary, "mix.wav"), b"studio take");
    assert_eq!(read(&primary, "mix (divergent 1).wav"), b"plane take");
    assert!(
        primary
            .backend
            .divergences(root_id)
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
        .chain(root_id, "mix.wav".into())
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
        .checkpoint_now(root_id, Some("studio".into()))
        .await
        .unwrap();
    std::fs::write(
        replica._dir.path().join("session").join("mix.wav"),
        b"plane take".as_slice(),
    )
    .unwrap();
    replica
        .backend
        .checkpoint_now(root_id, Some("plane".into()))
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

    let other = primary.backend.divergences(root_id).await.unwrap()[0].sides[1]
        .commit_id
        .clone();
    let err = primary
        .backend
        .resolve_divergence(
            root_id,
            "mix.wav".into(),
            files::DivergenceChoice::Pick { commit_id: other },
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
    let mix = listed.iter().find(|e| e.name == "mix.wav").expect("mix.wav");
    assert_eq!(mix.size, Some(96 * 1024));
    assert!(mix.stub, "a file whose bytes are elsewhere was called resident");
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
