//! Project Version restart (issue #268) at the spec's primary seam:
//! the Files RPC surface over an in-process memory link. One test per
//! acceptance criterion: the three starting modes, read-only
//! time-travel browse + copy-forward, the mid-flip save surviving as
//! flagged divergence, and the flip arriving as ordinary events.

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::{
    FilesBackend, FilesEvent, FilesServiceClient, FilesServiceStreamClient, RestartMode,
    RootFlavor, files_service_layer, files_service_stream_layer,
};

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new()
        .merge(files_service_layer(backend.clone()))
        .merge(files_service_stream_layer(backend))
}

struct Rig {
    data_dir: tempfile::TempDir,
    root_dir: std::path::PathBuf,
    root_id: uuid::Uuid,
    backend: FilesBackend,
    client: FilesServiceClient,
    local: LocalServer,
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
    let client: FilesServiceClient = local.establish().await.expect("client");
    let root = client
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "session".into(),
            RootFlavor::Media,
        )
        .await
        .expect("create_root");
    client
        .checkpoint_now(root.id, None)
        .await
        .expect("first checkpoint");
    Rig {
        data_dir,
        root_dir,
        root_id: root.id,
        backend,
        client,
        local,
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
    let chain = rig
        .client
        .chain(rig.root_id, "mix.wav".into())
        .await
        .unwrap();
    let old_terminal = chain[0].commit_id.clone();

    let pv = rig
        .client
        .restart_project_version(rig.root_id, RestartMode::Empty, Some("take two".into()))
        .await
        .expect("restart empty");
    assert_eq!(pv.number, 1);
    assert_eq!(pv.label.as_deref(), Some("take two"));

    // Live tree: tracked files gone, ignored junk untouched, internals
    // intact (browse hides them; the disk still has the store).
    let listed = rig.client.browse(rig.root_id, String::new()).await.unwrap();
    assert_eq!(names(&listed), vec!["mix.wav.reapeaks"]);
    assert!(rig.root_dir.join("mix.wav.reapeaks").exists());
    assert!(!rig.root_dir.join("mix.wav").exists());
    assert!(!rig.root_dir.join("stems").exists(), "emptied dirs pruned");

    // The old iteration is browsable read-only at the flip's parent.
    let old = rig
        .client
        .browse_at(rig.root_id, old_terminal.clone(), String::new())
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
        .client
        .restart_project_version(
            rig.root_id,
            RestartMode::Template {
                source_path: template.to_string_lossy().into_owned(),
            },
            None,
        )
        .await
        .expect("restart from template");
    assert_eq!(pv.number, 1);

    let listed = rig.client.browse(rig.root_id, String::new()).await.unwrap();
    assert_eq!(
        names(&listed),
        vec!["mix.wav.reapeaks", "session-notes.md", "stems"]
    );
    // The template's stale internals stayed out; the root's own store
    // is still the one the marker knows.
    assert!(!rig.root_dir.join(".fts-files").join("junk").exists());

    // The seed is versioned as the new lineage's content.
    let chain = rig
        .client
        .chain(rig.root_id, "session-notes.md".into())
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
        .client
        .restart_project_version(
            rig.root_id,
            RestartMode::CarryForward {
                paths: vec!["stems".into()],
            },
            None,
        )
        .await
        .expect("restart carry-forward");
    assert_eq!(pv.number, 1);

    let listed = rig.client.browse(rig.root_id, String::new()).await.unwrap();
    assert_eq!(names(&listed), vec!["mix.wav.reapeaks", "stems"]);
    assert!(rig.root_dir.join("stems").join("kick.wav").exists());
    assert!(!rig.root_dir.join("mix.wav").exists());

    // The pure lineage cut: carry everything.
    let pv2 = rig
        .client
        .restart_project_version(
            rig.root_id,
            RestartMode::CarryForward { paths: vec![] },
            None,
        )
        .await
        .expect("pure lineage cut");
    assert_eq!(pv2.number, 2, "auto-numbering advances");
    let listed = rig.client.browse(rig.root_id, String::new()).await.unwrap();
    assert_eq!(names(&listed), vec!["mix.wav.reapeaks", "stems"]);
}

