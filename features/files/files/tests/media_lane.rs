//! `MediaService` and the byte lane, against a real `FilesBackend`.
//!
//! In-process rather than over `LocalServer`, for the same reason
//! `roots_lane.rs` is: the lane has no `permits.rs` row yet, so it is not
//! mounted on a router. These call the trait directly, which is what the
//! dispatcher would do — and the redemption half is an inherent method
//! an HTTP route calls, so it is exercised the same way the route will.

use std::sync::Arc;
use std::time::Duration;

use files::{FilesBackend, FilesService, RootFlavor};
use files_proto::id::{ContentId, RootId, VersionId};
use files_proto::model::RenditionKind;
use files_proto::service::media::{HandoffItem, HandoffTarget, MediaService, Region};
use files_proto::{FilesFault, RootPath};

/// Bytes long enough that a range is a genuine window rather than the
/// whole thing, and patterned so a wrong offset is visible.
fn take() -> Vec<u8> {
    (0..4096u32).map(|n| (n % 251) as u8).collect()
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

/// A media root holding `mix.wav`, checkpointed so its content is in the
/// store — the byte lane serves the checkpoint head, never the live file.
async fn rig() -> (tempfile::TempDir, FilesBackend, RootId, Vec<u8>) {
    let data = tempfile::tempdir().expect("data tempdir");
    let backend = FilesBackend::new(data.path(), data.path().join("vault")).expect("backend");
    let root_dir = data.path().join("session");
    std::fs::create_dir(&root_dir).unwrap();
    let bytes = take();
    std::fs::write(root_dir.join("mix.wav"), &bytes).unwrap();

    let root = backend
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "Session".into(),
            RootFlavor::Media,
        )
        .await
        .expect("create root");
    backend
        .checkpoint_now(root.id, None)
        .await
        .expect("checkpoint");
    (data, backend, RootId::new(root.id), bytes)
}

async fn redeem(
    backend: &FilesBackend,
    token: &str,
    range: Option<(u64, u64)>,
) -> Result<Vec<u8>, FilesFault> {
    let mut out: Vec<u8> = Vec::new();
    backend.redeem_bytes(token, range, &mut out).await?;
    Ok(out)
}

/// The whole point of the lane: a ticket is redeemable, and what comes
/// back is the file.
// t[verify files.scale.large-media]
#[tokio::test(flavor = "multi_thread")]
async fn a_redeemed_ticket_streams_the_real_bytes() {
    let (_data, backend, root_id, bytes) = rig().await;

    let ticket = backend.read(root_id, p("mix.wav")).await.expect("read");
    assert_eq!(
        ticket.length,
        Some(bytes.len() as u64),
        "a stored object's length is known"
    );
    assert!(ticket.seekable, "both stores read by range");
    assert_eq!(ticket.content_type, "audio/wav");
    assert!(ticket.expires_at > chrono::Utc::now());

    let got = redeem(&backend, &ticket.token, None).await.expect("redeem");
    assert_eq!(got, bytes, "the bytes must be the file's, not a summary");

    // Not single-use: a `<video>` scrubbing issues many requests
    // against one grant.
    assert_eq!(
        redeem(&backend, &ticket.token, None).await.expect("again"),
        bytes
    );
}

