//! Software File Roots — colocated git (issue #273, ADR 0001) — driven
//! end to end over an in-process `architect::LocalServer`, the spec's
//! Testing Decisions primary seam ("the Files RPC surface over an
//! in-process memory link"), same harness as `rpc_surface.rs`.
//!
//! The four acceptance criteria, one test each:
//!
//! 1. a software root is a normal git repo — asserted with the real
//!    `git` binary (log, status, clone, push to a bare remote), because
//!    the criterion is about what *git tooling* sees, and only git can
//!    answer that honestly;
//! 2. the chain/history RPC behaves identically to a media root — the
//!    same assertions run against both flavors;
//! 3. flavor is chosen at creation and media stays the default;
//! 4. heavy stray files respect the Ignore set doctrine.

use std::path::Path;
use std::process::Command;

use architect::{LayerRouter, LocalServer, Scope};
use files::{FilesBackend, FilesServiceClient, RootFlavor, files_service_layer};

fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new().merge(files_service_layer(backend))
}

async fn client_for(data_dir: &Path) -> (FilesBackend, FilesServiceClient, std::sync::Arc<Scope>) {
    // The second argument is the org vault holding curated version
    // entities (issue #261) — nothing here touches curation, so it
    // points at a directory beside the roots rather than staging a
    // whole vault.
    let backend = FilesBackend::new(data_dir, data_dir.join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend.clone()), scope.clone());
    let client: FilesServiceClient = local.establish().await.expect("establish client");
    (backend, client, scope)
}

/// Run `git` in `dir`, asserting success, and return stdout.
fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(["-c", "user.email=test@example.com", "-c", "user.name=Test"])
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
    assert!(
        out.status.success(),
        "git {args:?} failed: {}\n{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn make_root_dir(data_dir: &Path, name: &str) -> std::path::PathBuf {
    let dir = data_dir.join(name);
    std::fs::create_dir(&dir).unwrap();
    dir
}

/// Acceptance 1: "A software root is a normal git repo (clone/push to a
/// remote works untouched)" — and the Files checkpoints are what git
/// sees, not a parallel history hidden from it.
#[tokio::test(flavor = "multi_thread")]
async fn software_root_is_a_normal_git_repo() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = make_root_dir(data_dir.path(), "synth-plugin");
    std::fs::write(root_dir.join("main.rs"), b"fn main() {}\n").unwrap();
    std::fs::create_dir(root_dir.join("src")).unwrap();
    std::fs::write(root_dir.join("src").join("dsp.rs"), b"// dsp\n").unwrap();

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Synth Plugin".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software)");
    assert_eq!(root.flavor, RootFlavor::Software);
    assert!(
        root_dir.join(".git").is_dir(),
        "a software root carries a real .git at its top level"
    );

    let checkpoint = client
        .checkpoint_now(root.id, Some("first checkpoint".to_string()))
        .await
        .expect("checkpoint_now");
    assert_eq!(
        checkpoint.changed_paths,
        vec!["main.rs".to_string(), "src/dsp.rs".to_string()],
    );

    // git sees the checkpoint as an ordinary commit on an ordinary
    // branch: reachable from HEAD, with the real tree in it.
    let log = git(&root_dir, &["log", "--oneline"]);
    assert!(
        log.contains("first checkpoint"),
        "checkpoint must be a reachable git commit; git log said: {log:?}"
    );
    assert_eq!(
        git(&root_dir, &["rev-parse", "HEAD"]).trim(),
        checkpoint.commit_id,
        "git HEAD is the commit the Files RPC reported"
    );
    let tracked = git(&root_dir, &["ls-tree", "-r", "--name-only", "HEAD"]);
    let tracked: Vec<&str> = tracked.lines().collect();
    assert_eq!(tracked, vec!["main.rs", "src/dsp.rs"]);

    // ...and the worktree is clean against it (the index is maintained,
    // so an IDE opening this folder sees a normal, unmodified repo).
    let status = git(&root_dir, &["status", "--porcelain"]);
    assert_eq!(
        status.trim(),
        "",
        "worktree should be clean after a checkpoint"
    );

    // Clone works untouched.
    let clone_dir = data_dir.path().join("clone");
    git(
        data_dir.path(),
        &[
            "clone",
            "--quiet",
            root_dir.to_str().unwrap(),
            clone_dir.to_str().unwrap(),
        ],
    );
    assert!(
        clone_dir.join("src").join("dsp.rs").exists(),
        "the clone carries the checkpointed tree"
    );

    // Push works untouched: a bare remote receives the branch.
    let remote_dir = data_dir.path().join("remote.git");
    git(
        data_dir.path(),
        &["init", "--quiet", "--bare", remote_dir.to_str().unwrap()],
    );
    git(
        &root_dir,
        &["remote", "add", "origin", remote_dir.to_str().unwrap()],
    );
    git(
        &root_dir,
        &["push", "--quiet", "origin", "HEAD:refs/heads/main"],
    );
    assert_eq!(
        git(&remote_dir, &["rev-parse", "refs/heads/main"]).trim(),
        checkpoint.commit_id,
        "the pushed branch carries the checkpoint commit"
    );

    backend.shutdown().await;
}

