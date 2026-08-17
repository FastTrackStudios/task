//! Issue #261 — Named Versions and Project Versions as Vault entities,
//! end to end over an in-process `architect::LocalServer` (the spec's
//! Testing Decisions primary seam), one test per acceptance criterion:
//!
//! 1. name a version over RPC; it appears in the chain as curated metadata
//! 2. GC sweeps an unnamed old checkpoint but never a Named Version's content
//! 3. the entities replicate with the Vault and re-resolve on any device
//! 4. share-link targeting of a Named Version resolves to the exact change id
//!
//! Criterion 2 is the one place this file reaches past the RPC surface:
//! chunk-level survival is invisible at the service boundary, so it uses
//! the spec's named "secondary harness" (`FilesBackend::with_version_store`)
//! to build the two commits and then to observe what survived. The GC pass
//! itself, and the naming that protects it, still go over RPC.

use std::io::Cursor;
use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, FilesServiceClient, RootFlavor, files_service_layer};
use files_store::version::VersionStoreBackend;
use jj_lib::backend::{
    Backend as _, ChangeId, Commit, CommitId, FileId, MillisSinceEpoch, Signature, Timestamp, Tree,
    TreeValue,
};
use jj_lib::merge::Merge;
use jj_lib::object_id::ObjectId as _;
use jj_lib::repo_path::{RepoPath, RepoPathComponentBuf};

/// A backend plus the two directories it straddles: the org's files
/// area (root content + version stores) and the org vault (the curated
/// version entities).
struct Fixture {
    _data: tempfile::TempDir,
    _vault: tempfile::TempDir,
    data_dir: std::path::PathBuf,
    vault_dir: std::path::PathBuf,
    root_dir: std::path::PathBuf,
    backend: FilesBackend,
    scope: std::sync::Arc<Scope>,
    _local: LocalServer,
    client: FilesServiceClient,
}

async fn fixture(root_name: &str) -> Fixture {
    let data = tempfile::tempdir().expect("data tempdir");
    let vault = tempfile::tempdir().expect("vault tempdir");
    let root_dir = data.path().join("mix-session");
    std::fs::create_dir(&root_dir).unwrap();

    let backend = FilesBackend::new(data.path(), vault.path()).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(
        LayerRouter::new().merge(files_service_layer(backend.clone())),
        scope.clone(),
    );
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            root_name.to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root rpc");

    Fixture {
        data_dir: data.path().to_path_buf(),
        vault_dir: vault.path().to_path_buf(),
        _data: data,
        _vault: vault,
        root_dir,
        backend,
        scope,
        _local: local,
        client,
    }
}

impl Fixture {
    async fn root_id(&self) -> uuid::Uuid {
        self.client.list_roots().await.expect("list_roots")[0].id
    }

    /// Tear the backend all the way down — flush its chunk stores,
    /// close the RPC scope, drop every handle — and hand back the two
    /// temp directories, so a caller that wants to open a *second*
    /// backend over the same on-disk store can keep them alive while
    /// the first one is genuinely gone (two `FsStore`s over one store
    /// in a process is the shape that hangs — see `rpc_surface.rs`).
    async fn finish(self) -> (tempfile::TempDir, tempfile::TempDir) {
        let Self {
            _data,
            _vault,
            backend,
            scope,
            _local,
            client,
            ..
        } = self;
        backend.shutdown().await;
        drop(client);
        scope.close().await;
        drop(_local);
        drop(backend);
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        (_data, _vault)
    }
}

/// Every `.md` page under `dir`, relative-path → contents.
fn vault_pages(dir: &std::path::Path) -> Vec<(String, String)> {
    fn walk(dir: &std::path::Path, prefix: &str, out: &mut Vec<(String, String)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let rel = if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            };
            if entry.path().is_dir() {
                walk(&entry.path(), &rel, out);
            } else if rel.ends_with(".md") {
                out.push((
                    rel,
                    std::fs::read_to_string(entry.path()).unwrap_or_default(),
                ));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, "", &mut out);
    out.sort();
    out
}