/// Seeking the middle fetches the middle — the requirement's own
/// example, minus the gigabytes.
// t[verify files.scale.large-media]
#[tokio::test(flavor = "multi_thread")]
async fn a_range_reads_exactly_that_window() {
    let (_data, backend, root_id, bytes) = rig().await;
    let ticket = backend.read(root_id, p("mix.wav")).await.expect("read");

    let got = redeem(&backend, &ticket.token, Some((1000, 1099)))
        .await
        .expect("ranged redeem");
    assert_eq!(got.len(), 100, "an inclusive HTTP range is last - first + 1");
    assert_eq!(got, bytes[1000..1100], "and it is the right window");

    // The last byte is addressable; one past it is not.
    let last = bytes.len() as u64 - 1;
    assert_eq!(
        redeem(&backend, &ticket.token, Some((last, last)))
            .await
            .expect("final byte")
            .len(),
        1
    );
    assert!(
        matches!(
            redeem(&backend, &ticket.token, Some((last, last + 1))).await,
            Err(FilesFault::Invalid(_))
        ),
        "a token must not be walked past what it was minted for"
    );
    assert!(matches!(
        redeem(&backend, &ticket.token, Some((10, 5))).await,
        Err(FilesFault::Invalid(_))
    ));
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_token_is_refused() {
    let (_data, backend, _root_id, _bytes) = rig().await;
    assert!(matches!(
        backend.byte_ticket("not-a-token"),
        Err(FilesFault::Invalid(_))
    ));
    assert!(matches!(
        redeem(&backend, "not-a-token", None).await,
        Err(FilesFault::Invalid(_))
    ));
}

/// The ticket a route reads its headers from is the ticket that was
/// minted.
#[tokio::test(flavor = "multi_thread")]
async fn a_token_resolves_back_to_its_ticket() {
    let (_data, backend, root_id, bytes) = rig().await;
    let minted = backend.read(root_id, p("mix.wav")).await.expect("read");
    let looked_up = backend.byte_ticket(&minted.token).expect("inspect");
    assert_eq!(looked_up, minted);
    assert_eq!(looked_up.length, Some(bytes.len() as u64));
}

/// A ticket book shared across orgs would make one org's token redeem
/// against another's content. `durable::Scoped` keys by the backend's
/// data dir, which is the org boundary.
#[tokio::test(flavor = "multi_thread")]
async fn one_orgs_ticket_is_not_anothers() {
    let (_a_data, a, a_root, _bytes) = rig().await;
    let (_b_data, b, _b_root, _) = rig().await;

    let ticket = a.read(a_root, p("mix.wav")).await.expect("read");
    assert!(
        matches!(b.byte_ticket(&ticket.token), Err(FilesFault::Invalid(_))),
        "a process-wide ticket store would have made these one org"
    );
    assert!(matches!(
        redeem(&b, &ticket.token, None).await,
        Err(FilesFault::Invalid(_))
    ));
}

/// A past version's bytes are the past version's, whatever the file
/// looks like now.
// t[verify files.scale.large-media]
#[tokio::test(flavor = "multi_thread")]
async fn read_at_serves_the_version_asked_for() {
    let (_data, backend, root_id, first) = rig().await;
    let chain = backend
        .chain(root_id.get(), "mix.wav".into())
        .await
        .expect("chain");
    let original = VersionId::from_commit_hex(&chain[0].commit_id);

    // Overwrite and checkpoint again.
    let root = backend.get_root(root_id.get()).await.expect("root");
    std::fs::write(
        std::path::Path::new(&root.path).join("mix.wav"),
        b"a second take",
    )
    .unwrap();
    backend
        .checkpoint_now(root_id.get(), None)
        .await
        .expect("second checkpoint");

    let now = backend.read(root_id, p("mix.wav")).await.expect("read");
    assert_eq!(
        redeem(&backend, &now.token, None).await.unwrap(),
        b"a second take"
    );

    let then = backend
        .read_at(root_id, p("mix.wav"), original)
        .await
        .expect("read_at");
    assert_eq!(
        redeem(&backend, &then.token, None).await.unwrap(),
        first,
        "a ticket is bound to an immutable address, not to a path"
    );
}

/// The federation read path: bytes already held resolve from the local
/// content-addressed store, with nothing to reach out to.
// t[verify files.scale.large-media]
#[tokio::test(flavor = "multi_thread")]
async fn content_already_held_resolves_without_an_origin() {
    let (_data, backend, root_id, bytes) = rig().await;
    let chain = backend
        .chain(root_id.get(), "mix.wav".into())
        .await
        .expect("chain");
    let content = ContentId::new(chain[0].file_id.clone());

    let ticket = backend.read_content(content).await.expect("read_content");
    assert_eq!(ticket.length, Some(bytes.len() as u64));
    assert_eq!(
        redeem(&backend, &ticket.token, None).await.unwrap(),
        bytes,
        "a content address is resolved by a store lookup"
    );

    // Content nobody here holds is refused rather than fetched.
    let absent = ContentId::new(
        files_store::chunk::blake3::hash(b"never stored")
            .to_hex()
            .to_string(),
    );
    assert!(matches!(
        backend.read_content(absent).await,
        Err(FilesFault::Invalid(_))
    ));
}

/// A rendition ticket streams from the *private* rendition CAS, and
/// honours a range there too — the `<video>` proxy seek path.
#[tokio::test(flavor = "multi_thread")]
async fn a_rendition_ticket_streams_and_seeks() {
    let data = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(data.path(), data.path().join("vault")).unwrap();
    backend.set_transcoder(Arc::new(files_transcode::transcoder::FakeTranscoder));
    let root_dir = data.path().join("cuts");
    std::fs::create_dir(&root_dir).unwrap();
    // The fake transcoder classifies by a magic prefix.
    let mut video = b"VIDEO".to_vec();
    video.extend_from_slice(&vec![0x11u8; 4096]);
    std::fs::write(root_dir.join("cut.mov"), &video).unwrap();
    std::fs::write(root_dir.join("notes.txt"), b"not media").unwrap();

    let root = backend
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "Cuts".into(),
            RootFlavor::Media,
        )
        .await
        .unwrap();
    let root_id = RootId::new(root.id);
    backend.checkpoint_now(root.id, None).await.unwrap();
    // The ladder warm-up is spawned detached; the fake is deterministic
    // and fast.
    tokio::time::sleep(Duration::from_millis(400)).await;

    let ticket = MediaService::rendition(&backend, root_id, p("cut.mov"), RenditionKind::Proxy720)
        .await
        .expect("proxy ticket");
    assert_eq!(
        ticket.content_type, "video/mp4",
        "a proxy of a .mov is an mp4 — the rendition's type, not the source's"
    );
    let whole = redeem(&backend, &ticket.token, None).await.expect("redeem");
    assert_eq!(whole.len() as u64, ticket.length.unwrap());
    assert!(whole.starts_with(b"proxy-720:"));

    let head = redeem(&backend, &ticket.token, Some((0, 9)))
        .await
        .expect("ranged");
    assert_eq!(head, &whole[..10]);
}