/// A folder that is *already* a git repository is adopted, not
/// re-initialized: its history stays, its remotes stay, and the first
/// Files checkpoint continues the branch it was on rather than forking a
/// second history behind git's back.
#[tokio::test(flavor = "multi_thread")]
async fn an_existing_git_repo_is_adopted_with_its_history() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = make_root_dir(data_dir.path(), "existing-repo");
    git(&root_dir, &["init", "--quiet", "-b", "trunk", "."]);
    std::fs::write(root_dir.join("lib.rs"), b"// v1\n").unwrap();
    git(&root_dir, &["add", "lib.rs"]);
    git(&root_dir, &["commit", "--quiet", "-m", "human commit"]);
    let human_commit = git(&root_dir, &["rev-parse", "HEAD"]).trim().to_string();

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Existing Repo".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software) adopting an existing repo");

    // The human's commit is already visible as a version of the file.
    let chain = client
        .chain(root.id, "lib.rs".to_string())
        .await
        .expect("chain over adopted history");
    assert_eq!(chain.len(), 1);
    assert_eq!(chain[0].commit_id, human_commit);

    // A checkpoint continues that branch rather than starting a new one.
    std::fs::write(root_dir.join("lib.rs"), b"// v2\n").unwrap();
    let cp = client
        .checkpoint_now(root.id, Some("files checkpoint".to_string()))
        .await
        .expect("checkpoint_now");
    assert_eq!(
        git(&root_dir, &["rev-parse", "trunk"]).trim(),
        cp.commit_id,
        "the checkpoint lands on the branch git had checked out"
    );
    assert_eq!(
        git(&root_dir, &["rev-parse", "HEAD~1"]).trim(),
        human_commit,
        "the human's commit is the checkpoint's parent"
    );
    let chain = client
        .chain(root.id, "lib.rs".to_string())
        .await
        .expect("chain after checkpoint");
    assert_eq!(chain.len(), 2, "both saved states are in the chain");
    assert_eq!(chain[0].commit_id, cp.commit_id);
    assert_eq!(chain[1].commit_id, human_commit);

    backend.shutdown().await;
}

