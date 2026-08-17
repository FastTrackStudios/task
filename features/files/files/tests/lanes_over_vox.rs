//! The v2 lanes over an in-process `architect::LocalServer`.
//!
//! Every other lane test calls the trait directly, which proves the
//! implementation and proves nothing about the mount. This file is the
//! other half: a real client, a real dispatcher, real serialisation of
//! the typed ids and paths, and the descriptor/permit pairing that
//! decides whether a method is reachable at all.
//!
//! It exists because "implemented" and "reachable" are different claims,
//! and until the lanes were mounted only the first was true.

use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files::FilesBackend;
use files_proto::model::RootFlavor;
use files_proto::service::roots::AdoptRequest;
use files_proto::service::write::OnConflict;
use files_proto::{
    RootId, RootPath, RootsServiceClient, TreeServiceClient, VersionServiceClient,
    WriteServiceClient,
};

/// The lanes this file exercises, mounted exactly as `org_layer_router`
/// mounts them.
fn router(backend: FilesBackend) -> LayerRouter {
    LayerRouter::new()
        .merge(files_proto::roots_layer(backend.clone()))
        .merge(files_proto::tree_layer(backend.clone()))
        .merge(files_proto::write_layer(backend.clone()))
        .merge(files_proto::version_layer(backend))
}

struct Rig {
    _tmp: tempfile::TempDir,
    _local: LocalServer,
    roots: RootsServiceClient,
    tree: TreeServiceClient,
    write: WriteServiceClient,
    version: VersionServiceClient,
}