/// AC 1: "Name a version over RPC; it appears in the chain as curated
/// metadata." And the entity really is a Vault page — plain markdown
/// with frontmatter, which is what makes AC 3 true for free.
#[tokio::test(flavor = "multi_thread")]
async fn naming_a_version_shows_up_in_the_chain_as_curated_metadata() {
    let fx = fixture("Mix Session").await;
    let root_id = fx.root_id().await;

    std::fs::write(fx.root_dir.join("mix.wav"), b"take one").unwrap();
    let cp1 = fx
        .client
        .checkpoint_now(root_id, Some("first save".into()))
        .await
        .expect("checkpoint_now rpc");
    std::fs::write(fx.root_dir.join("mix.wav"), b"take two, brighter").unwrap();
    let cp2 = fx
        .client
        .checkpoint_now(root_id, None)
        .await
        .expect("checkpoint_now rpc");

    // Before naming, the chain is uncurated.
    let chain = fx
        .client
        .chain(root_id, "mix.wav".into())
        .await
        .expect("chain rpc");
    assert!(
        chain.iter().all(|e| e.names.is_empty()),
        "an automatic chain carries no names until someone curates one: {chain:?}"
    );

    let named = fx
        .client
        .name_version(root_id, cp1.commit_id.clone(), "v3 for client".into())
        .await
        .expect("name_version rpc");
    assert_eq!(named.name, "v3 for client");
    assert_eq!(named.commit_id, cp1.commit_id);
    assert!(
        !named.change_id.is_empty(),
        "the entity references (root id, change id), not just a commit"
    );

    let chain = fx
        .client
        .chain(root_id, "mix.wav".into())
        .await
        .expect("chain rpc");
    let curated: Vec<_> = chain
        .iter()
        .filter(|e| !e.names.is_empty())
        .map(|e| (e.commit_id.as_str(), e.names.clone()))
        .collect();
    assert_eq!(
        curated,
        vec![(cp1.commit_id.as_str(), vec!["v3 for client".to_string()])],
        "only the named checkpoint carries the label: {chain:?}"
    );
    assert!(
        chain
            .iter()
            .find(|e| e.commit_id == cp2.commit_id)
            .is_some_and(|e| e.names.is_empty()),
        "the newer, uncurated save stays uncurated"
    );

    // Naming twice under one name is a conflict, not a silent second page.
    assert!(
        fx.client
            .name_version(root_id, cp2.commit_id.clone(), "v3 for client".into())
            .await
            .is_err(),
        "a root's Named Version names are unique"
    );

    // Naming a commit that isn't in this root's store is rejected.
    assert!(
        fx.client
            .name_version(root_id, "ab".repeat(32), "bogus".into())
            .await
            .is_err(),
        "a Named Version can't reference a commit the store doesn't have"
    );

    // The entity is an ordinary vault page.
    let pages = vault_pages(&fx.vault_dir);
    let (path, body) = pages
        .iter()
        .find(|(p, _)| p.contains("versions/"))
        .expect("the Named Version was written into the vault");
    assert!(
        path.starts_with("Files/mix-session/versions/"),
        "versions live under their root's own vault folder: {path}"
    );
    assert!(body.starts_with("---\n"), "frontmatter page: {body}");
    assert!(body.contains("type: files-named-version"));
    assert!(body.contains(&format!("rootId: {root_id}")));
    assert!(body.contains(&format!("commitId: {}", cp1.commit_id)));

    // `unname_version` drops the curation and leaves the chain alone.
    fx.client
        .unname_version(named.id)
        .await
        .expect("unname_version rpc");
    let chain = fx
        .client
        .chain(root_id, "mix.wav".into())
        .await
        .expect("chain rpc");
    assert_eq!(chain.len(), 2, "the automatic chain is untouched");
    assert!(chain.iter().all(|e| e.names.is_empty()));

    fx.finish().await;
}

/// Writes `content` at `name` and wraps it in a commit written straight
/// through the `Backend` trait rather than a jj transaction — so, like an
/// expired auto-snapshot or a checkpoint no current view head descends
/// from, it is never reachable from `index.all_heads_for_gc()`. That is
/// the shape that distinguishes "protected because the Vault says so"
/// from "protected because jj can still see it".
async fn write_unreachable_commit(
    vs: &VersionStoreBackend,
    name: &str,
    content: &[u8],
) -> (CommitId, files_store::chunk::FileId) {
    let path = RepoPath::from_internal_string(name).unwrap();
    let chunk_id = vs
        .chunks()
        .write_stream(Cursor::new(content.to_vec()))
        .await
        .unwrap();
    let copy_id = vs
        .write_origin_copy(path, chunk_id.as_bytes().to_vec())
        .await
        .unwrap();

    let tree = Tree::from_sorted_entries(vec![(
        RepoPathComponentBuf::new(name).unwrap(),
        TreeValue::File {
            id: FileId::from_bytes(chunk_id.as_bytes()),
            executable: false,
            copy_id,
        },
    )]);
    let tree_id = vs.write_tree(RepoPath::root(), &tree).await.unwrap();

    let empty_signature = Signature {
        name: String::new(),
        email: String::new(),
        timestamp: Timestamp {
            timestamp: MillisSinceEpoch(0),
            tz_offset: 0,
        },
    };
    let commit = Commit {
        parents: vec![vs.root_commit_id().clone()],
        predecessors: vec![],
        root_tree: Merge::from_vec(vec![tree_id]),
        conflict_labels: Merge::from_vec(vec![String::new()]),
        change_id: ChangeId::new(name.bytes().cycle().take(16).collect()),
        description: format!("unreachable: {name}"),
        author: empty_signature.clone(),
        committer: empty_signature,
        secure_sig: None,
    };
    let (commit_id, _) = vs.write_commit(commit, None).await.unwrap();
    (commit_id, chunk_id)
}