/// Acceptance 2: "The Files chain/history RPC works on it identically to
/// media roots" — the same script, run against both flavors, must
/// produce the same shape of answer.
#[tokio::test(flavor = "multi_thread")]
async fn chain_and_checkpoints_behave_identically_on_both_flavors() {
    for flavor in [RootFlavor::Media, RootFlavor::Software] {
        let data_dir = tempfile::tempdir().expect("data tempdir");
        let root_dir = make_root_dir(data_dir.path(), "project");
        std::fs::write(root_dir.join("notes.txt"), b"take one").unwrap();

        let (backend, client, _scope) = client_for(data_dir.path()).await;
        let root = client
            .create_root(
                root_dir.to_str().unwrap().to_string(),
                "Project".to_string(),
                flavor,
            )
            .await
            .expect("create_root");

        let first = client
            .checkpoint_now(root.id, Some("v1".to_string()))
            .await
            .expect("checkpoint 1");
        std::fs::write(root_dir.join("notes.txt"), b"take two").unwrap();
        let second = client
            .checkpoint_now(root.id, Some("v2".to_string()))
            .await
            .expect("checkpoint 2");
        assert_eq!(second.changed_paths, vec!["notes.txt".to_string()]);

        // A checkpoint with nothing changed is a true no-op in the diff.
        let third = client
            .checkpoint_now(root.id, None)
            .await
            .expect("checkpoint 3");
        assert!(
            third.changed_paths.is_empty(),
            "{flavor:?}: unchanged tree must report no changed paths"
        );

        let chain = client
            .chain(root.id, "notes.txt".to_string())
            .await
            .expect("chain");
        assert_eq!(
            chain.len(),
            2,
            "{flavor:?}: two saved states, newest first — got {chain:?}"
        );
        assert_eq!(chain[0].commit_id, second.commit_id);
        assert_eq!(chain[1].commit_id, first.commit_id);
        assert_eq!(chain[0].path, "notes.txt");
        assert_ne!(
            chain[0].file_id, chain[1].file_id,
            "{flavor:?}: each saved state has its own content address"
        );

        // A file that has never been checkpointed has an empty chain.
        let missing = client
            .chain(root.id, "nope.txt".to_string())
            .await
            .expect("chain of an unknown path");
        assert!(missing.is_empty(), "{flavor:?}: unknown path has no chain");

        // Root browsing hides the root's own internals on both flavors
        // (`.fts-files`, the marker, and a software root's `.git`).
        let entries = client.browse(root.id, String::new()).await.expect("browse");
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["notes.txt"], "{flavor:?}: browse listing");

        // Identity and history survive a genuine restart of the backend
        // (a fresh process reopening the same tree).
        backend.shutdown().await;
        drop(backend);
        drop(client);
        let (backend, client, _scope) = client_for(data_dir.path()).await;
        let reopened = client
            .get_root(root.id)
            .await
            .expect("get_root after restart");
        assert_eq!(reopened.flavor, flavor, "flavor survives a restart");
        let chain = client
            .chain(root.id, "notes.txt".to_string())
            .await
            .expect("chain after restart");
        assert_eq!(chain.len(), 2, "{flavor:?}: history survives a restart");
        backend.shutdown().await;
    }
}

/// Acceptance 3: "Flavor is chosen at creation; media stays the default."
/// The wire type has no default, so what "media stays the default" means
/// operationally is that a media root is unchanged by this ticket: no
/// git repo appears in it, and nothing about its store moves.
#[tokio::test(flavor = "multi_thread")]
async fn flavor_is_chosen_at_creation_and_media_is_untouched() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let media_dir = make_root_dir(data_dir.path(), "mix-session");
    std::fs::write(media_dir.join("mix.wav"), b"take one").unwrap();
    let software_dir = make_root_dir(data_dir.path(), "plugin");
    std::fs::write(software_dir.join("lib.rs"), b"// code\n").unwrap();

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let media = client
        .create_root(
            media_dir.to_str().unwrap().to_string(),
            "Mix Session".to_string(),
            RootFlavor::Media,
        )
        .await
        .expect("create media root");
    let software = client
        .create_root(
            software_dir.to_str().unwrap().to_string(),
            "Plugin".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create software root");

    assert_eq!(media.flavor, RootFlavor::Media);
    assert!(
        !media_dir.join(".git").exists(),
        "a media root gets no git repo — its store is the CAS backend"
    );
    assert!(media_dir.join(".fts-files").is_dir());
    assert!(software_dir.join(".git").is_dir());
    assert!(software_dir.join(".fts-files").is_dir());

    // A media checkpoint still versions a heavy file that a software
    // root would ignore — the doctrine is per-flavor, not global.
    let cp = client
        .checkpoint_now(media.id, None)
        .await
        .expect("media checkpoint");
    assert_eq!(cp.changed_paths, vec!["mix.wav".to_string()]);

    // Both flavors are listed side by side, each remembering its own.
    let listed = client.list_roots().await.expect("list_roots");
    let flavors: Vec<_> = listed.iter().map(|r| (r.name.as_str(), r.flavor)).collect();
    assert!(flavors.contains(&("Mix Session", RootFlavor::Media)));
    assert!(flavors.contains(&("Plugin", RootFlavor::Software)));
    assert_eq!(
        client.get_root(software.id).await.unwrap().flavor,
        RootFlavor::Software
    );

    backend.shutdown().await;
}