async fn rig() -> (Rig, String) {
    let tmp = tempfile::tempdir().expect("data tempdir");
    let dir = tmp.path().join("mix-session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();

    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(router(backend), scope);

    let rig = Rig {
        roots: local.establish().await.expect("RootsServiceClient"),
        tree: local.establish().await.expect("TreeServiceClient"),
        write: local.establish().await.expect("WriteServiceClient"),
        version: local.establish().await.expect("VersionServiceClient"),
        _local: local,
        _tmp: tmp,
    };
    (rig, dir.to_string_lossy().into_owned())
}

async fn adopted(rig: &Rig, path: String) -> RootId {
    let root = tokio::time::timeout(
        Duration::from_secs(10),
        rig.roots.adopt(AdoptRequest {
            path,
            name: "Mix Session".into(),
            flavor: RootFlavor::Media,
            hash_content: true,
        }),
    )
    .await
    .expect("adopt timed out")
    .expect("adopt over vox");
    RootId::new(root.id)
}

/// The whole point: a root adopted over the wire is browsable over the
/// wire, through a different lane.
#[tokio::test(flavor = "multi_thread")]
async fn a_root_adopted_on_one_lane_is_browsable_on_another() {
    let (rig, path) = rig().await;
    let root = adopted(&rig, path).await;

    let listed = rig.roots.list().await.expect("list over vox");
    assert!(listed.iter().any(|r| r.id == root.get()));

    let entries = rig
        .tree
        .browse(root, RootPath::root())
        .await
        .expect("browse over vox");
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert!(names.contains(&"mix.wav"), "got {names:?}");
    assert!(names.contains(&"stems"), "got {names:?}");
}

/// Typed ids and typed faults survive serialisation. This is the claim
/// the whole v2 surface rests on, and in-process tests cannot make it:
/// `RootNotFound(id)` has to arrive carrying the id, not flattened to a
/// string somewhere in the dispatcher.
#[tokio::test(flavor = "multi_thread")]
async fn a_typed_fault_survives_the_wire() {
    let (rig, path) = rig().await;
    adopted(&rig, path).await;
    let ghost = RootId::generate();

    let err = rig.roots.get(ghost).await.expect_err("no such root");
    // The transport wraps the application error rather than flattening
    // it — `VoxError::User` carries the domain fault intact, which is the
    // property the typed error surface depends on.
    match err {
        vox::VoxError::User(fault) => match *fault {
            files_proto::FilesFault::RootNotFound(id) => {
                assert_eq!(id, ghost, "the id must survive the round trip");
            }
            other => panic!("expected RootNotFound over the wire, got {other:?}"),
        },
        other => panic!("expected a user error, got {other:?}"),
    }
}

/// A path that escapes is refused over the wire too.
///
/// `RootPath` is transparent on the wire, so `Deserialize` builds one
/// without calling `parse` — which is exactly why every lane re-validates
/// what arrives. This proves the re-validation is real rather than a
/// comment.
#[tokio::test(flavor = "multi_thread")]
async fn confinement_holds_against_a_deserialised_path() {
    let (rig, path) = rig().await;
    let root = adopted(&rig, path).await;

    // Serialise a legitimate path, then tamper with the wire form the way
    // a hostile peer would.
    let json = facet_json::to_string(&RootPath::parse("stems").unwrap()).unwrap();
    assert_eq!(json, "\"stems\"", "the wire form is the string, not a tuple");
    let escaping: RootPath = facet_json::from_str("\"../../etc\"").expect("deserialises");
    assert_eq!(
        escaping.as_str(),
        "../../etc",
        "the newtype does not validate on deserialise — hence the re-check"
    );

    assert!(
        rig.tree.browse(root, escaping).await.is_err(),
        "a path that escapes must be refused even though it deserialised"
    );
}

/// A write over the wire lands, and the tree lane sees it — the two
/// halves of `note_write` and the mount, together.
#[tokio::test(flavor = "multi_thread")]
async fn a_write_over_the_wire_reaches_the_catalogue() {
    let (rig, path) = rig().await;
    let root = adopted(&rig, path).await;

    // Make the catalogue resident, then write through a different lane.
    rig.tree.catalogue(root, None).await.expect("catalogue");

    let made = RootPath::parse("Renders").unwrap();
    let receipt = rig
        .write
        .create_dirs(root, vec![made.clone()])
        .await
        .expect("create_dirs over vox");
    assert!(
        !receipt.operation.is_empty(),
        "a write records one version-store operation"
    );

    let entry = rig.tree.entry(root, made).await.expect("entry over vox");
    assert_eq!(entry.path.as_str(), "Renders");
}

/// A move is transactional across the wire, and refuses to move a
/// directory into itself.
#[tokio::test(flavor = "multi_thread")]
async fn a_move_into_itself_is_refused_over_the_wire() {
    let (rig, path) = rig().await;
    let root = adopted(&rig, path).await;

    let err = rig
        .write
        .move_paths(
            root,
            vec![files_proto::service::write::Relocation {
                from: RootPath::parse("stems").unwrap(),
                to: RootPath::parse("stems/inner").unwrap(),
            }],
            OnConflict::Fail,
        )
        .await
        .expect_err("a directory cannot be moved inside itself");
    assert!(matches!(
        err,
        vox::VoxError::User(ref f) if matches!(**f, files_proto::FilesFault::IntoSelf { .. })
    ));
}

/// Checkpoint on one lane, chain on the same lane, over the wire.
#[tokio::test(flavor = "multi_thread")]
async fn history_is_reachable_over_the_wire() {
    let (rig, path) = rig().await;
    let root = adopted(&rig, path).await;

    rig.version
        .checkpoint(root, Some("first".into()))
        .await
        .expect("checkpoint over vox");

    let chain = rig
        .version
        .chain(root, RootPath::parse("mix.wav").unwrap())
        .await
        .expect("chain over vox");
    assert!(!chain.is_empty(), "a checkpointed file has a chain");
}

/// Real bytes, over vox, through the byte lane.
///
/// This is the transport claim the whole design rests on: bytes ride vox
/// like every other call, so a native client needs no HTTP at all. The
/// signed-URL rendition route is a fallback for cold guest links, not the
/// mechanism.
#[tokio::test(flavor = "multi_thread")]
async fn bytes_cross_vox_and_arrive_intact() {
    use files_proto::MediaServiceStreamClient;
    use files_proto::service::media::{ByteFrame, ByteRequest, MediaService};

    let tmp = tempfile::tempdir().expect("data tempdir");
    let dir = tmp.path().join("mix-session");
    std::fs::create_dir(&dir).unwrap();
    // Big enough to span several frames, so this exercises the streaming
    // path rather than a single-shot send.
    let payload: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(dir.join("mix.wav"), &payload).unwrap();

    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(
        LayerRouter::new()
            .merge(files_proto::roots_layer(backend.clone()))
            .merge(files_proto::version_layer(backend.clone()))
            .merge(files_proto::media_layer(backend.clone()))
            .merge(files_proto::media_stream_layer(backend.clone())),
        scope,
    );

    let roots: RootsServiceClient = local.establish().await.expect("roots client");
    let root = RootId::new(
        roots
            .adopt(AdoptRequest {
                path: dir.to_string_lossy().into_owned(),
                name: "Mix Session".into(),
                flavor: RootFlavor::Media,
                hash_content: true,
            })
            .await
            .expect("adopt")
            .id,
    );

    // A ticket names an object in the store, so the content has to be
    // checkpointed before it can be read.
    let version: VersionServiceClient = local.establish().await.expect("version client");
    version
        .checkpoint(root, Some("first".into()))
        .await
        .expect("checkpoint");

    let ticket = backend
        .read(root, RootPath::parse("mix.wav").unwrap())
        .await
        .expect("mint a ticket");
    assert_eq!(ticket.length, Some(payload.len() as u64));

    let stream: MediaServiceStreamClient = local.establish().await.expect("byte lane client");
    let (tx, mut rx) = vox::channel::<ByteFrame>();
    let request = ByteRequest {
        token: ticket.token.clone(),
        range: None,
    };
    tokio::spawn(async move {
        let _ = stream.bytes(request, tx).await;
    });

    let mut got = Vec::new();
    let mut opened = None;
    let mut done = false;
    while let Ok(Ok(Some(frame))) = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
        let mut copied = None;
        let _ = frame.map(|f| copied = Some(f));
        match copied.expect("frame") {
            ByteFrame::Opened { length, total, .. } => opened = Some((length, total)),
            ByteFrame::Chunk { offset, bytes } => {
                assert_eq!(offset as usize, got.len(), "frames arrive in order");
                got.extend_from_slice(&bytes);
            }
            ByteFrame::Done => {
                done = true;
                break;
            }
            ByteFrame::Failed(f) => panic!("byte lane failed: {f:?}"),
        }
    }

    assert_eq!(
        opened,
        Some((payload.len() as u64, payload.len() as u64)),
        "the stream announces its length before any bytes"
    );
    assert!(
        done,
        "the stream says it finished rather than just stopping"
    );
    assert_eq!(got, payload, "every byte arrived, in order, unaltered");
}

