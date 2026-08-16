//! `CurationService` — the curated-version lane, against a real
//! `FilesBackend`.
//!
//! In-process rather than over `LocalServer`, for the reason
//! `roots_lane.rs` gives: the v2 lanes have no `permits.rs` rows yet, so
//! nothing is mounted on a router and the gate fails closed. These call
//! the trait directly, which is what the dispatcher would do.
//!
//! The v1 surface for the same entities is covered end-to-end over RPC
//! in `versions_rpc.rs` (vault pages, GC protection, replication) and
//! `restart_rpc.rs`. What is tested here is what this lane adds and
//! nothing it merely forwards: typed ids in, typed faults out, and the
//! two filters v1 had no method for — names of one *path*, and
//! resolving a *name*.

use files::FilesBackend;
use files_proto::id::{ProjectVersionId, RootId, VersionId};
use files_proto::model::{RestartMode, RootFlavor};
use files_proto::service::curation::CurationService;
use files_proto::service::legacy::FilesService as LegacyFiles;
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::{FilesFault, RootPath};

/// A media root with two files, adopted and ready to checkpoint.
struct Rig {
    _tmp: tempfile::TempDir,
    dir: std::path::PathBuf,
    backend: FilesBackend,
    root: RootId,
}

async fn rig() -> Rig {
    let tmp = tempfile::tempdir().expect("data tempdir");
    let dir = tmp.path().join("mix-session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();

    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: "Mix Session".into(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .expect("adopt");

    Rig {
        _tmp: tmp,
        dir,
        backend,
        root: RootId::new(root.id),
    }
}

impl Rig {
    /// Certify a checkpoint and hand back the [`VersionId`] addressing
    /// it — the leading 128 bits of its commit id, which is how a
    /// caller reading a chain entry would mint one.
    async fn checkpoint(&self, why: &str) -> VersionId {
        let info = LegacyFiles::checkpoint_now(&self.backend, self.root.get(), Some(why.into()))
            .await
            .expect("checkpoint");
        version_of(&info.commit_id)
    }

    async fn names(&self) -> Vec<String> {
        CurationService::named_versions(&self.backend, self.root, None)
            .await
            .expect("named_versions")
            .into_iter()
            .map(|n| n.name)
            .collect()
    }
}

fn version_of(commit_hex: &str) -> VersionId {
    VersionId::new(uuid::Uuid::try_parse(&commit_hex[..32]).expect("a commit id fills 32 hex"))
}

// t[verify files.version.cadence] — "any version can be named after the
// fact": the naming here happens on a checkpoint that closed before the
// name existed, which is the whole point.
#[tokio::test(flavor = "multi_thread")]
async fn a_checkpoint_is_named_after_the_fact_and_is_then_addressable_by_name() {
    let rig = rig().await;
    let version = rig.checkpoint("first pass").await;

    let named =
        CurationService::name_version(&rig.backend, rig.root, version, "v3 for client".into())
            .await
            .expect("name_version");
    assert_eq!(named.name, "v3 for client");
    assert_eq!(named.root_id, rig.root.get());
    assert!(
        !named.path.is_empty(),
        "the server fills in the entity's own vault page path"
    );
    assert!(
        !named.change_id.is_empty(),
        "and records the stable half of the reference, not only the commit"
    );

    assert_eq!(rig.names().await, vec!["v3 for client".to_string()]);

    let resolved = CurationService::resolve_name(&rig.backend, rig.root, "v3 for client".into())
        .await
        .expect("resolve_name");
    assert_eq!(resolved.id, named.id);
    assert!(
        resolved.commit_id.starts_with(&commit_prefix(version)),
        "resolving a name lands on the commit it was given: {} vs {}",
        resolved.commit_id,
        commit_prefix(version)
    );
}

fn commit_prefix(version: VersionId) -> String {
    version.get().simple().to_string()
}

/// Unnaming hands back the entity it removed, so a subscriber can drop
/// one row rather than refetch the list.
#[tokio::test(flavor = "multi_thread")]
async fn unnaming_returns_what_it_removed() {
    let rig = rig().await;
    let version = rig.checkpoint("first pass").await;
    let named = CurationService::name_version(&rig.backend, rig.root, version, "keep me".into())
        .await
        .expect("name_version");

    let removed = CurationService::unname_version(&rig.backend, rig.root, version)
        .await
        .expect("unname_version");
    assert_eq!(removed.id, named.id);
    assert_eq!(removed.name, "keep me");
    assert!(
        rig.names().await.is_empty(),
        "the name is gone; the version it pointed at is not this lane's to delete"
    );
    assert!(
        rig.dir.join("mix.wav").exists(),
        "and nothing in the live tree was touched"
    );
}

/// The entity's own id addresses it too — a client holding a
/// `NamedVersion` should not have to work out which of its three ids we
/// meant.
#[tokio::test(flavor = "multi_thread")]
async fn a_named_version_is_reachable_by_its_entity_id() {
    let rig = rig().await;
    let version = rig.checkpoint("first pass").await;
    let named = CurationService::name_version(&rig.backend, rig.root, version, "by entity".into())
        .await
        .expect("name_version");

    let removed =
        CurationService::unname_version(&rig.backend, rig.root, VersionId::new(named.id))
            .await
            .expect("unname by entity id");
    assert_eq!(removed.id, named.id);
}

/// A Named Version records a commit, not a path — so "names of this
/// path" means the names sitting on commits in that path's chain.
#[tokio::test(flavor = "multi_thread")]
async fn names_filter_to_one_path_through_its_chain() {
    let rig = rig().await;
    let first = rig.checkpoint("both files").await;
    std::fs::write(rig.dir.join("stems").join("kick.wav"), b"boom boom").unwrap();
    let second = rig.checkpoint("kick only").await;
    assert_ne!(first, second, "the second checkpoint is its own commit");

    CurationService::name_version(&rig.backend, rig.root, first, "session start".into())
        .await
        .expect("name first");
    CurationService::name_version(&rig.backend, rig.root, second, "kick v2".into())
        .await
        .expect("name second");

    let kick = RootPath::parse("stems/kick.wav").unwrap();
    let on_kick: Vec<String> = CurationService::named_versions(&rig.backend, rig.root, Some(kick))
        .await
        .expect("named_versions for a path")
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert!(
        on_kick.contains(&"kick v2".to_string()),
        "the commit that changed the file is in its chain: {on_kick:?}"
    );

    let mix = RootPath::parse("mix.wav").unwrap();
    let on_mix: Vec<String> = CurationService::named_versions(&rig.backend, rig.root, Some(mix))
        .await
        .expect("named_versions for a path")
        .into_iter()
        .map(|n| n.name)
        .collect();
    assert!(
        !on_mix.contains(&"kick v2".to_string()),
        "a checkpoint that never touched mix.wav is not one of its versions: {on_mix:?}"
    );

    assert_eq!(
        rig.names().await.len(),
        2,
        "unfiltered still answers for the whole root"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unresolvable_name_is_a_typed_refusal() {
    let rig = rig().await;
    rig.checkpoint("first pass").await;
    let err = CurationService::resolve_name(&rig.backend, rig.root, "never named".into())
        .await
        .expect_err("no such name");
    assert!(matches!(err, FilesFault::Invalid(_)), "got {err:?}");
}

/// v1 answered `NotFound(String)` for both of these and the caller had
/// to read the prose to tell them apart.
#[tokio::test(flavor = "multi_thread")]
async fn absent_roots_and_absent_versions_are_different_faults() {
    let rig = rig().await;
    let ghost = RootId::generate();

    match CurationService::named_versions(&rig.backend, ghost, None)
        .await
        .expect_err("no such root")
    {
        FilesFault::RootNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected RootNotFound, got {other:?}"),
    }
    match CurationService::project_versions(&rig.backend, ghost)
        .await
        .expect_err("no such root")
    {
        FilesFault::RootNotFound(id) => assert_eq!(id, ghost),
        other => panic!("expected RootNotFound, got {other:?}"),
    }

    let nowhere = VersionId::generate();
    match CurationService::name_version(&rig.backend, rig.root, nowhere, "ghost".into())
        .await
        .expect_err("no such commit")
    {
        FilesFault::VersionNotFound(id) => assert_eq!(id, nowhere),
        other => panic!("expected VersionNotFound, got {other:?}"),
    }
    match CurationService::unname_version(&rig.backend, rig.root, nowhere)
        .await
        .expect_err("no such named version")
    {
        FilesFault::VersionNotFound(id) => assert_eq!(id, nowhere),
        other => panic!("expected VersionNotFound, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nameless_name_is_refused() {
    let rig = rig().await;
    let version = rig.checkpoint("first pass").await;
    let err = CurationService::name_version(&rig.backend, rig.root, version, "   ".into())
        .await
        .expect_err("a Named Version needs a name");
    assert!(matches!(err, FilesFault::Invalid(_)), "got {err:?}");
}

/// Numbers are the identity of a Project Version: 1-based, per root,
/// never reused. The label is decoration on top.
#[tokio::test(flavor = "multi_thread")]
async fn project_versions_number_themselves_from_one() {
    let rig = rig().await;
    rig.checkpoint("first pass").await;

    let first =
        CurationService::start_project_version(&rig.backend, rig.root, "Client remix".into())
            .await
            .expect("start_project_version");
    assert_eq!(first.number, 1);
    assert_eq!(first.label.as_deref(), Some("Client remix"));
    assert_eq!(first.root_id, rig.root.get());

    let second = CurationService::start_project_version(&rig.backend, rig.root, "  ".into())
        .await
        .expect("an unlabelled iteration is still an iteration");
    assert_eq!(second.number, 2);
    assert_eq!(
        second.label, None,
        "whitespace is no label, and no label is not an error"
    );

    let listed = CurationService::project_versions(&rig.backend, rig.root)
        .await
        .expect("project_versions");
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().any(|pv| pv.id == first.id));
}

/// Restarting mints the next iteration and carries the restarted one's
/// label across — "begin again" keeps the name of what began. The
/// number never rides along.
#[tokio::test(flavor = "multi_thread")]
async fn restarting_begins_the_next_iteration_under_the_same_label() {
    let rig = rig().await;
    rig.checkpoint("first pass").await;
    let first = CurationService::start_project_version(&rig.backend, rig.root, "Album cut".into())
        .await
        .expect("start_project_version");

    // The picker's default: everything minus the Ignore set, i.e. a
    // pure lineage cut with no tree change.
    let next = CurationService::restart_project_version(
        &rig.backend,
        rig.root,
        ProjectVersionId::new(first.id),
        RestartMode::CarryForward { paths: vec![] },
    )
    .await
    .expect("restart_project_version");

    assert_eq!(next.number, 2, "a restart never reuses a number");
    assert_eq!(next.label.as_deref(), Some("Album cut"));
    assert_ne!(next.id, first.id);
    assert!(
        rig.dir.join("mix.wav").exists() && rig.dir.join("stems").join("kick.wav").exists(),
        "carrying everything forward leaves the tree exactly as it was"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn restarting_something_that_was_never_started_is_a_typed_fault() {
    let rig = rig().await;
    rig.checkpoint("first pass").await;
    let ghost = ProjectVersionId::generate();

    match CurationService::restart_project_version(
        &rig.backend,
        rig.root,
        ghost,
        RestartMode::CarryForward { paths: vec![] },
    )
    .await
    .expect_err("no such project version")
    {
        FilesFault::VersionNotFound(id) => assert_eq!(id.get(), ghost.get()),
        other => panic!("expected VersionNotFound, got {other:?}"),
    }
}