/// Acceptance 4: "Heavy stray files respect the Ignore set doctrine" —
/// the flavor seed keeps stray media and build scaffolding out of a
/// software root's git object store, the tree's own `.gitignore` is
/// honored on top of it, and an ignore pattern never retroactively
/// deletes something already tracked.
#[tokio::test(flavor = "multi_thread")]
async fn heavy_stray_files_respect_the_ignore_set() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = make_root_dir(data_dir.path(), "sampler");
    std::fs::write(root_dir.join("lib.rs"), b"// code\n").unwrap();
    // A heavy stray render dropped in the source tree, and a build dir:
    // both are in the software flavor's seed.
    std::fs::write(root_dir.join("bounce.wav"), vec![0u8; 4096]).unwrap();
    std::fs::create_dir(root_dir.join("target")).unwrap();
    std::fs::write(root_dir.join("target").join("huge.bin"), vec![7u8; 8192]).unwrap();
    // The project's own .gitignore, honored on top of the seed.
    std::fs::write(root_dir.join(".gitignore"), b"secrets.env\n").unwrap();
    std::fs::write(root_dir.join("secrets.env"), b"TOKEN=hunter2\n").unwrap();

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Sampler".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software)");

    let cp = client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint_now");
    assert_eq!(
        cp.changed_paths,
        vec![".gitignore".to_string(), "lib.rs".to_string()],
        "ignored paths never enter the store"
    );
    let tracked = git(&root_dir, &["ls-tree", "-r", "--name-only", "HEAD"]);
    let tracked: Vec<&str> = tracked.lines().collect();
    assert_eq!(tracked, vec![".gitignore", "lib.rs"]);

    // An ignored file has no chain: it was never versioned.
    assert!(
        client
            .chain(root.id, "bounce.wav".to_string())
            .await
            .expect("chain of an ignored path")
            .is_empty()
    );

    // Ignoring something already tracked does NOT delete it from
    // history — git's own rule, and the only safe one for a versioning
    // system (see `files::ignore`).
    std::fs::write(root_dir.join(".gitignore"), b"secrets.env\nlib.rs\n").unwrap();
    let cp = client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint after ignoring a tracked file");
    assert_eq!(
        cp.changed_paths,
        vec![".gitignore".to_string()],
        "a newly-ignored but already-tracked file must not be recorded as deleted"
    );
    let tracked = git(&root_dir, &["ls-tree", "-r", "--name-only", "HEAD"]);
    let tracked: Vec<&str> = tracked.lines().collect();
    assert_eq!(tracked, vec![".gitignore", "lib.rs"]);

    backend.shutdown().await;
}

// ---------------------------------------------------------------------
// Regressions from the PR #282 review — each of these failed before the
// fix it names.
// ---------------------------------------------------------------------