/// AC 2: "GC sweeps an unnamed old checkpoint but never a Named
/// Version's content."
///
/// Both commits below are the same age and the same shape — old,
/// index-unreachable, with one chunk each. The *only* difference is
/// that the Vault holds a Named Version pointing at one of them, which
/// is exactly ADR 0001's "the Vault is the authority on immortality".
#[tokio::test(flavor = "multi_thread")]
async fn gc_sweeps_the_unnamed_old_checkpoint_and_never_the_named_one() {
    let fx = fixture("Retention").await;
    let root_id = fx.root_id().await;

    let (kept_commit, kept_chunk, swept_chunk) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    let (kept, kept_chunk) =
                        write_unreachable_commit(vs, "v3-for-client.wav", b"the deliverable").await;
                    let (_swept, swept_chunk) =
                        write_unreachable_commit(vs, "scratch.wav", b"an expired auto-snapshot")
                            .await;
                    (kept, kept_chunk, swept_chunk)
                })
            })
            .expect("with_version_store")
    });

    fx.client
        .name_version(root_id, kept_commit.hex(), "v3 for client".into())
        .await
        .expect("name_version rpc");

    // Both commits are older than this pass's concurrent-writer guard.
    tokio::time::sleep(Duration::from_millis(20)).await;
    let report = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect("gc_root rpc");
    assert_eq!(
        report.protected_commits, 1,
        "the Vault contributed exactly the Named Version to the protect set"
    );
    assert_eq!(
        report.manifests_swept, 1,
        "only the unnamed commit's content was swept: {report:?}"
    );

    let (kept_present, swept_present) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    (
                        vs.chunks().has(kept_chunk).await,
                        vs.chunks().has(swept_chunk).await,
                    )
                })
            })
            .expect("with_version_store")
    });
    assert!(
        kept_present,
        "a Named Version's content is immortal regardless of age or index reachability"
    );
    assert!(
        !swept_present,
        "the unnamed old checkpoint's content is gone"
    );

    // The named content is still readable, not merely present.
    let bytes = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(vs.chunks().read_to_vec(kept_chunk))
            })
            .expect("with_version_store")
    })
    .expect("read the protected content back");
    assert_eq!(bytes, b"the deliverable");

    // Drop the curation and the same pass now sweeps it: the protect
    // set really is read from the Vault every time, not baked in.
    let named = fx
        .client
        .list_named_versions(Some(root_id))
        .await
        .expect("list_named_versions rpc");
    fx.client
        .unname_version(named[0].id)
        .await
        .expect("unname_version rpc");
    let report = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect("gc_root rpc");
    assert_eq!(report.protected_commits, 0);
    assert_eq!(report.manifests_swept, 1, "{report:?}");

    fx.finish().await;
}

/// AC 3: "Named/Project Version entities replicate with the Vault
/// (offline-first) and re-resolve on any device."
///
/// The proof is in two halves. First: another device's copy of the
/// vault — a byte-for-byte copy of the pages, which is all vault
/// replication delivers — re-resolves both entities against the same
/// root with no shared state whatsoever, because a fresh backend rebuilt
/// them from the markdown alone. Second: a page dropped into the vault
/// after the backend was already running is picked up on the very next
/// call (live FS scan, no reindex), which is what an inbound sync looks
/// like.
#[tokio::test(flavor = "multi_thread")]
async fn version_entities_replicate_with_the_vault_and_re_resolve_elsewhere() {
    let fx = fixture("Album").await;
    let root_id = fx.root_id().await;

    std::fs::write(fx.root_dir.join("master.wav"), b"master v1").unwrap();
    let cp1 = fx
        .client
        .checkpoint_now(root_id, Some("master".into()))
        .await
        .expect("checkpoint_now rpc");

    let named = fx
        .client
        .name_version(root_id, cp1.commit_id.clone(), "v1 approved".into())
        .await
        .expect("name_version rpc");
    let pv1 = fx
        .client
        .start_project_version(root_id, None)
        .await
        .expect("start_project_version rpc");
    let pv2 = fx
        .client
        .start_project_version(root_id, Some("Client remix".into()))
        .await
        .expect("start_project_version rpc");
    assert_eq!((pv1.number, pv2.number), (1, 2), "auto-numbered from 1");
    assert_eq!(pv2.label.as_deref(), Some("Client remix"));
    assert_eq!(
        pv1.commit_id, cp1.commit_id,
        "started from the current head"
    );

    // What replication actually carries: markdown pages.
    let pages = vault_pages(&fx.vault_dir);
    assert_eq!(
        pages.len(),
        3,
        "one page per entity, nothing else: {:?}",
        pages.iter().map(|(p, _)| p).collect::<Vec<_>>()
    );

    let data_dir = fx.data_dir.clone();
    let root_dir = fx.root_dir.clone();
    // Hold the temp dirs: the root's content and version store outlive
    // the device that made them, which is the whole point.
    let _dirs = fx.finish().await;

    // "Another device": a fresh backend over a *different* vault
    // directory holding only the replicated pages.
    let other_vault = tempfile::tempdir().expect("other vault");
    for (rel, body) in &pages {
        let abs = other_vault.path().join(rel);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(abs, body).unwrap();
    }

    let backend = FilesBackend::new(&data_dir, other_vault.path()).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(
        LayerRouter::new().merge(files_service_layer(backend.clone())),
        scope.clone(),
    );
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    let replicated = client
        .list_named_versions(Some(root_id))
        .await
        .expect("list_named_versions rpc");
    assert_eq!(replicated.len(), 1);
    assert_eq!(replicated[0].id, named.id, "same entity identity");
    assert_eq!(replicated[0].name, "v1 approved");

    let resolved = client
        .resolve_named_version(named.id)
        .await
        .expect("resolve_named_version rpc");
    assert_eq!(resolved.root_id, root_id);
    assert_eq!(resolved.commit_id, cp1.commit_id);
    assert_eq!(resolved.change_id, named.change_id);

    let project_versions = client
        .list_project_versions(root_id)
        .await
        .expect("list_project_versions rpc");
    assert_eq!(
        project_versions
            .iter()
            .map(|v| v.number)
            .collect::<Vec<_>>(),
        vec![1, 2]
    );
    assert_eq!(project_versions[1].label.as_deref(), Some("Client remix"));

    // An inbound sync while the backend is live: numbering and listing
    // both see it on the next call, with no reindex step anywhere.
    let (rel, body) = pages
        .iter()
        .find(|(p, _)| p.contains("project-versions/"))
        .expect("a project-version page");
    let arrived = other_vault
        .path()
        .join(rel.replace("v1.md", "v9-from-the-laptop.md"));
    std::fs::create_dir_all(arrived.parent().unwrap()).unwrap();
    std::fs::write(
        &arrived,
        body.replace("number: 1", "number: 9")
            .replace(&named.id.to_string(), &uuid::Uuid::new_v4().to_string())
            .replace(&pv1.id.to_string(), &uuid::Uuid::new_v4().to_string()),
    )
    .unwrap();

    let after_sync = client
        .list_project_versions(root_id)
        .await
        .expect("list_project_versions rpc");
    assert_eq!(
        after_sync.iter().map(|v| v.number).collect::<Vec<_>>(),
        vec![1, 2, 9],
        "a page that arrived by replication is visible on the next scan"
    );
    let pv3 = client
        .start_project_version(root_id, None)
        .await
        .expect("start_project_version rpc");
    assert_eq!(
        pv3.number, 10,
        "numbering counts what replication delivered, so two devices don't collide on v3"
    );

    // Leave the root untouched on disk for the tempdirs to clean up.
    assert!(root_dir.join(".fts-root.json").exists());
    backend.shutdown().await;
    scope.close().await;
}

