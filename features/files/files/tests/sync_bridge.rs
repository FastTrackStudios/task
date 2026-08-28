//! `FilesBackend::sync_ingest_path` called repeatedly from inside one
//! runtime. Reported (docs/spec/unmet.md) to stall on the third call.

use files::{FilesBackend, FilesService, RootFlavor};

async fn rig() -> (tempfile::TempDir, FilesBackend, uuid::Uuid) {
    let data = tempfile::tempdir().expect("data tempdir");
    let backend = FilesBackend::new(data.path(), data.path().join("vault")).expect("backend");
    let root_dir = data.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    let root = backend
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "Session".into(),
            RootFlavor::Media,
        )
        .await
        .expect("create root");
    (data, backend, root.id)
}

async fn hammer() {
    let (data, backend, root) = rig().await;
    let mut files = Vec::new();
    for i in 0..4 {
        let p = data.path().join(format!("shot{i}.jpg"));
        std::fs::write(&p, format!("still {i}")).unwrap();
        files.push(p);
    }
    for p in &files {
        backend.sync_ingest_path(root, p).expect("ingest");
    }
    for p in &files {
        backend.sync_ingest_path(root, p).expect("ingest again");
    }
}

#[tokio::test]
async fn repeated_calls_on_a_current_thread_runtime_complete() {
    hammer().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_calls_on_a_multi_thread_runtime_complete() {
    hammer().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_calls_from_spawn_blocking_complete() {
    tokio::task::spawn_blocking(|| pollster::block_on(hammer()))
        .await
        .unwrap();
}