/// Ignoring a *directory* must not delete the tracked files inside it.
/// The per-file "ignored but already tracked keeps being versioned"
/// exemption is worthless if the walk prunes the directory before its
/// files are ever enumerated: the removal pass then commits them as
/// deleted. Two shapes, both real: a project adding `docs/` to its
/// `.gitignore`, and adopting a repo that deliberately commits fixtures
/// under `target/` (which the software flavor's own seed ignores).
#[tokio::test(flavor = "multi_thread")]
async fn ignoring_a_directory_never_deletes_what_is_already_tracked() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = make_root_dir(data_dir.path(), "docs-project");
    std::fs::create_dir(root_dir.join("docs")).unwrap();
    std::fs::write(root_dir.join("docs").join("notes.txt"), b"notes\n").unwrap();
    // Committed-on-purpose fixtures under a seed-ignored directory.
    git(&root_dir, &["init", "--quiet", "-b", "main", "."]);
    std::fs::create_dir(root_dir.join("target")).unwrap();
    std::fs::write(root_dir.join("target").join("fixture.bin"), b"golden\n").unwrap();
    std::fs::write(root_dir.join("lib.rs"), b"// code\n").unwrap();
    git(&root_dir, &["add", "-A"]);
    git(&root_dir, &["commit", "--quiet", "-m", "seed"]);

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Docs Project".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software)");

    // The seed ignores `target/`, but those files are already tracked, so
    // the first checkpoint must leave them exactly where they are.
    let cp = client
        .checkpoint_now(root.id, None)
        .await
        .expect("first checkpoint");
    assert!(
        cp.changed_paths.is_empty(),
        "adopting a repo that tracks files under a seed-ignored directory must change nothing, \
         got {:?}",
        cp.changed_paths
    );
    let tracked = git(&root_dir, &["ls-tree", "-r", "--name-only", "HEAD"]);
    assert!(tracked.lines().any(|l| l == "target/fixture.bin"));
    assert!(tracked.lines().any(|l| l == "docs/notes.txt"));

    // Now the project ignores a directory full of tracked files.
    std::fs::write(root_dir.join(".gitignore"), b"docs/\n").unwrap();
    let cp = client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint after ignoring a tracked directory");
    assert_eq!(
        cp.changed_paths,
        vec![".gitignore".to_string()],
        "ignoring a directory must not record deletions of the tracked files under it"
    );
    let tracked = git(&root_dir, &["ls-tree", "-r", "--name-only", "HEAD"]);
    assert!(
        tracked.lines().any(|l| l == "docs/notes.txt"),
        "docs/notes.txt must still be tracked; tree was: {tracked:?}"
    );

    // A *new* file under the now-ignored directory still stays out.
    std::fs::write(root_dir.join("docs").join("draft.txt"), b"draft\n").unwrap();
    let cp = client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint with a new file under an ignored directory");
    assert!(
        cp.changed_paths.is_empty(),
        "a new file under an ignored directory must not be versioned, got {:?}",
        cp.changed_paths
    );

    backend.shutdown().await;
}

/// On an adopted repo with several branches, the checkpoint must report
/// the commit it actually wrote and move only the checked-out branch.
/// `view().heads()` holds every imported branch tip in unordered set
/// order, so deriving the result from it could report — and force the
/// checked-out branch onto — a different branch's head.
#[tokio::test(flavor = "multi_thread")]
async fn a_multi_branch_repo_moves_only_the_checked_out_branch() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = make_root_dir(data_dir.path(), "many-branches");
    git(&root_dir, &["init", "--quiet", "-b", "main", "."]);
    std::fs::write(root_dir.join("lib.rs"), b"// base\n").unwrap();
    git(&root_dir, &["add", "-A"]);
    git(&root_dir, &["commit", "--quiet", "-m", "base"]);

    // A handful of other branches, each with its own tip.
    for (i, branch) in ["alpha", "beta", "gamma"].iter().enumerate() {
        git(&root_dir, &["checkout", "--quiet", "-b", branch]);
        std::fs::write(root_dir.join("lib.rs"), format!("// {branch} {i}\n")).unwrap();
        git(&root_dir, &["add", "-A"]);
        git(&root_dir, &["commit", "--quiet", "-m", branch]);
    }
    git(&root_dir, &["checkout", "--quiet", "main"]);
    let main_before = git(&root_dir, &["rev-parse", "main"]).trim().to_string();
    let others: Vec<(String, String)> = ["alpha", "beta", "gamma"]
        .iter()
        .map(|b| {
            (
                (*b).to_string(),
                git(&root_dir, &["rev-parse", b]).trim().to_string(),
            )
        })
        .collect();

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Many Branches".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software)");

    std::fs::write(root_dir.join("lib.rs"), b"// files edit\n").unwrap();
    let cp = client
        .checkpoint_now(root.id, Some("files checkpoint".to_string()))
        .await
        .expect("checkpoint_now");

    assert_eq!(
        git(&root_dir, &["rev-parse", "main"]).trim(),
        cp.commit_id,
        "the checked-out branch is at the reported checkpoint"
    );
    assert_eq!(
        git(&root_dir, &["rev-parse", "HEAD~1"]).trim(),
        main_before,
        "the checkpoint's parent is main's own previous tip, not another branch's"
    );
    for (branch, before) in &others {
        assert_eq!(
            git(&root_dir, &["rev-parse", branch]).trim(),
            before,
            "{branch} must not have moved"
        );
    }
    assert_eq!(git(&root_dir, &["status", "--porcelain"]).trim(), "");

    backend.shutdown().await;
}