/// AC 4: "Share-link targeting of a Named Version resolves to the exact
/// change id" — the resolution a share link performs before it streams
/// anything, still exact after the chain has moved on.
#[tokio::test(flavor = "multi_thread")]
async fn share_link_targeting_resolves_to_the_exact_change() {
    let fx = fixture("Cut").await;
    let root_id = fx.root_id().await;

    std::fs::write(fx.root_dir.join("cut.mov"), b"rough cut").unwrap();
    let cp1 = fx
        .client
        .checkpoint_now(root_id, Some("rough cut".into()))
        .await
        .expect("checkpoint_now rpc");
    let named = fx
        .client
        .name_version(root_id, cp1.commit_id.clone(), "v2 for client".into())
        .await
        .expect("name_version rpc");

    // The chain moves on: two more saves after the shared one.
    for take in ["fine cut", "final"] {
        std::fs::write(fx.root_dir.join("cut.mov"), take).unwrap();
        fx.client
            .checkpoint_now(root_id, None)
            .await
            .expect("checkpoint_now rpc");
    }

    let resolved = fx
        .client
        .resolve_named_version(named.id)
        .await
        .expect("resolve_named_version rpc");
    assert_eq!(resolved.root_id, root_id);
    assert_eq!(
        resolved.commit_id, cp1.commit_id,
        "the link still points at the exact change that was shared, not at the head"
    );
    assert_eq!(resolved.change_id, named.change_id);
    assert!(!resolved.change_id.is_empty());

    // The resolved change is the one the chain attributes the name to.
    let chain = fx
        .client
        .chain(root_id, "cut.mov".into())
        .await
        .expect("chain rpc");
    let entry = chain
        .iter()
        .find(|e| e.commit_id == resolved.commit_id)
        .expect("the resolved commit is in the chain");
    assert_eq!(entry.names, vec!["v2 for client".to_string()]);

    // A link whose target was un-named no longer resolves.
    fx.client
        .unname_version(named.id)
        .await
        .expect("unname_version rpc");
    assert!(
        fx.client.resolve_named_version(named.id).await.is_err(),
        "a revoked Named Version resolves to nothing"
    );

    fx.finish().await;
}

