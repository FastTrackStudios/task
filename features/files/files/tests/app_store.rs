//! The Files lanes as an app's store — over vox, the way Keyflow,
//! Session and Signal reach them.
//!
//! What an app needs and the rest of the suite does not ask for: a root
//! it can make without a server path, bytes that land in any root (not
//! only a media one), a save that cannot silently overwrite another
//! machine's, and a live stream that tells it when something changed.

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::FilesBackend;
use files_proto::error::FilesFault;
use files_proto::model::RootFlavor;
use files_proto::service::FilesEvent;
use files_proto::service::roots::CreateRequest;
use files_proto::service::tree::CatalogueEntry;
use files_proto::service::upload::{Expect, UploadFrame, UploadSpec};
use files_proto::service::write::OnConflict;
use files_proto::{
    RootId, RootPath, RootsServiceClient, TreeServiceStreamClient, UploadServiceClient,
};

struct Rig {
    tmp: tempfile::TempDir,
    _local: LocalServer,
    roots: RootsServiceClient,
    uploads: UploadServiceClient,
    stream: TreeServiceStreamClient,
}

async fn rig() -> Rig {
    let tmp = tempfile::tempdir().expect("data tempdir");
    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let local = LocalServer::serve(
        LayerRouter::new()
            .merge(files_proto::roots_layer(backend.clone()))
            .merge(files_proto::tree_layer(backend.clone()))
            .merge(files_proto::tree_stream_layer(backend.clone()))
            .merge(files_proto::upload_layer(backend)),
        Scope::new(),
    );
    Rig {
        roots: local.establish().await.expect("roots client"),
        uploads: local.establish().await.expect("upload client"),
        stream: local.establish().await.expect("tree stream client"),
        _local: local,
        tmp,
    }
}

async fn create(rig: &Rig, dir: &str, flavor: RootFlavor) -> RootId {
    let root = rig
        .roots
        .create(CreateRequest {
            dir: dir.into(),
            name: dir.into(),
            flavor,
        })
        .await
        .expect("create");
    RootId::new(root.id)
}

fn spec(root: RootId, path: &str, size: usize, expect: Option<Expect>) -> UploadSpec {
    UploadSpec {
        root_id: root,
        path: RootPath::parse(path).unwrap(),
        size: size as u64,
        content: None,
        modified_at: None,
        expect,
    }
}

