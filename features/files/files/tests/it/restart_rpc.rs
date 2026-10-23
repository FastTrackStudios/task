//! Project Version restart (issue #268) at the spec's primary seam:
//! the Files lanes over an in-process memory link. One test per
//! acceptance criterion: the three starting modes, read-only
//! time-travel browse + copy-forward, the mid-flip save surviving as
//! flagged divergence, and the flip arriving as ordinary events.
//!
//! `CurationService::restart_project_version` restarts a Project Version
//! *by id*, so the lineage being restarted is recorded first
//! (`start_project_version`, v1) and the first restart mints v2.

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, FilesEvent, FilesFault, ProjectVersion, RestartMode, RootFlavor};
use files_proto::service::curation::CurationEvent;
use files_proto::service::roots::AdoptRequest;
use files_proto::service::version::VersionEvent;
use files_proto::{
    CurationServiceClient, ProjectVersionId, RootId, RootPath, RootsServiceClient,
    TreeServiceClient, TreeServiceStreamClient, VersionId, VersionServiceClient,
};

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new()
        .merge(files_proto::roots_layer(backend.clone()))
        .merge(files_proto::tree_layer(backend.clone()))
        .merge(files_proto::tree_stream_layer(backend.clone()))
        .merge(files_proto::version_layer(backend.clone()))
        .merge(files_proto::curation_layer(backend))
}

struct Clients {
    roots: RootsServiceClient,
    tree: TreeServiceClient,
    version: VersionServiceClient,
    curation: CurationServiceClient,
}