/// Review findings on the first cut of this ticket, all about the GC
/// protect set being the one place a mistake destroys data.
///
/// - A version page this process can't parse must **stop** the sweep.
///   The list paths log-and-skip a broken page (right for a listing,
///   fatal here): skipping it silently forfeits the protection of the
///   version it names, and GC is not undoable.
/// - A page naming a commit the store doesn't have must **not** stop
///   it. There is nothing left to protect, and failing would wedge GC
///   for that root forever — one stale page from a replication
///   reorder and the store is never swept again.
#[tokio::test(flavor = "multi_thread")]
async fn a_broken_version_page_stops_gc_but_a_stale_reference_does_not() {
    let fx = fixture("Retention Edge").await;
    let root_id = fx.root_id().await;

    let (kept_commit, kept_chunk, swept_chunk) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    let (kept, kept_chunk) =
                        write_unreachable_commit(vs, "keeper.wav", b"the deliverable").await;
                    let (_swept, swept_chunk) =
                        write_unreachable_commit(vs, "scratch.wav", b"an expired auto-snapshot")
                            .await;
                    (kept, kept_chunk, swept_chunk)
                })
            })
            .expect("with_version_store")
    });
    let named = fx
        .client
        .name_version(root_id, kept_commit.hex(), "keeper".into())
        .await
        .expect("name_version rpc");
    tokio::time::sleep(Duration::from_millis(20)).await;

    // Corrupt the page the way a hand-edit or a bad merge would.
    let page = fx.vault_dir.join(&named.path);
    let original = std::fs::read_to_string(&page).unwrap();
    std::fs::write(&page, original.replace("rootId:", "rootID-typo:")).unwrap();

    let err = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect_err("an unreadable version page must abort the sweep");
    assert!(
        format!("{err}").contains(&named.path),
        "the error names the page a human has to fix: {err}"
    );
    let (kept_present, swept_present) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    (
                        vs.chunks().has(kept_chunk).await,
                        vs.chunks().has(swept_chunk).await,
                    )
                })
            })
            .expect("with_version_store")
    });
    assert!(
        kept_present && swept_present,
        "an aborted pass sweeps nothing at all"
    );

    // Repair the page, and point a second (Project Version) page at a
    // commit this store has never had. The sweep proceeds: there is
    // nothing to protect there, and wedging GC would be worse.
    std::fs::write(&page, &original).unwrap();
    let pv = fx
        .client
        .start_project_version(root_id, None)
        .await
        .expect("start_project_version rpc");
    let pv_page = fx.vault_dir.join(&pv.path);
    let pv_body = std::fs::read_to_string(&pv_page).unwrap();
    std::fs::write(&pv_page, pv_body.replace(&pv.commit_id, &"cd".repeat(32))).unwrap();

    let report = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect("a stale reference must not wedge gc");
    assert_eq!(
        report.protected_commits, 1,
        "the stale reference protects nothing; the readable one still does"
    );
    let (kept_present, swept_present) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    (
                        vs.chunks().has(kept_chunk).await,
                        vs.chunks().has(swept_chunk).await,
                    )
                })
            })
            .expect("with_version_store")
    });
    assert!(kept_present, "the Named Version's content survived");
    assert!(!swept_present, "the unnamed content was swept");

    fx.finish().await;
}

/// `task files chain` prints a twelve-character commit prefix, and the
/// CLI tells the user to paste it into `version name` — so the service
/// has to accept an unambiguous prefix, and reject an ambiguous one
/// rather than pick.
#[tokio::test(flavor = "multi_thread")]
async fn naming_accepts_the_commit_prefix_the_chain_prints() {
    let fx = fixture("Prefixes").await;
    let root_id = fx.root_id().await;

    std::fs::write(fx.root_dir.join("take.wav"), b"one").unwrap();
    let cp1 = fx
        .client
        .checkpoint_now(root_id, None)
        .await
        .expect("checkpoint_now rpc");

    let named = fx
        .client
        .name_version(root_id, cp1.commit_id[..12].to_string(), "v1".into())
        .await
        .expect("a twelve-character prefix must resolve");
    assert_eq!(
        named.commit_id, cp1.commit_id,
        "the entity records the full id it resolved to"
    );

    // An empty prefix matches everything, so it must be refused.
    assert!(
        fx.client
            .name_version(root_id, String::new(), "v2".into())
            .await
            .is_err(),
        "an ambiguous (here: empty) prefix must not silently pick a commit"
    );
    // Not hex at all.
    assert!(
        fx.client
            .name_version(root_id, "not-a-commit".into(), "v3".into())
            .await
            .is_err()
    );

    fx.finish().await;
}

/// Write one real checkpoint commit — a jj transaction, so it lands in
/// the op log on disk — straight through the backend's *cached* repo
/// handle, without telling the backend about it.
///
/// That is precisely the state a second process leaves behind: the op
/// log has moved on while this `FilesBackend`'s cached handle and its
/// `head` have not. The CLI reaches this shape for real — its
/// `establish_for_url` falls back to an embedded backend whenever the
/// dial fails — but it cannot be staged with two `FilesBackend`s in one
/// test process, because two `FsStore`s over one store hangs (see
/// `rpc_surface.rs`). Everything below the cache is identical either
/// way: same store, same op log, a cache that is simply behind.
async fn commit_behind_the_cache(
    repo: &std::sync::Arc<jj_lib::repo::ReadonlyRepo>,
    name: &str,
    content: &[u8],
) -> (CommitId, files_store::chunk::FileId) {
    use jj_lib::repo::Repo as _;

    let vs = repo
        .store()
        .backend_impl::<VersionStoreBackend>()
        .expect("a VersionStoreBackend");
    let path = RepoPath::from_internal_string(name).unwrap();
    let chunk_id = vs
        .chunks()
        .write_stream(Cursor::new(content.to_vec()))
        .await
        .unwrap();
    let copy_id = vs
        .write_origin_copy(path, chunk_id.as_bytes().to_vec())
        .await
        .unwrap();

    let parent = repo
        .view()
        .heads()
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(|| repo.store().root_commit_id().clone());
    let base_tree_id = repo
        .store()
        .get_commit_async(&parent)
        .await
        .unwrap()
        .tree()
        .tree_ids()
        .as_resolved()
        .cloned()
        .expect("an unconflicted parent tree");

    let mut builder = jj_lib::tree_builder::TreeBuilder::new(repo.store().clone(), base_tree_id);
    builder.set(
        path.to_owned(),
        TreeValue::File {
            id: FileId::from_bytes(chunk_id.as_bytes()),
            executable: false,
            copy_id,
        },
    );
    let tree_id = builder.write_tree().await.unwrap();
    let merged = jj_lib::merged_tree::MergedTree::resolved(repo.store().clone(), tree_id);

    let mut tx = repo.start_transaction();
    tx.repo_mut()
        .new_commit(vec![parent], merged)
        .set_description(format!("outside writer: {name}"))
        .write()
        .await
        .unwrap();
    let new_repo = tx.commit("outside checkpoint").await.unwrap();
    let commit_id = new_repo.view().heads().iter().next().cloned().unwrap();
    (commit_id, chunk_id)
}