/// Send `bytes[from..to]` for an open upload.
async fn send(rig: &Rig, upload: files_proto::UploadId, bytes: &[u8], from: usize, to: usize) {
    let (tx, rx) = vox::channel::<UploadFrame>();
    let chunk = bytes[from..to].to_vec();
    let sending = tokio::spawn(async move {
        for (i, piece) in chunk.chunks(16 * 1024).enumerate() {
            let offset = (from + i * 16 * 1024) as u64;
            if tx
                .send(UploadFrame::Chunk {
                    offset,
                    bytes: piece.to_vec(),
                })
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = tx.send(UploadFrame::Finished).await;
    });
    tokio::time::timeout(Duration::from_secs(30), rig.uploads.send_bytes(upload, rx))
        .await
        .expect("send_bytes timed out")
        .expect("send_bytes");
    sending.await.expect("sender");
}

/// One whole save: begin, send everything, complete.
async fn save(
    rig: &Rig,
    root: RootId,
    path: &str,
    bytes: &[u8],
    on_conflict: OnConflict,
    expect: Option<Expect>,
) -> Result<CatalogueEntry, FilesFault> {
    let plan = rig
        .uploads
        .begin(spec(root, path, bytes.len(), expect))
        .await
        .expect("begin");
    if !plan.needed.is_empty() {
        send(rig, plan.upload_id, bytes, 0, bytes.len()).await;
    }
    rig.uploads
        .complete(plan.upload_id, on_conflict)
        .await
        .map_err(|e| match e {
            vox::VoxError::User(f) => *f,
            other => panic!("transport: {other:?}"),
        })
}

// t[verify files.adopt.create]
#[tokio::test(flavor = "multi_thread")]
async fn an_app_makes_its_store_once_and_finds_it_again() {
    let rig = rig().await;
    let first = create(&rig, "signal/impulse-responses", RootFlavor::Media).await;
    assert!(
        rig.tmp.path().join("signal/impulse-responses").is_dir(),
        "the directory is made where the org keeps its files"
    );
    let again = create(&rig, "signal/impulse-responses", RootFlavor::Media).await;
    assert_eq!(first, again, "asking again returns the same root");

    for reserved in ["uploads", "stores/x", ".hidden"] {
        let refused = rig
            .roots
            .create(CreateRequest {
                dir: reserved.into(),
                name: reserved.into(),
                flavor: RootFlavor::Media,
            })
            .await;
        assert!(refused.is_err(), "`{reserved}` is the backend's own");
    }

    std::fs::create_dir_all(rig.tmp.path().join("somebody-elses")).unwrap();
    std::fs::write(rig.tmp.path().join("somebody-elses/file"), b"x").unwrap();
    match rig
        .roots
        .create(CreateRequest {
            dir: "somebody-elses".into(),
            name: "mine".into(),
            flavor: RootFlavor::Media,
        })
        .await
    {
        Err(vox::VoxError::User(f)) if matches!(*f, FilesFault::Exists { .. }) => {}
        other => panic!("creating over an existing folder is not adopting it: {other:?}"),
    }
}

// t[verify files.write.upload] — bytes land in any root, not only media
#[tokio::test(flavor = "multi_thread")]
async fn a_software_root_takes_an_upload() {
    let rig = rig().await;
    let root = create(&rig, "keyflow/charts", RootFlavor::Software).await;
    let chart = b"{ \"title\": \"Hosanna\", \"key\": \"E\" }".repeat(500);

    let entry = save(&rig, root, "hosanna.kf", &chart, OnConflict::Fail, None)
        .await
        .expect("a software root lands an upload");
    assert_eq!(
        std::fs::read(rig.tmp.path().join("keyflow/charts/hosanna.kf")).unwrap(),
        chart
    );
    assert!(entry.content.is_some(), "the landing reports its etag");
}

// t[verify files.write.upload] — holes never become content
#[tokio::test(flavor = "multi_thread")]
async fn a_partial_send_cannot_land_and_resumes_where_it_stopped() {
    let rig = rig().await;
    let root = create(&rig, "session/takes", RootFlavor::Media).await;
    let take: Vec<u8> = (0..300_000u32).map(|i| (i % 241) as u8).collect();

    let plan = rig
        .uploads
        .begin(spec(root, "vox.wav", take.len(), None))
        .await
        .expect("begin");
    send(&rig, plan.upload_id, &take, 0, 100_000).await;

    let progress = rig
        .uploads
        .progress(plan.upload_id)
        .await
        .expect("progress");
    assert_eq!(progress.received, 100_000);
    assert_eq!(progress.needed.len(), 1);
    assert_eq!(progress.needed[0].start, 100_000);
    assert!(
        rig.uploads
            .complete(plan.upload_id, OnConflict::Fail)
            .await
            .is_err(),
        "a file with holes does not land"
    );
    assert!(!rig.tmp.path().join("session/takes/vox.wav").exists());

    send(&rig, plan.upload_id, &take, 100_000, take.len()).await;
    rig.uploads
        .complete(plan.upload_id, OnConflict::Fail)
        .await
        .expect("whole now");
    assert_eq!(
        std::fs::read(rig.tmp.path().join("session/takes/vox.wav")).unwrap(),
        take
    );
}

// t[verify files.write.safe-save]
#[tokio::test(flavor = "multi_thread")]
async fn a_save_against_a_stale_copy_is_refused_not_applied() {
    let rig = rig().await;
    let root = create(&rig, "session/songs", RootFlavor::Media).await;
    let path = "opening-night.rpp";

    let v1 = save(
        &rig,
        root,
        path,
        b"<REAPER v1>",
        OnConflict::Fail,
        Some(Expect::Absent),
    )
    .await
    .expect("create-only save onto nothing");
    let etag1 = v1.content.expect("etag");

    // The laptop saves on top of what it read.
    let v2 = save(
        &rig,
        root,
        path,
        b"<REAPER v2 laptop>",
        OnConflict::Replace,
        Some(Expect::Content(etag1.clone())),
    )
    .await
    .expect("a save against the current copy lands");
    assert_ne!(v2.content, Some(etag1.clone()));

    // The studio machine still holds v1 and saves. It is told, not obeyed.
    match save(
        &rig,
        root,
        path,
        b"<REAPER v2 studio>",
        OnConflict::Replace,
        Some(Expect::Content(etag1)),
    )
    .await
    {
        Err(FilesFault::Stale { .. }) => {}
        other => panic!("expected Stale, got {other:?}"),
    }
    assert_eq!(
        std::fs::read(rig.tmp.path().join("session/songs").join(path)).unwrap(),
        b"<REAPER v2 laptop>",
        "the laptop's save survived"
    );

    // And create-only means create-only, whatever the conflict policy.
    match save(
        &rig,
        root,
        path,
        b"again",
        OnConflict::Replace,
        Some(Expect::Absent),
    )
    .await
    {
        Err(FilesFault::Stale { .. }) => {}
        other => panic!("expected Stale for a create-only save, got {other:?}"),
    }
}

// t[verify files.live.propagation] — the v2 stream
#[tokio::test(flavor = "multi_thread")]
async fn the_live_stream_tells_an_app_what_landed() {
    let rig = rig().await;
    let root = create(&rig, "keyflow/library", RootFlavor::Media).await;

    let (tx, mut rx) = vox::channel::<FilesEvent>();
    let stream = rig.stream.clone();
    tokio::spawn(async move {
        let _ = stream.events(Some(root), tx).await;
    });
    // Let the subscription attach before the write it should hear.
    tokio::time::sleep(Duration::from_millis(200)).await;

    save(&rig, root, "set.json", b"[]", OnConflict::Fail, None)
        .await
        .expect("save");

    let mut heard = false;
    while let Ok(Ok(Some(frame))) = tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
        let mut event = None;
        let _ = frame.map(|e| event = Some(e));
        if let Some(FilesEvent::Upload(files_proto::service::upload::UploadEvent::Completed(e))) =
            event
        {
            assert_eq!(e.path.as_str(), "set.json");
            heard = true;
            break;
        }
    }
    assert!(heard, "the upload reached the subscriber");
}
