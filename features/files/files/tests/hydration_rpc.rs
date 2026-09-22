//! Pointer stubs and hydration (issue #263) at the spec's primary
//! seam: the Files lanes over an in-process `architect::LocalServer`
//! memory link. One test per acceptance criterion, plus the invariants
//! that make dehydration safe: a checkpoint never ingests stub bytes or
//! records a stub as a deletion, and a dirty file can never be
//! dehydrated.

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, FilesFault, RootFlavor};
use files_proto::service::roots::AdoptRequest;
use files_proto::{
    RootId, RootPath, RootsServiceClient, SyncServiceClient, TreeServiceClient,
    VersionServiceClient,
};

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new()
        .merge(files_proto::roots_layer(backend.clone()))
        .merge(files_proto::tree_layer(backend.clone()))
        .merge(files_proto::version_layer(backend.clone()))
        .merge(files_proto::sync_layer(backend))
}

struct Clients {
    roots: RootsServiceClient,
    tree: TreeServiceClient,
    version: VersionServiceClient,
    sync: SyncServiceClient,
}

async fn clients(local: &LocalServer) -> Clients {
    Clients {
        roots: local.establish().await.expect("roots client"),
        tree: local.establish().await.expect("tree client"),
        version: local.establish().await.expect("version client"),
        sync: local.establish().await.expect("sync client"),
    }
}

struct Rig {
    _data_dir: tempfile::TempDir,
    root_dir: std::path::PathBuf,
    root_id: RootId,
    backend: FilesBackend,
    client: Clients,
    _local: LocalServer,
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

/// Adopt `dir` over the wire and wait for the adoption walk, so the
/// first checkpoint does not race it.
///
/// A survey (`hash_content: false`) rather than a full adoption: hashing
/// ends in an "adopted" checkpoint of its own, and these tests take the
/// first checkpoint themselves so they can assert what it recorded.
async fn adopt(
    backend: &FilesBackend,
    client: &Clients,
    dir: &std::path::Path,
    name: &str,
    flavor: RootFlavor,
) -> RootId {
    let root = client
        .roots
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: name.into(),
            flavor,
            hash_content: false,
        })
        .await
        .expect("adopt");
    let id = RootId::new(root.id);
    backend.settled(id).await;
    id
}

/// The message a refusal carries. Refusals of this kind arrive as
/// `Invalid`; anything else is printed whole so the assertion names it.
fn refusal(err: vox::VoxError<FilesFault>) -> String {
    match err {
        vox::VoxError::User(fault) => match *fault {
            FilesFault::Invalid(m) => m,
            other => format!("{other:?}"),
        },
        other => format!("{other:?}"),
    }
}

/// One media root with two files, checkpointed once.
async fn rig() -> Rig {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("mix.wav"), vec![0x11u8; 64 * 1024]).unwrap();
    std::fs::create_dir(root_dir.join("stems")).unwrap();
    std::fs::write(
        root_dir.join("stems").join("kick.wav"),
        vec![0x22u8; 32 * 1024],
    )
    .unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client = clients(&local).await;

    let root_id = adopt(&backend, &client, &root_dir, "session", RootFlavor::Media).await;
    client
        .version
        .checkpoint(root_id, None)
        .await
        .expect("first checkpoint");
    Rig {
        _data_dir: data_dir,
        root_dir,
        root_id,
        backend,
        client,
        _local: local,
    }
}

impl Rig {
    async fn browse(&self, dir: &str) -> Vec<files::BrowseEntry> {
        self.client
            .tree
            .browse(self.root_id, p(dir))
            .await
            .expect("browse")
    }

    /// Release a file's bytes to a stub, returning its listing entry.
    async fn dehydrate(&self, path: &str) -> files::BrowseEntry {
        let done = self
            .client
            .sync
            .hydrate(self.root_id, vec![p(path)], false)
            .await
            .expect("dehydrate");
        assert_eq!(done, vec![p(path)]);
        self.listed(path).await
    }

    /// Fetch a file's bytes back, returning its listing entry.
    async fn hydrate(&self, path: &str) -> files::BrowseEntry {
        let done = self
            .client
            .sync
            .hydrate(self.root_id, vec![p(path)], true)
            .await
            .expect("hydrate");
        assert_eq!(done, vec![p(path)]);
        self.listed(path).await
    }

    /// A file's entry as its parent's listing reports it.
    async fn listed(&self, path: &str) -> files::BrowseEntry {
        let (dir, name) = path.rsplit_once('/').unwrap_or(("", path));
        entry(&self.browse(dir).await, name).clone()
    }
}

fn entry<'a>(entries: &'a [files::BrowseEntry], name: &str) -> &'a files::BrowseEntry {
    entries
        .iter()
        .find(|e| e.name == name)
        .unwrap_or_else(|| panic!("{name} missing from listing"))
}