/// Conductor review, finding 1: `gc_root` must not sweep from a stale
/// cached index.
///
/// The repo cache only ever advances on *this* process's own writes,
/// and `root_locks` is a `Mutex` in this process's memory, not a lock
/// on disk. A checkpoint written by anyone else is in neither the stale
/// index's heads nor the Vault — and `keep_newer` doesn't save it,
/// because that guard is about writes happening *now*, not about a
/// handle that went stale an hour ago. The sweep below runs with the
/// guard fully off, so staleness is the only thing under test.
#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_written_behind_the_cache_survives_gc() {
    let fx = fixture("Two Writers").await;
    let root_id = fx.root_id().await;

    std::fs::write(fx.root_dir.join("mix.wav"), b"server take").unwrap();
    fx.client
        .checkpoint_now(root_id, Some("server".into()))
        .await
        .expect("checkpoint_now rpc");

    let (outside_commit, outside_chunk) = tokio::task::block_in_place(|| {
        fx.backend
            .with_repo(root_id, |repo| {
                pollster::block_on(commit_behind_the_cache(repo, "mix.wav", b"laptop take"))
            })
            .expect("with_repo")
    });

    tokio::time::sleep(Duration::from_millis(20)).await;
    let report = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect("gc_root rpc");
    assert_eq!(report.manifests_swept, 0, "nothing here is garbage");

    let present = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(vs.chunks().read_to_vec(outside_chunk))
            })
            .expect("with_version_store")
    });
    assert_eq!(
        present.expect("the outside checkpoint's content survived gc"),
        b"laptop take",
        "a checkpoint this process never saw must not be swept as garbage"
    );

    // And the reload is what put it back in view: the chain now walks
    // through the commit the cache had never heard of.
    let chain = fx
        .client
        .chain(root_id, "mix.wav".into())
        .await
        .expect("chain rpc");
    assert!(
        chain.iter().any(|e| e.commit_id == outside_commit.hex()),
        "the reloaded head sees the outside checkpoint: {chain:?}"
    );

    fx.finish().await;
}

/// Conductor review, finding 2: the strict parse is scoped to the
/// root's own folder, so a broken page belonging to a *different* root
/// — or an ordinary note that merely carries the `files-named-version`
/// tag — cannot wedge this root's sweep.
#[tokio::test(flavor = "multi_thread")]
async fn a_foreign_broken_page_does_not_block_this_roots_gc() {
    let fx = fixture("Scoped").await;
    let root_id = fx.root_id().await;

    let (kept_commit, kept_chunk, swept_chunk) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    let (kept, kept_chunk) =
                        write_unreachable_commit(vs, "keeper.wav", b"the deliverable").await;
                    let (_swept, swept_chunk) =
                        write_unreachable_commit(vs, "scratch.wav", b"expired").await;
                    (kept, kept_chunk, swept_chunk)
                })
            })
            .expect("with_version_store")
    });
    fx.client
        .name_version(root_id, kept_commit.hex(), "keeper".into())
        .await
        .expect("name_version rpc");

    // Another root's version page, malformed.
    let foreign = fx.vault_dir.join("Files/some-other-root/versions/v1.md");
    std::fs::create_dir_all(foreign.parent().unwrap()).unwrap();
    std::fs::write(
        &foreign,
        "---\ntype: files-named-version\nname: theirs\nnope: no root id\n---\n",
    )
    .unwrap();

    // And an ordinary note a user happened to tag — `matches` accepts a
    // `tags:` entry, not just `type:`, so this is claimed by the walk
    // too.
    let tagged = fx.vault_dir.join("Notes/mixing-thoughts.md");
    std::fs::create_dir_all(tagged.parent().unwrap()).unwrap();
    std::fs::write(
        &tagged,
        "---\ntags:\n  - files-named-version\n---\n\nJust a note about versioning.\n",
    )
    .unwrap();

    tokio::time::sleep(Duration::from_millis(20)).await;
    let report = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect("a page outside this root's folder must not block its sweep");
    assert_eq!(report.protected_commits, 1);

    let (kept_present, swept_present) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    (
                        vs.chunks().has(kept_chunk).await,
                        vs.chunks().has(swept_chunk).await,
                    )
                })
            })
            .expect("with_version_store")
    });
    assert!(kept_present, "the Named Version's content survived");
    assert!(!swept_present, "the unnamed content was swept");

    fx.finish().await;
}