/// A human committing with plain `git` while the server is running must
/// be seen by the *same* backend instance — no restart. Otherwise the
/// per-process repo cache serves a stale view: chains miss their commit,
/// and the next checkpoint parents onto the stale head, forking history.
#[tokio::test(flavor = "multi_thread")]
async fn a_git_side_commit_mid_session_is_picked_up_without_a_restart() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = make_root_dir(data_dir.path(), "live-repo");
    std::fs::write(root_dir.join("lib.rs"), b"// v1\n").unwrap();

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Live Repo".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software)");
    let first = client
        .checkpoint_now(root.id, Some("files v1".to_string()))
        .await
        .expect("checkpoint 1");

    // A human commits in the checkout, with this backend still live.
    std::fs::write(root_dir.join("lib.rs"), b"// v2 by hand\n").unwrap();
    git(&root_dir, &["add", "-A"]);
    git(&root_dir, &["commit", "--quiet", "-m", "by hand"]);
    let human = git(&root_dir, &["rev-parse", "HEAD"]).trim().to_string();

    // The same client, no restart: the chain shows their commit.
    let chain = client
        .chain(root.id, "lib.rs".to_string())
        .await
        .expect("chain");
    assert_eq!(
        chain.first().map(|e| e.commit_id.as_str()),
        Some(human.as_str()),
        "a git-side commit must be visible without restarting the server; chain was {chain:?}"
    );

    // ...and the next checkpoint builds on it rather than forking.
    std::fs::write(root_dir.join("lib.rs"), b"// v3 by files\n").unwrap();
    let third = client
        .checkpoint_now(root.id, Some("files v3".to_string()))
        .await
        .expect("checkpoint 3");
    assert_eq!(
        git(&root_dir, &["rev-parse", "HEAD~1"]).trim(),
        human,
        "the checkpoint's parent is the human's commit"
    );
    assert_eq!(
        git(&root_dir, &["rev-parse", "HEAD"]).trim(),
        third.commit_id
    );
    assert_eq!(git(&root_dir, &["status", "--porcelain"]).trim(), "");
    assert_ne!(third.commit_id, first.commit_id);

    backend.shutdown().await;
}