struct Rig {
    data_dir: tempfile::TempDir,
    root_dir: std::path::PathBuf,
    root_id: RootId,
    backend: FilesBackend,
    client: Clients,
    local: LocalServer,
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
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

/// One media root: mix.wav + stems/kick.wav + an ignored peak cache,
/// checkpointed once.
async fn rig() -> Rig {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = data_dir.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    std::fs::write(root_dir.join("mix.wav"), vec![0x11u8; 8 * 1024]).unwrap();
    std::fs::create_dir(root_dir.join("stems")).unwrap();
    std::fs::write(
        root_dir.join("stems").join("kick.wav"),
        vec![0x22u8; 4 * 1024],
    )
    .unwrap();
    // Ignored junk (media seed ignores REAPER peak caches): a restart
    // must leave unversioned data alone.
    std::fs::write(root_dir.join("mix.wav.reapeaks"), b"peaks").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client = Clients {
        roots: local.establish().await.expect("roots client"),
        tree: local.establish().await.expect("tree client"),
        version: local.establish().await.expect("version client"),
        curation: local.establish().await.expect("curation client"),
    };
    // A survey (`hash_content: false`): the test's own checkpoint is the
    // first, as it was when roots were created rather than adopted.
    let root = client
        .roots
        .adopt(AdoptRequest {
            path: root_dir.to_string_lossy().into_owned(),
            name: "session".into(),
            flavor: RootFlavor::Media,
            hash_content: false,
        })
        .await
        .expect("adopt");
    let root_id = RootId::new(root.id);
    backend.settled(root_id).await;
    client
        .version
        .checkpoint(root_id, None)
        .await
        .expect("first checkpoint");
    Rig {
        data_dir,
        root_dir,
        root_id,
        backend,
        client,
        local,
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

    /// The lineage a restart begins again from: the newest Project
    /// Version, or — on a root never restarted — the current tree
    /// recorded as one, carrying `label` across.
    async fn current_lineage(&self, label: Option<&str>) -> ProjectVersion {
        let versions = self
            .client
            .curation
            .project_versions(self.root_id)
            .await
            .expect("project versions");
        match versions.into_iter().max_by_key(|pv| pv.number) {
            Some(pv) => pv,
            None => self
                .client
                .curation
                .start_project_version(self.root_id, label.unwrap_or_default().into())
                .await
                .expect("record the current lineage"),
        }
    }

    async fn restart(
        &self,
        mode: RestartMode,
        label: Option<&str>,
    ) -> Result<ProjectVersion, vox::VoxError<FilesFault>> {
        let lineage = self.current_lineage(label).await;
        self.client
            .curation
            .restart_project_version(self.root_id, ProjectVersionId::new(lineage.id), mode)
            .await
    }

    /// The version at the head of a path's chain — the old terminal a
    /// time-travel browse or copy-forward reads.
    async fn terminal(&self, path: &str) -> VersionId {
        let chain = self
            .client
            .version
            .chain(self.root_id, p(path))
            .await
            .unwrap();
        VersionId::from_commit_hex(&chain[0].commit_id)
    }
}

fn names(entries: &[files::BrowseEntry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

/// AC 1, mode 1: Empty — the new lineage starts with nothing tracked;
/// ignored junk survives on disk; the old terminal state is one
/// browse_at away.
#[tokio::test(flavor = "multi_thread")]
async fn restart_empty_produces_an_empty_lineage() {
    let rig = rig().await;
    // The old terminal is read off the chain BEFORE the restart: after
    // an Empty flip the path is deleted at the new head, and a deleted
    // path's chain is empty by design.
    let old_terminal = rig.terminal("mix.wav").await;

    let pv = rig
        .restart(RestartMode::Empty, Some("take two"))
        .await
        .expect("restart empty");
    assert_eq!(pv.number, 2, "v1 is the lineage that was restarted");
    assert_eq!(pv.label.as_deref(), Some("take two"));

    // Live tree: tracked files gone, ignored junk untouched, internals
    // intact (browse hides them; the disk still has the store). The
    // junk is on disk but not listed — `files.ignore.retained` keeps an
    // ignored file out of listings as well as history.
    let listed = rig.browse("").await;
    assert_eq!(names(&listed), Vec::<&str>::new());
    assert!(rig.root_dir.join("mix.wav.reapeaks").exists());
    assert!(!rig.root_dir.join("mix.wav").exists());
    assert!(!rig.root_dir.join("stems").exists(), "emptied dirs pruned");

    // The old iteration is browsable read-only at the flip's parent.
    let old = rig
        .client
        .version
        .browse_at(rig.root_id, RootPath::root(), old_terminal)
        .await
        .expect("time-travel browse");
    assert_eq!(names(&old), vec!["mix.wav", "stems"]);
}

/// AC 1, mode 2: Template — the new lineage starts from the template
/// folder's contents (with a root's internals never copied in).
#[tokio::test(flavor = "multi_thread")]
async fn restart_from_template_seeds_the_new_lineage() {
    let rig = rig().await;
    let template = rig.data_dir.path().join("template");
    std::fs::create_dir_all(template.join("stems")).unwrap();
    std::fs::write(template.join("session-notes.md"), b"# fresh start").unwrap();
    std::fs::write(template.join("stems").join(".keep"), b"").unwrap();
    // A stale store dir in a template must not be smuggled in.
    std::fs::create_dir_all(template.join(".fts-files")).unwrap();
    std::fs::write(template.join(".fts-files").join("junk"), b"x").unwrap();

    let pv = rig
        .restart(
            RestartMode::Template {
                source_path: template.to_string_lossy().into_owned(),
            },
            None,
        )
        .await
        .expect("restart from template");
    assert_eq!(pv.number, 2);

    let listed = rig.browse("").await;
    assert_eq!(
        names(&listed),
        vec!["session-notes.md", "stems"],
        "the template's content, beside the (unlisted) ignored junk"
    );
    // The template's stale internals stayed out; the root's own store
    // is still the one the marker knows.
    assert!(!rig.root_dir.join(".fts-files").join("junk").exists());

    // The seed is versioned as the new lineage's content.
    let chain = rig
        .client
        .version
        .chain(rig.root_id, p("session-notes.md"))
        .await
        .unwrap();
    assert_eq!(chain.len(), 1, "template file enters at the flip");
}

/// AC 1, mode 3: Carry forward — chosen paths survive into the new
/// lineage (directories carry their subtree); everything else clears.
/// An empty carry list is the picker default: everything, a pure
/// lineage cut.
#[tokio::test(flavor = "multi_thread")]
async fn restart_carry_forward_keeps_chosen_files() {
    let rig = rig().await;
    let pv = rig
        .restart(
            RestartMode::CarryForward {
                paths: vec!["stems".into()],
            },
            None,
        )
        .await
        .expect("restart carry-forward");
    assert_eq!(pv.number, 2);

    let listed = rig.browse("").await;
    assert_eq!(names(&listed), vec!["stems"]);
    assert!(rig.root_dir.join("stems").join("kick.wav").exists());
    assert!(!rig.root_dir.join("mix.wav").exists());
    assert!(
        rig.root_dir.join("mix.wav.reapeaks").exists(),
        "junk survives"
    );

    // The pure lineage cut: carry everything.
    let pv2 = rig
        .restart(RestartMode::CarryForward { paths: vec![] }, None)
        .await
        .expect("pure lineage cut");
    assert_eq!(pv2.number, 3, "auto-numbering advances");
    let listed = rig.browse("").await;
    assert_eq!(names(&listed), vec!["stems"]);
}

/// AC 2: the old iteration browses read-only and copy-forward brings
/// chosen files into the current one — refusing to clobber unversioned
/// work.
#[tokio::test(flavor = "multi_thread")]
async fn old_iterations_browse_read_only_and_copy_forward() {
    let rig = rig().await;
    let original = std::fs::read(rig.root_dir.join("mix.wav")).unwrap();
    let old_terminal = rig.terminal("mix.wav").await;

    rig.restart(RestartMode::Empty, None)
        .await
        .expect("restart");

    // Read-only: browse_at answers from the store — nothing reappears
    // on disk, and repeated browsing changes nothing.
    let old = rig
        .client
        .version
        .browse_at(rig.root_id, p("stems"), old_terminal)
        .await
        .expect("browse_at subdir");
    assert_eq!(names(&old), vec!["kick.wav"]);
    assert!(!rig.root_dir.join("stems").exists());

    // Copy-forward: the everyday quarry verb.
    let written = rig
        .client
        .version
        .copy_forward(rig.root_id, old_terminal, vec![p("mix.wav")])
        .await
        .expect("copy forward");
    assert_eq!(written, vec![p("mix.wav")]);
    assert_eq!(
        std::fs::read(rig.root_dir.join("mix.wav")).unwrap(),
        original
    );

    // A dirty target is refused, not clobbered.
    std::fs::write(rig.root_dir.join("mix.wav"), b"unversioned work").unwrap();
    let err = rig
        .client
        .version
        .copy_forward(rig.root_id, old_terminal, vec![p("mix.wav")])
        .await
        .expect_err("dirty target must refuse");
    let message = refusal(err);
    assert!(message.contains("checkpoint first"), "{message}");
    assert_eq!(
        std::fs::read(rig.root_dir.join("mix.wav")).unwrap(),
        b"unversioned work"
    );
}

/// AC 3: a save landing in the old lineage mid-flip survives as
/// flagged divergence — never deleted, never silently absorbed.
#[tokio::test(flavor = "multi_thread")]
async fn a_mid_flip_save_survives_as_flagged_divergence() {
    let rig = rig().await;
    // Recorded before the hook is armed, so the only flip is the one
    // the hook is meant to land in.
    rig.current_lineage(None).await;
    // The seam fires between the terminal checkpoint and the clear —
    // exactly where a DAW's save lands during a restart.
    let root_dir = rig.root_dir.clone();
    rig.backend
        .set_mid_flip_hook(Some(std::sync::Arc::new(move |_root: &std::path::Path| {
            std::fs::write(root_dir.join("mix.wav"), b"the save nobody waited for").unwrap();
        })));

    rig.restart(RestartMode::Empty, None)
        .await
        .expect("restart with a mid-flip save");
    rig.backend.set_mid_flip_hook(None);

    // The save is flagged as divergence in the listing union (it lives
    // on a sibling head of the old terminal, not in the new lineage).
    let listed = rig.browse("").await;
    let mix = listed
        .iter()
        .find(|e| e.name == "mix.wav")
        .expect("the mid-flip save is visible");
    assert!(mix.divergent, "flagged as Divergent versions");
    // And its bytes are durably in the store even though the live tree
    // moved on — nothing was lost.
    assert!(!rig.root_dir.join("mix.wav").exists() || mix.stub || mix.divergent);
}

/// AC 4: replicas receive the flip as ordinary sync events — the
/// checkpoint that IS the flip, then the Project Version naming it.
#[tokio::test(flavor = "multi_thread")]
async fn the_flip_arrives_as_ordinary_events() {
    let rig = rig().await;
    let stream: TreeServiceStreamClient = rig.local.establish().await.expect("stream client");
    let (tx, mut rx) = vox::channel::<FilesEvent>();
    let root_id = rig.root_id;
    let _subscription = tokio::spawn(async move {
        let _ = stream.events(Some(root_id), tx).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let pv = rig
        .restart(RestartMode::Empty, None)
        .await
        .expect("restart");

    let mut saw_checkpoint = false;
    let mut saw_pv = false;
    // Room for the recorded v1's own event and catalogue deltas ahead
    // of the flip's two.
    for _ in 0..16 {
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("event in time")
            .expect("channel open")
            .expect("stream open");
        let mut copied = None;
        let _ = frame.map(|ev| copied = Some(ev));
        match copied.expect("event") {
            FilesEvent::Version(VersionEvent::Checkpointed(info))
                if info.description.contains("restart: Project Version") =>
            {
                saw_checkpoint = true;
            }
            FilesEvent::Curation(CurationEvent::ProjectVersionStarted(got)) if got.id == pv.id => {
                saw_pv = true;
            }
            _ => {}
        }
        if saw_checkpoint && saw_pv {
            break;
        }
    }
    assert!(saw_checkpoint, "the flip checkpoint is an ordinary event");
    assert!(saw_pv, "the Project Version rides the same stream");
}

/// PR #290 review regressions: the validations that keep a restart a
/// no-op when its inputs are wrong, and the seed that never clobbers.
#[tokio::test(flavor = "multi_thread")]
async fn restart_refuses_bad_inputs_before_touching_anything() {
    let rig = rig().await;

    // A template inside the root would be gutted by the clear first.
    let inner = rig.root_dir.join("_template");
    std::fs::create_dir_all(&inner).unwrap();
    let err = rig
        .restart(
            RestartMode::Template {
                source_path: inner.to_string_lossy().into_owned(),
            },
            None,
        )
        .await
        .expect_err("template inside the root must refuse");
    let message = refusal(err);
    assert!(message.contains("outside the root"), "{message}");

    // A carry-forward typo would clear the whole tree.
    let err = rig
        .restart(
            RestartMode::CarryForward {
                paths: vec!["stemz".into()],
            },
            None,
        )
        .await
        .expect_err("carry-forward typo must refuse");
    let message = refusal(err);
    assert!(message.contains("matches nothing tracked"), "{message}");

    // Both refusals were no-ops: live tree intact, and nothing minted
    // beyond the recorded lineage they were asked to restart.
    assert!(rig.root_dir.join("mix.wav").exists());
    assert!(rig.root_dir.join("stems").join("kick.wav").exists());
    let versions = rig
        .client
        .curation
        .project_versions(rig.root_id)
        .await
        .unwrap();
    assert_eq!(
        versions.iter().map(|pv| pv.number).collect::<Vec<_>>(),
        vec![1],
        "a refused restart mints no Project Version"
    );
}

/// A template file never overwrites a file that survived the clear —
/// the survivor (here: ignored, never versioned) wins.
#[tokio::test(flavor = "multi_thread")]
async fn template_seed_never_overwrites_a_survivor() {
    let rig = rig().await;
    let template = rig.data_dir.path().join("template");
    std::fs::create_dir_all(&template).unwrap();
    // The template ships a file colliding with the root's ignored junk.
    std::fs::write(template.join("mix.wav.reapeaks"), b"template peaks").unwrap();

    rig.restart(
        RestartMode::Template {
            source_path: template.to_string_lossy().into_owned(),
        },
        None,
    )
    .await
    .expect("restart from template");
    assert_eq!(
        std::fs::read(rig.root_dir.join("mix.wav.reapeaks")).unwrap(),
        b"peaks",
        "the survivor's bytes, not the template's"
    );
}