/// Conductor review, findings 3 and 4, on the two shapes a hand-written
/// page actually takes (these pages are advertised as editable in a
/// text editor):
///
/// - a twelve-character commit *prefix* — what `task files chain` and
///   `version list` print — sweeps normally, because the protect set
///   stores the id `resolve_commit` resolved rather than the truncated
///   one parsed off the page (which would decode to a six-byte id and
///   fail the mark phase on every pass);
/// - an empty `commitId`, which `ProjectVersions::from_page`
///   deliberately tolerates, names nothing and is skipped rather than
///   wedging the root forever.
#[tokio::test(flavor = "multi_thread")]
async fn a_prefix_page_gcs_fine_and_an_empty_commit_id_does_not_wedge() {
    let fx = fixture("Handwritten").await;
    let root_id = fx.root_id().await;

    // A real checkpoint, so its commit is in the index — which is what
    // prefix resolution goes through.
    std::fs::write(fx.root_dir.join("keeper.wav"), b"the deliverable").unwrap();
    let cp = fx
        .client
        .checkpoint_now(root_id, Some("keeper".into()))
        .await
        .expect("checkpoint_now rpc");
    let swept_chunk = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(write_unreachable_commit(vs, "scratch.wav", b"expired")).1
            })
            .expect("with_version_store")
    });
    let named = fx
        .client
        .name_version(root_id, cp.commit_id.clone(), "keeper".into())
        .await
        .expect("name_version rpc");

    // Rewrite the page the way a human would after copying an id out of
    // `task files chain`: the first twelve characters.
    let page = fx.vault_dir.join(&named.path);
    let body = std::fs::read_to_string(&page).unwrap();
    std::fs::write(
        &page,
        body.replace(&named.commit_id, &named.commit_id[..12]),
    )
    .unwrap();

    // And a hand-written Project Version page that never got a commit
    // id — a shape `ProjectVersions::from_page` deliberately tolerates.
    // (`start_project_version` refuses to write one, so it can only
    // arrive by hand or by replication.)
    let empty = fx
        .vault_dir
        .join("Files/handwritten/project-versions/v7.md");
    std::fs::create_dir_all(empty.parent().unwrap()).unwrap();
    std::fs::write(
        &empty,
        format!(
            "---\ntype: files-project-version\nrootId: {root_id}\nnumber: 7\ncommitId: ''\n---\n"
        ),
    )
    .unwrap();

    tokio::time::sleep(Duration::from_millis(20)).await;
    let report = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect("a prefix id and an empty id must both leave gc working");
    assert_eq!(
        report.protected_commits, 1,
        "the prefix resolved to the real commit; the empty id protected nothing"
    );
    assert_eq!(report.manifests_swept, 1, "the unnamed orphan was swept");

    // Still working on the next pass — the point of "does not wedge".
    fx.client
        .gc_root(root_id, Some(0))
        .await
        .expect("gc stays healthy on subsequent passes");
    let swept_present = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(vs.chunks().has(swept_chunk))
            })
            .expect("with_version_store")
    });
    assert!(!swept_present);

    // The one abbreviation that must NOT be waved through: a prefix
    // naming a commit the index can't reach. Prefix lookup goes through
    // the index, and pointing past it is a Named Version's whole
    // purpose — so "didn't resolve" here means "couldn't interpret",
    // not "already gone", and forfeiting the content would be the very
    // mistake this protect set exists to prevent.
    let orphan = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(write_unreachable_commit(vs, "orphan.wav", b"unreachable")).0
            })
            .expect("with_version_store")
    });
    let orphan_named = fx
        .client
        .name_version(root_id, orphan.hex(), "orphan".into())
        .await
        .expect("name_version rpc");
    let orphan_page = fx.vault_dir.join(&orphan_named.path);
    let orphan_body = std::fs::read_to_string(&orphan_page).unwrap();
    std::fs::write(
        &orphan_page,
        orphan_body.replace(&orphan_named.commit_id, &orphan_named.commit_id[..12]),
    )
    .unwrap();
    let err =
        fx.client.gc_root(root_id, Some(0)).await.expect_err(
            "an unresolvable abbreviation must stop the sweep, not forfeit the content",
        );
    assert!(
        format!("{err}").contains(&orphan_named.path),
        "the error names the page to fix: {err}"
    );

    fx.finish().await;
}