/// AC 1: "Dehydrate a file: tree shows a stub; logical size and
/// identity are preserved in listings."
#[tokio::test(flavor = "multi_thread")]
async fn dehydrate_preserves_size_and_identity_in_listings() {
    let rig = rig().await;
    let chain_before = rig
        .client
        .version
        .chain(rig.root_id, p("mix.wav"))
        .await
        .expect("chain before");

    let dehydrated = rig.dehydrate("mix.wav").await;
    assert!(dehydrated.stub);
    assert_eq!(dehydrated.size, Some(64 * 1024), "logical size preserved");

    // On disk: a small placeholder, honest to a raw reader.
    let on_disk = std::fs::read(rig.root_dir.join("mix.wav")).unwrap();
    assert!(on_disk.len() < 4096);
    assert!(on_disk.starts_with(files::stub::MAGIC.as_bytes()));

    // In listings: still the file, logical size, stub-flagged.
    let listed = rig.browse("").await;
    let mix = entry(&listed, "mix.wav");
    assert!(mix.stub);
    assert_eq!(mix.size, Some(64 * 1024));

    // Identity: the version chain is untouched by dehydration.
    let chain_after = rig
        .client
        .version
        .chain(rig.root_id, p("mix.wav"))
        .await
        .expect("chain after");
    assert_eq!(chain_before, chain_after);
}

/// AC 2: "Hydrate on demand restores exact content (verified by
/// FileId)" — bit-exact round trip.
#[tokio::test(flavor = "multi_thread")]
async fn hydrate_restores_exact_content() {
    let rig = rig().await;
    let original = std::fs::read(rig.root_dir.join("mix.wav")).unwrap();

    rig.dehydrate("mix.wav").await;
    let hydrated = rig.hydrate("mix.wav").await;
    assert!(!hydrated.stub);
    assert_eq!(hydrated.size, Some(original.len() as u64));
    assert_eq!(
        std::fs::read(rig.root_dir.join("mix.wav")).unwrap(),
        original,
        "hydrated bytes are the dehydrated bytes"
    );

    // Idempotent both ways: hydrating a resident file is a no-op.
    let again = rig.hydrate("mix.wav").await;
    assert!(!again.stub);
}

/// AC 3: "Policy patterns auto-hydrate matching paths and keep the
/// rest dehydrated."
#[tokio::test(flavor = "multi_thread")]
async fn policy_hydrates_matches_and_dehydrates_the_rest() {
    let rig = rig().await;
    // Start fully dehydrated.
    rig.dehydrate("mix.wav").await;
    rig.dehydrate("stems/kick.wav").await;

    // Policy: keep the stems hydrated, nothing else.
    rig.client
        .sync
        .set_residency(rig.root_id, vec!["stems/".into()])
        .await
        .expect("set policy");
    let report = rig
        .client
        .sync
        .apply_residency(rig.root_id)
        .await
        .expect("apply");
    assert_eq!(report.hydrated, vec!["stems/kick.wav".to_string()]);
    assert_eq!(report.dehydrated, Vec::<String>::new());
    assert_eq!(report.skipped_dirty, Vec::<String>::new());

    let listed = rig.browse("stems").await;
    assert!(!entry(&listed, "kick.wav").stub, "match is resident");
    let listed = rig.browse("").await;
    assert!(entry(&listed, "mix.wav").stub, "non-match stays dehydrated");

    // Re-hydrate mix by hand, re-apply: the policy dehydrates it again
    // (clean, non-matching), and the stems stay put.
    rig.hydrate("mix.wav").await;
    let report = rig
        .client
        .sync
        .apply_residency(rig.root_id)
        .await
        .expect("re-apply");
    assert_eq!(report.dehydrated, vec!["mix.wav".to_string()]);
    assert_eq!(report.hydrated, Vec::<String>::new());
}

/// The empty policy is opt-in: applying it touches nothing.
#[tokio::test(flavor = "multi_thread")]
async fn empty_policy_touches_nothing() {
    let rig = rig().await;
    let report = rig
        .client
        .sync
        .apply_residency(rig.root_id)
        .await
        .expect("apply empty policy");
    assert_eq!(report.hydrated, Vec::<String>::new());
    assert_eq!(report.dehydrated, Vec::<String>::new());
    let listed = rig.browse("").await;
    assert!(!entry(&listed, "mix.wav").stub);
}

/// AC 4 is asserted throughout via `BrowseEntry::stub`; this pins the
/// "without opening files" half observably: a directory full of large
/// media lists with stub=false and true sizes, while the one stub in
/// it is flagged with its logical size.
#[tokio::test(flavor = "multi_thread")]
async fn listings_distinguish_resident_from_stub() {
    let rig = rig().await;
    rig.dehydrate("stems/kick.wav").await;
    let listed = rig.browse("stems").await;
    let kick = entry(&listed, "kick.wav");
    assert!(kick.stub);
    assert_eq!(kick.size, Some(32 * 1024));

    let top = rig.browse("").await;
    let mix = entry(&top, "mix.wav");
    assert!(!mix.stub);
    assert_eq!(mix.size, Some(64 * 1024));
}