/// Listing reports what was generated, and does not generate to answer.
#[tokio::test(flavor = "multi_thread")]
async fn renditions_lists_what_was_generated() {
    let data = tempfile::tempdir().unwrap();
    let backend = FilesBackend::new(data.path(), data.path().join("vault")).unwrap();
    backend.set_transcoder(Arc::new(files_transcode::transcoder::FakeTranscoder));
    let root_dir = data.path().join("cuts");
    std::fs::create_dir(&root_dir).unwrap();
    let mut video = b"VIDEO".to_vec();
    video.extend_from_slice(&vec![0x11u8; 4096]);
    std::fs::write(root_dir.join("cut.mov"), &video).unwrap();
    std::fs::write(root_dir.join("notes.txt"), b"not media").unwrap();

    let root = backend
        .create_root(
            root_dir.to_string_lossy().into_owned(),
            "Cuts".into(),
            RootFlavor::Media,
        )
        .await
        .unwrap();
    let root_id = RootId::new(root.id);
    backend.checkpoint_now(root.id, None).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;

    let listed = backend
        .renditions(root_id, p("cut.mov"))
        .await
        .expect("renditions");
    assert!(
        listed.iter().any(|r| r.kind == RenditionKind::Proxy720),
        "a warmed video ladder includes a 720p proxy: {listed:?}"
    );
    assert!(
        listed.iter().all(|r| r.len > 0 && !r.file_id.is_empty()),
        "a listed rendition is one that exists"
    );

    // A file with no ladder lists nothing — and asking cost no
    // transcode, which is the property that makes this a query.
    assert!(
        backend
            .renditions(root_id, p("notes.txt"))
            .await
            .expect("renditions")
            .is_empty()
    );
}

/// `files.handoff.editor`: region bounds survive the trip.
// t[verify files.handoff.editor]
#[tokio::test(flavor = "multi_thread")]
async fn a_handoff_carries_its_regions_to_collection() {
    let (_data, backend, root_id, _bytes) = rig().await;
    let hit = Region::Time {
        start_ms: 40_000,
        end_ms: 52_000,
    };
    let handoff = backend
        .handoff(
            "Selects".into(),
            HandoffTarget::Timeline,
            vec![HandoffItem {
                root_id,
                path: p("mix.wav"),
                region: hit.clone(),
            }],
        )
        .await
        .expect("handoff");
    assert!(!handoff.token.is_empty());
    assert_eq!(handoff.items[0].region, hit);

    let collected = backend.collect_handoff(&handoff.token).expect("collect");
    assert_eq!(collected.target, HandoffTarget::Timeline);
    assert_eq!(
        collected.items[0].region, hit,
        "a hit covering 0:40-0:52 must arrive as that range, not the whole clip"
    );
    assert_eq!(collected.items[0].path, p("mix.wav"));

    // A delivery happens once; a bin arriving twice is a duplicated bin.
    assert!(matches!(
        backend.collect_handoff(&handoff.token),
        Err(FilesFault::Invalid(_))
    ));
}

/// A handoff naming a path that is not there would deliver a bin with
/// holes in it.
// t[verify files.handoff.editor]
#[tokio::test(flavor = "multi_thread")]
async fn a_handoff_of_something_absent_is_refused() {
    let (_data, backend, root_id, _bytes) = rig().await;
    assert!(matches!(
        backend
            .handoff(
                "Selects".into(),
                HandoffTarget::Bin,
                vec![HandoffItem {
                    root_id,
                    path: p("ghost.wav"),
                    region: Region::Whole,
                }],
            )
            .await,
        Err(FilesFault::PathNotFound(_))
    ));
    assert!(matches!(
        backend
            .handoff("Empty".into(), HandoffTarget::Bin, vec![])
            .await,
        Err(FilesFault::Invalid(_))
    ));
}

/// The lane refuses what it cannot serve rather than minting a token
/// that fails at redemption.
#[tokio::test(flavor = "multi_thread")]
async fn what_is_not_implemented_refuses_rather_than_pretending() {
    use files_proto::service::write::WriteService;
    let (_data, backend, root_id, _bytes) = rig().await;

    // No archive writer exists in this crate, so no archive ticket is
    // minted. When one lands, this is the test that changes.
    let refused = WriteService::archive(&backend, root_id, vec![p("mix.wav")]).await;
    assert!(
        matches!(&refused, Err(FilesFault::Internal(m)) if m.contains("byte lane")),
        "got {refused:?}"
    );

    // Nothing is readable before it has been checkpointed: the byte lane
    // serves the store, and a file being written has no stable length.
    let root = backend.get_root(root_id.get()).await.unwrap();
    std::fs::write(std::path::Path::new(&root.path).join("new.wav"), b"fresh").unwrap();
    assert!(matches!(
        backend.read(root_id, p("new.wav")).await,
        Err(FilesFault::Invalid(_))
    ));
}