/// A repo checked out at a detached HEAD (a tag, a CI checkout) must keep
/// its detached checkout: the checkpoint commits on top of that commit and
/// moves HEAD itself, touching no branch — exactly what `git commit` does
/// there. Substituting a `main` fallback would move an unrelated branch
/// and yank the user off their checkout.
#[tokio::test(flavor = "multi_thread")]
async fn a_detached_head_is_preserved_and_no_branch_is_clobbered() {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = make_root_dir(data_dir.path(), "detached-repo");
    git(&root_dir, &["init", "--quiet", "-b", "main", "."]);
    std::fs::write(root_dir.join("lib.rs"), b"// v1\n").unwrap();
    git(&root_dir, &["add", "-A"]);
    git(&root_dir, &["commit", "--quiet", "-m", "v1"]);
    let v1 = git(&root_dir, &["rev-parse", "HEAD"]).trim().to_string();
    std::fs::write(root_dir.join("lib.rs"), b"// v2\n").unwrap();
    git(&root_dir, &["add", "-A"]);
    git(&root_dir, &["commit", "--quiet", "-m", "v2"]);
    let main_tip = git(&root_dir, &["rev-parse", "main"]).trim().to_string();
    // Check out an older commit, detached — the CI / release-tag shape.
    git(&root_dir, &["checkout", "--quiet", "--detach", &v1]);

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Detached Repo".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software) on a detached checkout");

    std::fs::write(root_dir.join("lib.rs"), b"// v1 edited\n").unwrap();
    let cp = client
        .checkpoint_now(root.id, Some("on a detached head".to_string()))
        .await
        .expect("checkpoint_now on a detached HEAD");

    assert_eq!(
        git(&root_dir, &["rev-parse", "HEAD"]).trim(),
        cp.commit_id,
        "HEAD moved to the checkpoint"
    );
    assert_eq!(
        git(&root_dir, &["rev-parse", "HEAD~1"]).trim(),
        v1,
        "the checkpoint parented on the detached commit, not on a branch"
    );
    assert_eq!(
        git(&root_dir, &["rev-parse", "main"]).trim(),
        main_tip,
        "main must not have moved"
    );
    let symbolic = Command::new("git")
        .args(["symbolic-ref", "-q", "HEAD"])
        .current_dir(&root_dir)
        .output()
        .expect("git symbolic-ref");
    assert!(
        !symbolic.status.success(),
        "HEAD must still be detached, but it points at {}",
        String::from_utf8_lossy(&symbolic.stdout)
    );
    assert_eq!(git(&root_dir, &["status", "--porcelain"]).trim(), "");

    backend.shutdown().await;
}

/// File modes are real git metadata: a checkpoint must record 100755 for
/// an executable file and preserve it across edits, or a clone ships a
/// script that won't run — and `git status` shows a mode diff immediately
/// after a supposedly clean checkpoint.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_executable_bit_is_recorded_and_preserved() {
    use std::os::unix::fs::PermissionsExt as _;

    fn mode_of(dir: &Path, path: &str) -> String {
        let out = git(dir, &["ls-files", "--stage", path]);
        out.split_whitespace()
            .next()
            .unwrap_or_default()
            .to_string()
    }

    let data_dir = tempfile::tempdir().expect("data tempdir");
    let root_dir = make_root_dir(data_dir.path(), "scripts");
    let script = root_dir.join("build.sh");
    std::fs::write(&script, b"#!/bin/sh\necho hi\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(root_dir.join("readme.md"), b"# hi\n").unwrap();

    let (backend, client, _scope) = client_for(data_dir.path()).await;
    let root = client
        .create_root(
            root_dir.to_str().unwrap().to_string(),
            "Scripts".to_string(),
            RootFlavor::Software,
        )
        .await
        .expect("create_root(Software)");
    client
        .checkpoint_now(root.id, None)
        .await
        .expect("first checkpoint");

    assert_eq!(mode_of(&root_dir, "build.sh"), "100755");
    assert_eq!(mode_of(&root_dir, "readme.md"), "100644");
    assert_eq!(
        git(&root_dir, &["status", "--porcelain"]).trim(),
        "",
        "a clean worktree means git agrees about modes too"
    );

    // Editing the script keeps it executable.
    std::fs::write(&script, b"#!/bin/sh\necho bye\n").unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    let cp = client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint after editing the script");
    assert_eq!(cp.changed_paths, vec!["build.sh".to_string()]);
    assert_eq!(mode_of(&root_dir, "build.sh"), "100755");
    assert_eq!(git(&root_dir, &["status", "--porcelain"]).trim(), "");

    // Flipping the bit alone is itself a change worth recording.
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cp = client
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint after chmod -x");
    assert_eq!(
        cp.changed_paths,
        vec!["build.sh".to_string()],
        "a mode-only change is a change"
    );
    assert_eq!(mode_of(&root_dir, "build.sh"), "100644");

    backend.shutdown().await;
}