/// The invariant everything above depends on: a checkpoint over a
/// dehydrated tree records NOTHING — stub bytes never become content,
/// and a stub is never a deletion. History is identical before and
/// after a checkpoint taken mid-dehydration.
#[tokio::test(flavor = "multi_thread")]
async fn checkpoint_over_stubs_is_a_no_op() {
    let rig = rig().await;
    rig.dehydrate("mix.wav").await;

    let info = rig
        .client
        .version
        .checkpoint(rig.root_id, Some("mid-dehydration".into()))
        .await
        .expect("checkpoint over stubs");
    assert_eq!(
        info.changed_paths,
        Vec::<String>::new(),
        "a stub is neither content nor a deletion"
    );

    // And hydration back out still restores the original content.
    rig.hydrate("mix.wav").await;
    assert_eq!(
        std::fs::read(rig.root_dir.join("mix.wav")).unwrap(),
        vec![0x11u8; 64 * 1024]
    );
}

/// Dehydration must never destroy unversioned work: a file whose
/// content moved past the checkpoint head is refused, and the policy
/// pass reports it instead of touching it.
#[tokio::test(flavor = "multi_thread")]
async fn dirty_files_are_never_dehydrated() {
    let rig = rig().await;
    std::fs::write(rig.root_dir.join("mix.wav"), b"unversioned edit").unwrap();

    let err = rig
        .client
        .sync
        .hydrate(rig.root_id, vec![p("mix.wav")], false)
        .await
        .expect_err("dehydrating a dirty file must fail");
    let message = refusal(err);
    assert!(
        message.contains("checkpoint first"),
        "unexpected error: {message}"
    );
    assert_eq!(
        std::fs::read(rig.root_dir.join("mix.wav")).unwrap(),
        b"unversioned edit",
        "the dirty file is untouched"
    );

    // The policy pass reports rather than errors.
    rig.client
        .sync
        .set_residency(rig.root_id, vec!["stems/".into()])
        .await
        .expect("set policy");
    let report = rig
        .client
        .sync
        .apply_residency(rig.root_id)
        .await
        .expect("apply with a dirty file present");
    assert_eq!(report.skipped_dirty, vec!["mix.wav".to_string()]);

    rig.backend.shutdown().await;
}

/// One malformed stub-shaped file (magic line, garbage body) must
/// never take down checkpoints or listings for the whole root — it is
/// handled as the ordinary content its bytes are (PR #289 review).
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_stub_shaped_file_is_ordinary_content() {
    let rig = rig().await;
    let weird = rig.root_dir.join("notes.txt");
    let bytes = format!("{}not json at all", files::stub::MAGIC);
    std::fs::write(&weird, &bytes).unwrap();

    // Checkpoint versions it as its literal bytes rather than erroring.
    let info = rig
        .client
        .version
        .checkpoint(rig.root_id, None)
        .await
        .expect("checkpoint with a malformed stub-shaped file present");
    assert_eq!(info.changed_paths, vec!["notes.txt".to_string()]);

    // Browse lists it unflagged, with its literal size.
    let listed = rig.browse("").await;
    let it = entry(&listed, "notes.txt");
    assert!(!it.stub);
    assert_eq!(it.size, Some(bytes.len() as u64));
}

/// Stubs don't exist on the software flavor: a stub-shaped file in a
/// git root is versioned as its literal bytes, never silently excluded
/// (PR #289 review) — and dehydrate refuses the flavor outright.
#[tokio::test(flavor = "multi_thread")]
async fn software_roots_have_no_stubs() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("repo");
    std::fs::create_dir(&root_dir).unwrap();
    let stubish = files::stub::Stub::new(&jj_lib::backend::FileId::new(vec![0xab; 32]), 999, false);
    std::fs::write(root_dir.join("fixture.stub"), stubish.to_bytes()).unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client = clients(&local).await;
    let root_id = adopt(&backend, &client, &root_dir, "repo", RootFlavor::Software).await;
    let info = client
        .version
        .checkpoint(root_id, None)
        .await
        .expect("checkpoint");
    assert_eq!(info.changed_paths, vec!["fixture.stub".to_string()]);

    let listed = client.tree.browse(root_id, RootPath::root()).await.unwrap();
    assert!(
        !entry(&listed, "fixture.stub").stub,
        "no stubs on git roots"
    );

    let err = client
        .sync
        .hydrate(root_id, vec![p("fixture.stub")], false)
        .await
        .expect_err("dehydrate is media-only");
    let message = refusal(err);
    assert!(message.contains("media-only"), "{message}");
    backend.shutdown().await;
}