/// An upload of content the server does not already hold, end to end.
///
/// This is what the byte lane's egress half could not do. `complete`
/// refused for want of bytes, because nothing in the codebase could
/// deliver them — a subscription streams *out*, and an upload needs the
/// other direction.
///
/// vox binds a channel handle in either position: a `Tx` in a
/// subscription's args means the server sends, an `Rx` in a method's args
/// means the server receives. The capability was there and unused.
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_sends_its_bytes_over_vox_and_lands() {
    use files_proto::UploadServiceClient;
    use files_proto::service::upload::{UploadFrame, UploadSpec};

    let tmp = tempfile::tempdir().expect("data tempdir");
    let dir = tmp.path().join("mix-session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();

    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(
        LayerRouter::new()
            .merge(files_proto::roots_layer(backend.clone()))
            .merge(files_proto::tree_layer(backend.clone()))
            .merge(files_proto::upload_layer(backend.clone())),
        scope,
    );

    let roots: RootsServiceClient = local.establish().await.expect("roots client");
    let root = RootId::new(
        roots
            .adopt(AdoptRequest {
                path: dir.to_string_lossy().into_owned(),
                name: "Mix Session".into(),
                flavor: RootFlavor::Media,
                hash_content: true,
            })
            .await
            .expect("adopt")
            .id,
    );

    // Content the server has never seen — so no dedup shortcut, and the
    // bytes genuinely have to cross the wire.
    let payload: Vec<u8> = (0..200_000u32).map(|i| (i % 253) as u8).collect();
    let dest = RootPath::parse("stems/new-take.wav").unwrap();

    let uploads: UploadServiceClient = local.establish().await.expect("upload client");
    let plan = uploads
        .begin(UploadSpec {
            root_id: root,
            path: dest.clone(),
            size: payload.len() as u64,
            content: None,
            modified_at: None,
        })
        .await
        .expect("begin");
    assert!(
        !plan.needed.is_empty(),
        "content the store has never seen must be asked for"
    );

    // The client keeps the sender and hands over the receiver; vox binds
    // it, and the server reads with the channel's credit pacing us.
    let (tx, rx) = vox::channel::<UploadFrame>();
    let sending = tokio::spawn(async move {
        for (i, chunk) in payload.chunks(64 * 1024).enumerate() {
            let offset = (i * 64 * 1024) as u64;
            if tx
                .send(UploadFrame::Chunk {
                    offset,
                    bytes: chunk.to_vec(),
                })
                .await
                .is_err()
            {
                return;
            }
        }
        let _ = tx.send(UploadFrame::Finished).await;
    });

    let received = tokio::time::timeout(
        Duration::from_secs(30),
        uploads.send_bytes(plan.upload_id, rx),
    )
    .await
    .expect("send_bytes timed out")
    .expect("send_bytes");
    sending.await.expect("sender finished");

    assert_eq!(received.written, 200_000, "every byte was written");
    assert!(
        received.needed.is_empty(),
        "after sending, nothing is outstanding: {:?}",
        received.needed
    );

    let entry = uploads
        .complete(plan.upload_id, OnConflict::Fail)
        .await
        .expect("complete must not refuse once the bytes are held");
    assert_eq!(entry.path, dest);

    // And it is really on disk, byte for byte.
    let landed = std::fs::read(dir.join("stems").join("new-take.wav")).expect("landed");
    assert_eq!(landed.len(), 200_000);
    assert_eq!(
        landed[..64],
        (0..64u32).map(|i| (i % 253) as u8).collect::<Vec<_>>()[..]
    );
}

/// An archive of a selection, generated as it is sent, over vox.
///
/// The last stub. It refused for a good reason — a `ByteTicket` nothing
/// could redeem is worse than an honest refusal — and the reason is gone
/// now that the byte lane can carry a generated stream.
#[tokio::test(flavor = "multi_thread")]
async fn an_archive_streams_as_a_tar_over_vox() {
    use files_proto::MediaServiceStreamClient;
    use files_proto::service::media::{ByteFrame, ByteRequest};
    use files_proto::service::write::WriteService;

    let tmp = tempfile::tempdir().expect("data tempdir");
    let dir = tmp.path().join("mix-session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("mix.wav"), b"take one").unwrap();
    std::fs::create_dir(dir.join("stems")).unwrap();
    std::fs::write(dir.join("stems").join("kick.wav"), b"boom").unwrap();

    let backend = FilesBackend::new(tmp.path(), tmp.path().join("vault")).expect("backend");
    let scope = Scope::new();
    let local = LocalServer::serve(
        LayerRouter::new()
            .merge(files_proto::roots_layer(backend.clone()))
            .merge(files_proto::media_stream_layer(backend.clone())),
        scope,
    );

    let roots: RootsServiceClient = local.establish().await.expect("roots client");
    let root = RootId::new(
        roots
            .adopt(AdoptRequest {
                path: dir.to_string_lossy().into_owned(),
                name: "Mix Session".into(),
                flavor: RootFlavor::Media,
                hash_content: true,
            })
            .await
            .expect("adopt")
            .id,
    );

    let ticket = backend
        .archive(root, vec![RootPath::parse("stems").unwrap()])
        .await
        .expect("archive must no longer refuse");
    assert_eq!(
        ticket.length, None,
        "a generated stream has no known length"
    );
    assert!(!ticket.seekable, "and cannot be ranged");
    assert_eq!(ticket.content_type, "application/x-tar");

    let stream: MediaServiceStreamClient = local.establish().await.expect("byte lane");
    let (tx, mut rx) = vox::channel::<ByteFrame>();
    let request = ByteRequest {
        token: ticket.token.clone(),
        range: None,
    };
    tokio::spawn(async move {
        let _ = stream.bytes(request, tx).await;
    });

    let mut tar = Vec::new();
    let mut done = false;
    while let Ok(Ok(Some(frame))) = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await {
        let mut copied = None;
        let _ = frame.map(|f| copied = Some(f));
        match copied.expect("frame") {
            ByteFrame::Chunk { bytes, .. } => tar.extend_from_slice(&bytes),
            ByteFrame::Done => {
                done = true;
                break;
            }
            ByteFrame::Failed(f) => panic!("archive failed: {f:?}"),
            ByteFrame::Opened { .. } => {}
        }
    }
    assert!(done, "the archive says it finished");

    // Read it back the way an extractor would, so this asserts a real
    // tar rather than merely some bytes.
    assert_eq!(tar.len() % 512, 0, "block-aligned");
    assert!(
        tar[tar.len() - 1024..].iter().all(|b| *b == 0),
        "two zero blocks end it"
    );
    let name = |block: &[u8]| {
        String::from_utf8_lossy(&block[..100])
            .trim_end_matches('\0')
            .to_string()
    };
    let mut names = Vec::new();
    let mut at = 0;
    while at + 512 <= tar.len() {
        let block = &tar[at..at + 512];
        if block.iter().all(|b| *b == 0) {
            break;
        }
        assert_eq!(&block[257..263], b"ustar\0", "every header is USTAR");
        let n = name(block);
        let size = u64::from_str_radix(
            String::from_utf8_lossy(&block[124..135]).trim_end_matches(['\0', ' ']),
            8,
        )
        .unwrap_or(0);
        if n == "stems/kick.wav" {
            let body = &tar[at + 512..at + 512 + size as usize];
            assert_eq!(body, b"boom", "the file's bytes are in the archive");
        }
        names.push(n);
        at += 512 + size as usize + files_padding(size);
    }
    assert!(
        names.iter().any(|n| n == "stems/"),
        "the directory is announced before its contents: {names:?}"
    );
    assert!(names.iter().any(|n| n == "stems/kick.wav"), "{names:?}");
    assert!(
        !names.iter().any(|n| n == "mix.wav"),
        "only what was selected: {names:?}"
    );
}

/// Tar pads each entry's body to a 512-byte boundary.
fn files_padding(size: u64) -> usize {
    let rem = (size % 512) as usize;
    if rem == 0 { 0 } else { 512 - rem }
}

/// A timestamp survives the wire with the precision it was created with.
///
/// `DateTime<Utc>` is an opaque scalar to facet: its only encode path
/// is one `display` fn, so whatever that fn writes is what every facet
/// format writes, vox included. Upstream rc.5 hardcoded
/// `SecondsFormat::Secs` there and rounded every timestamp on every RPC
/// in this repo to the second — see the vendored `facet-core` patch.
///
/// This is the regression test for that, and it belongs at the wire
/// rather than next to the fix: `files.catalogue.staleness` answers
/// "how fresh is this" from these values, and two events in the same
/// second cannot be ordered by a field that rounds them together.
#[tokio::test(flavor = "multi_thread")]
async fn a_timestamp_crosses_the_wire_without_losing_precision() {
    let (rig, path) = rig().await;
    let root = adopted(&rig, path).await;

    let over_the_wire = rig.roots.get(root).await.expect("get").created_at;
    assert_ne!(
        over_the_wire.timestamp_subsec_nanos(),
        0,
        "the wire rounded a timestamp to the second"
    );
}
