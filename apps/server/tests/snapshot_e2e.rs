#![allow(clippy::large_futures)]
//! Server-native snapshot e2e: boots a real `AppState` on a temp
//! data root, hammers sqlite writes (threads creates) *while* a
//! snapshot cycle runs, then proves the committed sqlite is
//! consistent (`PRAGMA integrity_check` on a copy extracted from
//! the git history). Also covers the restore round-trip
//! (create → snapshot A → mutate → restore A → verify) and the
//! branch verb, all through the `SnapshotService` impl the server
//! mounts at `/server/vox`.
//!
//! Single test fn: env-var driven boot (`TASK_DATA_ROOT` +
//! `TASK_BACKUP_GIT_TOKEN`) must not race a second boot in the
//! same process.

use std::process::Command;

use org_proto::SnapshotService as _;
use sea_orm::{ConnectionTrait, Statement};
use task_server::AppState;
use task_server::snapshot::SnapshotImpl;
use threads::service::{CreateThreadRequest, ThreadsService as _};

const BACKUP_TOKEN: &str = "e2e-backup-token";

fn git_show(git_dir: &std::path::Path, spec: &str) -> Vec<u8> {
    let out = Command::new("git")
        .arg("--git-dir")
        .arg(git_dir)
        .arg("show")
        .arg(spec)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git show {spec}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_under_concurrent_writes_restore_and_branch() {
    let tmp = tempfile::tempdir().unwrap();

    // SAFETY: single boot in this test binary; nothing else reads
    // these vars concurrently.
    unsafe {
        std::env::set_var("TASK_DATA_ROOT", tmp.path());
        std::env::set_var("TASK_BACKUP_GIT_TOKEN", BACKUP_TOKEN);
        // Never inherit a real backup remote (or stray path
        // overrides) from the ambient environment.
        for var in [
            "TASK_BACKUP_REMOTE_BASE",
            "TASK_BACKUP_GIT_USER",
            "TASK_SERVER_ORG",
            "TASK_SERVER_VAULT_ROOT",
            "TASK_SERVER_CRDT_ROOT",
            "TASK_SERVER_WIKI_ROOT",
            "TASK_SERVER_MAIL_ROOT",
            "TASK_SERVER_AGENT_TASKS_URL",
            "TASK_SERVER_TIMER_URL",
            "TASK_SERVER_THREADS_URL",
            "TASK_SERVER_FINANCE_URL",
        ] {
            std::env::remove_var(var);
        }
    }

    let data_root = org_proto::DataRoot::from_env().unwrap();
    data_root.ensure().unwrap();
    let org_root = data_root.init_org("alpha", "Alpha", true).unwrap();
    let vault_note = org_root.vault_dir().join("note.md");
    std::fs::create_dir_all(org_root.vault_dir()).unwrap();
    std::fs::write(&vault_note, "state A\n").unwrap();

    let state = AppState::new(None).await.expect("boot AppState");
    let org = state.org("alpha").expect("alpha hosted");
    assert!(
        org.sqlite_conns.len() >= 5,
        "auth/agent-tasks/timer/threads/finance pools collected"
    );
    let snap = SnapshotImpl::new_without_exit(state.clone());

    // ── snapshot A (vault at "state A") ──────────────────────────
    let report_a = snap.snapshot(BACKUP_TOKEN.into()).await.unwrap();
    assert_eq!(report_a.repos.len(), 2);
    for r in &report_a.repos {
        assert!(r.error.is_empty(), "{}: {}", r.repo, r.error);
        assert!(!r.clean);
        assert!(!r.pushed, "no remote configured in this test");
    }
    let sha_a = snap.log(BACKUP_TOKEN.into(), 1).await.unwrap()[0]
        .commit
        .clone();

    // ── snapshot under concurrent sqlite writes ──────────────────
    // Writers go straight at the store (bypassing the vox entry
    // gate) — the worst case for the committed db file. WAL mode
    // keeps the main file consistent; checkpoint + commit must
    // produce a db that passes integrity_check.
    let mut writers = tokio::task::JoinSet::new();
    for w in 0..4 {
        let threads = org.threads.clone();
        writers.spawn(async move {
            for i in 0..40 {
                threads
                    .create_thread(CreateThreadRequest {
                        org_id: uuid::Uuid::new_v4(),
                        entity_type: "task".into(),
                        entity_id: uuid::Uuid::new_v4(),
                        title: format!("hammer {w}-{i}"),
                        kind: String::new(),
                        created_by: uuid::Uuid::new_v4(),
                        source_kind: "native".into(),
                        source_ref: None,
                        source_url: None,
                    })
                    .await
                    .expect("create_thread during snapshot");
            }
        });
    }
    let report_b = snap.snapshot(BACKUP_TOKEN.into()).await.unwrap();
    for r in &report_b.repos {
        assert!(r.error.is_empty(), "{}: {}", r.repo, r.error);
    }
    while let Some(res) = writers.join_next().await {
        res.expect("writer task");
    }

    // Extract the committed threads.sqlite from the org repo's
    // history and integrity-check that copy.
    let org_git = tmp.path().join(".gitstate/orgs/alpha.git");
    let db_bytes = git_show(&org_git, "HEAD:threads.sqlite");
    assert!(!db_bytes.is_empty());
    let extracted = tmp.path().join("extracted-threads.sqlite");
    std::fs::write(&extracted, &db_bytes).unwrap();
    let check = sea_orm::Database::connect(format!("sqlite://{}?mode=ro", extracted.display()))
        .await
        .expect("open extracted sqlite");
    let row = check
        .query_one(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "PRAGMA integrity_check;",
        ))
        .await
        .expect("run integrity_check")
        .expect("integrity_check row");
    let verdict: String = row.try_get_by_index(0).unwrap();
    assert_eq!(verdict, "ok", "committed sqlite must be consistent");
    // And it actually contains hammered rows (the snapshot ran
    // mid-hammer, so somewhere between 0 and 160 — just prove the
    // table is queryable).
    let row = check
        .query_one(Statement::from_string(
            sea_orm::DatabaseBackend::Sqlite,
            "SELECT COUNT(*) FROM threads_threads;",
        ))
        .await
        .unwrap()
        .unwrap();
    let _count: i64 = row.try_get_by_index(0).unwrap();
    check.close().await.unwrap();

    // ── auth guard ────────────────────────────────────────────────
    let err = snap.snapshot("wrong-token".into()).await.unwrap_err();
    assert!(matches!(err, org_proto::SnapshotError::Unauthorized(_)));

    // ── restore round-trip: mutate → restore A → verify ──────────
    std::fs::write(&vault_note, "state B\n").unwrap();
    let report_c = snap.snapshot(BACKUP_TOKEN.into()).await.unwrap();
    assert!(report_c.repos.iter().any(|r| !r.clean), "B must commit");

    // confirm must match the commit.
    let err = snap
        .restore(BACKUP_TOKEN.into(), sha_a.clone(), "nope".into(), false)
        .await
        .unwrap_err();
    assert!(matches!(err, org_proto::SnapshotError::Refused(_)));

    let restored = snap
        .restore(BACKUP_TOKEN.into(), sha_a.clone(), sha_a.clone(), false)
        .await
        .unwrap();
    assert_eq!(restored.commit, sha_a);
    assert!(!restored.restarting, "test impl keeps the process alive");
    assert!(
        !restored.pre_restore.is_empty(),
        "default restore takes a rescue snapshot first"
    );
    let note = std::fs::read_to_string(&vault_note).unwrap();
    assert_eq!(note, "state A\n", "vault reverted to snapshot A");
    // The rescue snapshot means "state B" is still reachable as a
    // commit — roll-forward stays possible.
    let log = snap.log(BACKUP_TOKEN.into(), 10).await.unwrap();
    assert!(log.len() >= 3);

    // ── branch verb ───────────────────────────────────────────────
    let branch = snap
        .branch(BACKUP_TOKEN.into(), "rescue/e2e".into())
        .await
        .unwrap();
    assert!(!branch.commit.is_empty());
    let full_git = tmp.path().join(".gitstate/full.git");
    let resolved = Command::new("git")
        .arg("--git-dir")
        .arg(&full_git)
        .args(["rev-parse", "rescue/e2e"])
        .output()
        .unwrap();
    assert!(resolved.status.success());
    assert_eq!(
        String::from_utf8_lossy(&resolved.stdout).trim(),
        branch.commit
    );

    state.scope.close().await;
}
