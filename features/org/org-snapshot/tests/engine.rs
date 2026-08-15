//! Engine-level tests over a tempdir data root: layout + history
//! continuity, skip-if-clean, de-gitlink defense, local-remote push,
//! branch, and restore-checkout (incl. sqlite sidecar cleanup).
//!
//! Everything here shells out to the real `git` — these tests need it
//! on PATH (it is, everywhere this workspace builds).

use std::path::{Path, PathBuf};
use std::process::Command;

use org_snapshot::{RemoteConfig, SnapshotEngine};

/// Plain git runner for asserts (independent of the engine's own
/// plumbing).
fn git(args: &[&str]) -> String {
    let out = Command::new("git").args(args).output().expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn detached(git_dir: &Path, work_tree: &Path, args: &[&str]) -> String {
    let gd = git_dir.display().to_string();
    let wt = work_tree.display().to_string();
    let mut full = vec!["--git-dir", gd.as_str(), "--work-tree", wt.as_str()];
    full.extend_from_slice(args);
    git(&full)
}

/// Scaffold `<root>/orgs/alpha` with one vault file and return
/// (data_root, org_dir).
fn scaffold(root: &Path) -> PathBuf {
    let org = root.join("orgs/alpha");
    std::fs::create_dir_all(org.join("vault")).unwrap();
    std::fs::write(org.join("vault/note.md"), "state A\n").unwrap();
    std::fs::write(root.join("server-key.ed25519"), "not-a-real-key").unwrap();
    org
}

#[tokio::test]
async fn snapshot_commits_then_skips_when_clean() {
    let tmp = tempfile::tempdir().unwrap();
    let org = scaffold(tmp.path());
    let engine = SnapshotEngine::new(tmp.path());

    let report = engine.snapshot().await.unwrap();
    assert_eq!(report.repos.len(), 2, "org repo + full repo");
    assert_eq!(report.repos[0].repo, "task-org-alpha");
    assert_eq!(report.repos[1].repo, "task-data");
    for r in &report.repos {
        assert!(r.error.is_empty(), "{}: {}", r.repo, r.error);
        assert!(!r.clean, "{}: first cycle must commit", r.repo);
        assert!(!r.committed.is_empty());
        assert!(!r.pushed, "no remote configured");
    }

    // The full repo must NOT track .gitstate/ (exclude file).
    let full_git = tmp.path().join(".gitstate/full.git");
    let files = detached(&full_git, tmp.path(), &["ls-files"]);
    assert!(files.contains("orgs/alpha/vault/note.md"));
    assert!(files.contains("server-key.ed25519"));
    assert!(!files.contains(".gitstate"), "gitstate leaked: {files}");

    // Second cycle on an untouched tree: clean skip, no new commit.
    let report2 = engine.snapshot().await.unwrap();
    for r in &report2.repos {
        assert!(r.clean, "{}: second cycle must be clean", r.repo);
        assert!(r.committed.is_empty());
    }
    let count = detached(&full_git, tmp.path(), &["rev-list", "--count", "HEAD"]);
    assert_eq!(count, "1");

    // A change extends the SAME history — no reinit, parent linkage
    // intact (this is what keeps production lineages alive).
    std::fs::write(org.join("vault/note.md"), "state B\n").unwrap();
    let report3 = engine.snapshot().await.unwrap();
    assert!(!report3.repos[1].clean);
    let count = detached(&full_git, tmp.path(), &["rev-list", "--count", "HEAD"]);
    assert_eq!(count, "2", "history continued, not reinitialized");
}

#[tokio::test]
async fn de_gitlink_recovers_embedded_repo_contents() {
    let tmp = tempfile::tempdir().unwrap();
    let org = scaffold(tmp.path());
    let engine = SnapshotEngine::new(tmp.path());

    // Data migrated in with a stray embedded .git (with history —
    // `git add` refuses commit-less embedded repos outright) — it
    // gets recorded as a gitlink (mode 160000) on the first add and
    // its contents are invisible to the snapshot repo.
    let embedded = org.join("vault/imported");
    std::fs::create_dir_all(&embedded).unwrap();
    git(&["init", "-q", embedded.to_str().unwrap()]);
    std::fs::write(embedded.join("inner.md"), "imported content\n").unwrap();
    git(&["-C", embedded.to_str().unwrap(), "add", "."]);
    git(&[
        "-C",
        embedded.to_str().unwrap(),
        "-c",
        "user.name=t",
        "-c",
        "user.email=t@t",
        "commit",
        "-qm",
        "imported",
    ]);

    let first = engine.snapshot().await.unwrap();
    assert!(first.repos.iter().all(|r| r.error.is_empty()));
    let org_git = tmp.path().join(".gitstate/orgs/alpha.git");
    let listing = detached(&org_git, &org, &["ls-files", "-s"]);
    assert!(
        listing.lines().any(|l| l.starts_with("160000 ")),
        "embedded repo should be recorded as a gitlink first: {listing}"
    );

    // The stray .git goes away (or never belonged) — the next cycle
    // must un-record the gitlink so the files inside become visible.
    std::fs::remove_dir_all(embedded.join(".git")).unwrap();
    let report = engine.snapshot().await.unwrap();
    assert!(report.repos.iter().all(|r| r.error.is_empty()));
    let listing = detached(&org_git, &org, &["ls-files"]);
    assert!(
        listing.contains("vault/imported/inner.md"),
        "inner file must be tracked after de-gitlink: {listing}"
    );
    let content = detached(&org_git, &org, &["show", "HEAD:vault/imported/inner.md"]);
    assert_eq!(content, "imported content");
}

#[tokio::test]
async fn push_to_local_directory_remote() {
    let tmp = tempfile::tempdir().unwrap();
    let remote_base = tempfile::tempdir().unwrap();
    scaffold(tmp.path());
    let engine = SnapshotEngine::new(tmp.path()).with_remote(Some(RemoteConfig {
        base: remote_base.path().display().to_string(),
        user: String::new(),
        token: String::new(),
    }));

    let report = engine.snapshot().await.unwrap();
    for r in &report.repos {
        assert!(r.error.is_empty(), "{}: {}", r.repo, r.error);
        assert!(r.pushed, "{} should push to the dir remote", r.repo);
    }

    // The pushed bare repo holds the snapshot.
    let bare = remote_base.path().join("task-data.git");
    let note = git(&[
        "--git-dir",
        bare.to_str().unwrap(),
        "show",
        "main:orgs/alpha/vault/note.md",
    ]);
    assert_eq!(note, "state A");

    // Clean cycle still pushes (keeps remotes converged, like the
    // script).
    let report2 = engine.snapshot().await.unwrap();
    assert!(report2.repos.iter().all(|r| r.clean && r.pushed));
}

#[tokio::test]
async fn branch_log_and_restore_round_trip() {
    let tmp = tempfile::tempdir().unwrap();
    let org = scaffold(tmp.path());
    let engine = SnapshotEngine::new(tmp.path());

    // Snapshot A.
    let report_a = engine.snapshot().await.unwrap();
    let sha_a = report_a.repos[1].committed.clone();
    // Mutate + leave a stale sqlite WAL sidecar lying around.
    std::fs::write(org.join("vault/note.md"), "state B\n").unwrap();
    std::fs::write(org.join("auth.sqlite"), b"fake db bytes").unwrap();
    std::fs::write(org.join("auth.sqlite-wal"), b"stale wal").unwrap();
    std::fs::write(org.join("auth.sqlite-shm"), b"stale shm").unwrap();
    let report_b = engine.snapshot().await.unwrap();
    assert!(report_b.repos.iter().all(|r| r.error.is_empty()));

    // log: newest first, both commits, snapshot messages.
    let log = engine.log(10).await.unwrap();
    assert_eq!(log.len(), 2);
    assert_eq!(log[1].commit, sha_a);
    assert!(log[0].message.starts_with("snapshot "));

    // branch at HEAD.
    let branch = engine.branch("rescue/test").await.unwrap();
    assert_eq!(branch.commit, log[0].commit);
    assert!(!branch.pushed, "no remote configured");
    let full_git = tmp.path().join(".gitstate/full.git");
    let resolved = detached(&full_git, tmp.path(), &["rev-parse", "rescue/test"]);
    assert_eq!(resolved, branch.commit);

    // Dirty detection.
    assert!(!engine.full_tree_dirty().await.unwrap());
    std::fs::write(org.join("vault/extra.md"), "uncommitted\n").unwrap();
    assert!(engine.full_tree_dirty().await.unwrap());
    std::fs::remove_file(org.join("vault/extra.md")).unwrap();

    // Restore to A: file content reverts, sqlite sidecars are gone
    // (a leftover live WAL would shadow the restored db on reopen).
    let resolved = engine.restore_checkout(&sha_a).await.unwrap();
    assert_eq!(resolved, sha_a);
    let note = std::fs::read_to_string(org.join("vault/note.md")).unwrap();
    assert_eq!(note, "state A\n");
    assert!(
        !org.join("auth.sqlite-wal").exists(),
        "wal sidecar must be cleaned"
    );
    assert!(
        !org.join("auth.sqlite-shm").exists(),
        "shm sidecar must be cleaned"
    );

    // Unknown commit refuses cleanly.
    let err = engine.restore_checkout("deadbeef").await.unwrap_err();
    assert!(err.to_string().contains("unknown commit"));
}

/// The full-state repo MUST capture databases even when the org's
/// own `.gitignore` excludes `*.sqlite` (the "rebuildable from vault"
/// policy is correct for the per-org repo, fatal for the full
/// backup — auth.sqlite is not rebuildable). Volatile WAL/SHM
/// sidecars stay excluded.
#[tokio::test]
async fn full_repo_force_captures_gitignored_databases() {
    let tmp = tempfile::tempdir().unwrap();
    let org = scaffold(tmp.path());
    // The vault-centric ignore the live orgs ship.
    std::fs::write(
        org.join(".gitignore"),
        "*.sqlite\n*.sqlite-wal\n*.sqlite-shm\n",
    )
    .unwrap();
    std::fs::write(org.join("auth.sqlite"), b"db bytes").unwrap();
    std::fs::write(org.join("auth.sqlite-wal"), b"wal bytes").unwrap();

    let engine = SnapshotEngine::new(tmp.path());
    let report = engine.snapshot().await.unwrap();
    for r in &report.repos {
        assert!(r.error.is_empty(), "{}: {}", r.repo, r.error);
    }

    let full_git = tmp.path().join(".gitstate/full.git");
    let full_files = detached(&full_git, tmp.path(), &["ls-files"]);
    assert!(
        full_files.contains("orgs/alpha/auth.sqlite"),
        "full repo must capture the ignored db: {full_files}"
    );
    assert!(
        !full_files.contains("auth.sqlite-wal"),
        "volatile WAL sidecar must stay excluded: {full_files}"
    );

    // The per-org repo honors the ignore — db absent there.
    let org_git = tmp.path().join(".gitstate/orgs/alpha.git");
    let org_files = detached(&org_git, &org, &["ls-files"]);
    assert!(
        !org_files.contains("auth.sqlite"),
        "per-org repo must respect its .gitignore: {org_files}"
    );
}