/// Curation is flavor-agnostic — the surface this branch and #273's
/// software roots create together, which neither side could test alone.
///
/// A Named Version references `(root id, change id)` and the store
/// "knows nothing about names" (ADR 0001), so nothing about it should
/// care whether those ids live in Files' own CAS or in a colocated git
/// repository. What legitimately *is* flavor-specific is the sweep:
/// a software root's objects are git's, and git collects its own
/// garbage, so `gc_root` says so rather than failing obscurely.
#[tokio::test(flavor = "multi_thread")]
async fn curation_works_the_same_on_a_software_root() {
    let data = tempfile::tempdir().expect("data tempdir");
    let vault = tempfile::tempdir().expect("vault tempdir");
    let root_dir = data.path().join("synth-plugin");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("main.rs"), b"fn main() {}\n").unwrap();

    let backend = FilesBackend::new(data.path(), vault.path()).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(
        LayerRouter::new().merge(files_service_layer(backend.clone())),
        scope.clone(),
    );
    let client: FilesServiceClient = local.establish().await.expect("establish client");

    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Synth Plugin".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software)");
    let cp1 = client
        .checkpoint_now(root.id, Some("v1".into()))
        .await
        .expect("checkpoint_now rpc");

    // Naming resolves against git's objects through jj's `Backend`
    // trait, exactly as the chain does.
    let named = client
        .name_version(root.id, cp1.commit_id.clone(), "v1 for review".into())
        .await
        .expect("name_version on a software root");
    assert_eq!(named.commit_id, cp1.commit_id);
    assert!(!named.change_id.is_empty());

    // The name rides the chain, and the entity is an ordinary vault
    // page — same as any media root.
    std::fs::write(root_dir.join("main.rs"), b"fn main() { todo!() }\n").unwrap();
    client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint_now rpc");
    let chain = client
        .chain(root.id, "main.rs".into())
        .await
        .expect("chain rpc");
    assert_eq!(chain.len(), 2, "{chain:?}");
    assert_eq!(
        chain
            .iter()
            .find(|e| e.commit_id == cp1.commit_id)
            .map(|e| e.names.clone()),
        Some(vec!["v1 for review".to_string()]),
    );
    assert!(
        vault_pages(vault.path())
            .iter()
            .any(|(p, _)| p.starts_with("Files/synth-plugin/versions/")),
    );

    // Project Versions too — numbering is Vault-side, not store-side.
    let pv = client
        .start_project_version(root.id, Some("rewrite".into()))
        .await
        .expect("start_project_version on a software root");
    assert_eq!(pv.number, 1);

    // Share-link targeting still resolves to the exact change.
    let resolved = client
        .resolve_named_version(named.id)
        .await
        .expect("resolve_named_version rpc");
    assert_eq!(resolved.commit_id, cp1.commit_id);

    // The sweep is the one verb that is genuinely media-only.
    let err = client
        .gc_root(root.id, None)
        .await
        .expect_err("gc_root must refuse a software root");
    let message = format!("{err}");
    assert!(
        message.contains("software root") && message.contains("git gc"),
        "the refusal has to say why and what collects instead: {message}"
    );

    backend.shutdown().await;
    drop(client);
    scope.close().await;
}

/// Issue #270 AC 2's durability half: the version a review comment
/// pins is a GC protect-set member exactly like a Named Version — and
/// deleting the comment releases it (the protect set is read from the
/// Vault every pass, never baked in).
#[tokio::test(flavor = "multi_thread")]
async fn gc_never_sweeps_a_commit_a_review_comment_pins() {
    let fx = fixture("Review Retention").await;
    let root_id = fx.root_id().await;

    // A tracked file, so the review can exist at all.
    std::fs::write(fx.root_dir.join("cut.mov"), vec![7u8; 1024]).unwrap();
    fx.client
        .checkpoint_now(root_id, None)
        .await
        .expect("checkpoint");

    let (pinned_commit, pinned_chunk, swept_chunk) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    let (pinned, pinned_chunk) =
                        write_unreachable_commit(vs, "client-cut.mov", b"the reviewed cut").await;
                    let (_swept, swept_chunk) =
                        write_unreachable_commit(vs, "scratch.mov", b"nobody pinned this").await;
                    (pinned, pinned_chunk, swept_chunk)
                })
            })
            .expect("with_version_store")
    });

    let review = fx
        .client
        .review_for_file(root_id, "cut.mov".into())
        .await
        .expect("review");
    fx.client
        .add_review_comment(
            review.id,
            files_proto::NewReviewComment {
                timecode_secs: 4.0,
                author: "Client".into(),
                body: "this is the take".into(),
                commit_id: pinned_commit.hex(),
                annotation: Vec::new(),
            },
        )
        .await
        .expect("comment pinning the old version");

    tokio::time::sleep(Duration::from_millis(20)).await;
    let report = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect("gc_root rpc");
    assert_eq!(
        report.protected_commits, 1,
        "the comment's pin joined the protect set: {report:?}"
    );

    let (pinned_present, swept_present) = tokio::task::block_in_place(|| {
        fx.backend
            .with_version_store(root_id, |vs| {
                pollster::block_on(async {
                    (
                        vs.chunks().has(pinned_chunk).await,
                        vs.chunks().has(swept_chunk).await,
                    )
                })
            })
            .expect("with_version_store")
    });
    assert!(
        pinned_present,
        "a commented version's content survives GC (AC 2: the reference stays resolvable)"
    );
    assert!(!swept_present, "the unpinned commit's content is gone");

    // Delete the comment and the same pass now sweeps the pin.
    let comments = fx
        .client
        .review_comments(review.id)
        .await
        .expect("comments");
    fx.client
        .delete_review_comment(comments[0].id)
        .await
        .expect("delete comment");
    let report = fx
        .client
        .gc_root(root_id, Some(0))
        .await
        .expect("gc_root rpc");
    assert_eq!(report.protected_commits, 0);

    fx.backend.shutdown().await;
}