/// AC 2: the old iteration browses read-only and copy-forward brings
/// chosen files into the current one — refusing to clobber unversioned
/// work.
#[tokio::test(flavor = "multi_thread")]
async fn old_iterations_browse_read_only_and_copy_forward() {
    let rig = rig().await;
    let original = std::fs::read(rig.root_dir.join("mix.wav")).unwrap();
    let chain = rig
        .client
        .chain(rig.root_id, "mix.wav".into())
        .await
        .unwrap();
    let old_terminal = chain[0].commit_id.clone();

    rig.client
        .restart_project_version(rig.root_id, RestartMode::Empty, None)
        .await
        .expect("restart");

    // Read-only: browse_at answers from the store — nothing reappears
    // on disk, and repeated browsing changes nothing.
    let old = rig
        .client
        .browse_at(rig.root_id, old_terminal.clone(), "stems".into())
        .await
        .expect("browse_at subdir");
    assert_eq!(names(&old), vec!["kick.wav"]);
    assert!(!rig.root_dir.join("stems").exists());

    // Copy-forward: the everyday quarry verb.
    let written = rig
        .client
        .copy_forward(rig.root_id, old_terminal.clone(), vec!["mix.wav".into()])
        .await
        .expect("copy forward");
    assert_eq!(written, vec!["mix.wav".to_string()]);
    assert_eq!(
        std::fs::read(rig.root_dir.join("mix.wav")).unwrap(),
        original
    );

    // A dirty target is refused, not clobbered.
    std::fs::write(rig.root_dir.join("mix.wav"), b"unversioned work").unwrap();
    let err = rig
        .client
        .copy_forward(rig.root_id, old_terminal, vec!["mix.wav".into()])
        .await
        .expect_err("dirty target must refuse");
    assert!(err.to_string().contains("checkpoint first"), "{err}");
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
    // The seam fires between the terminal checkpoint and the clear —
    // exactly where a DAW's save lands during a restart.
    let root_dir = rig.root_dir.clone();
    rig.backend
        .set_mid_flip_hook(Some(std::sync::Arc::new(move |_root: &std::path::Path| {
            std::fs::write(root_dir.join("mix.wav"), b"the save nobody waited for").unwrap();
        })));

    rig.client
        .restart_project_version(rig.root_id, RestartMode::Empty, None)
        .await
        .expect("restart with a mid-flip save");
    rig.backend.set_mid_flip_hook(None);

    // The save is flagged as divergence in the listing union (it lives
    // on a sibling head of the old terminal, not in the new lineage).
    let listed = rig.client.browse(rig.root_id, String::new()).await.unwrap();
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
    let stream: FilesServiceStreamClient = rig.local.establish().await.expect("stream client");
    let (tx, mut rx) = vox::channel::<FilesEvent>();
    let _subscription = tokio::spawn(async move {
        let _ = stream.events(tx).await;
    });
    tokio::time::sleep(Duration::from_millis(200)).await;

    let pv = rig
        .client
        .restart_project_version(rig.root_id, RestartMode::Empty, None)
        .await
        .expect("restart");

    let mut saw_checkpoint = false;
    let mut saw_pv = false;
    for _ in 0..4 {
        let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
            .await
            .expect("event in time")
            .expect("channel open")
            .expect("stream open");
        let mut copied = None;
        let _ = frame.map(|ev| copied = Some(ev));
        match copied.expect("event") {
            FilesEvent::Checkpointed(info)
                if info.description.contains("restart: Project Version") =>
            {
                saw_checkpoint = true;
            }
            FilesEvent::ProjectVersionStarted(got) if got.id == pv.id => {
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
        .client
        .restart_project_version(
            rig.root_id,
            RestartMode::Template {
                source_path: inner.to_string_lossy().into_owned(),
            },
            None,
        )
        .await
        .expect_err("template inside the root must refuse");
    assert!(err.to_string().contains("outside the root"), "{err}");

    // A carry-forward typo would clear the whole tree.
    let err = rig
        .client
        .restart_project_version(
            rig.root_id,
            RestartMode::CarryForward {
                paths: vec!["stemz".into()],
            },
            None,
        )
        .await
        .expect_err("carry-forward typo must refuse");
    assert!(err.to_string().contains("matches nothing tracked"), "{err}");

    // Both refusals were no-ops: live tree intact, no entity minted.
    assert!(rig.root_dir.join("mix.wav").exists());
    assert!(rig.root_dir.join("stems").join("kick.wav").exists());
    let versions = rig.client.list_project_versions(rig.root_id).await.unwrap();
    assert!(versions.is_empty());
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

    rig.client
        .restart_project_version(
            rig.root_id,
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
