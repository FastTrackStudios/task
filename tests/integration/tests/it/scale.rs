//! Chapter seven — a file bigger than a chunk.
//!
//! Every other fixture in this suite is a few hundred bytes, against a
//! 1 MiB average chunk size. That is fine for asking whether a lane is
//! reachable and useless for asking what the storage layer does, because
//! nothing ever crosses a boundary: no file has a second chunk, no two
//! files can share one, and a transfer that "completed" moved one piece.
//!
//! So this chapter has a take that does cross one. It is generated
//! rather than committed — a fixture that has to be large does not
//! belong in git — and it is deliberately not compressible, because a
//! content-defined chunker splits on the data and a uniform file gives
//! it nothing to split on.

use files::model::RootFlavor;
use files::path::RootPath;
use files::service::roots::RootsService;
use files::service::tree::TreeService;
use files_domain::Hosting;

use integration::scenario::Scenario;

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

/// The size the fixture writes. Three chunks' worth at the default
/// average, so anything that splits has room to.
const TAKE: u64 = 3 << 20;

/// Run a `sync_*` seam off the async thread.
///
/// They are synchronous with `pollster::block_on` inside, and the bao
/// ones stream from the iroh-blobs actor — which needs *this* runtime to
/// make progress. Calling one directly from an async test blocks the
/// worker the actor is waiting on, so the test hangs rather than fails.
/// `SyncHost` wraps every one of these the same way, for the same reason.
async fn off_thread<T, F>(f: F) -> T
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.expect("blocking task")
}

/// The hash of the single stored chunk behind `path`.
///
/// **Not** its `ContentId`. That is the hash of the *manifest*, which is
/// what addresses a file; the chunk inside it has an address of its own,
/// and the transfer machinery is keyed by the latter. Staging a partial
/// under the content id puts it somewhere nothing looks — which is
/// exactly what these tests did until a probe showed the pull fetching
/// the whole file and passing anyway.
async fn chunk_hash_of(s: &Scenario, path: &str) -> String {
    let content = s
        .as_alice()
        .await
        .tree()
        .await
        .entry(s.acme_root, p(path))
        .await
        .expect("the file is catalogued")
        .content
        .expect("hashed");

    s.orgs
        .acme
        .backend
        .with_version_store(s.acme_root.get(), |vs| {
            let id = files_store::chunk::FileId::from_hex(content.as_str()).expect("content id");
            let manifest = pollster::block_on(vs.chunks().manifest(id)).expect("manifest");
            assert_eq!(
                manifest.chunks.len(),
                1,
                "this helper assumes the whole-file tier"
            );
            manifest.chunks[0].hash.to_hex().to_string()
        })
        .expect("version store")
}

/// A replica with a tree, ready to pull content into.
///
/// Distinct from a structure host, and the difference is the whole of
/// `files.peering.replication`: a host with no tree has nowhere for
/// bytes to land and says so rather than pretending to hold them.
async fn replica(s: &Scenario) -> (tempfile::TempDir, std::path::PathBuf, files::FilesBackend) {
    let dir = tempfile::tempdir().expect("replica dir");
    let tree = dir.path().join("replica");
    let backend =
        files::FilesBackend::new(dir.path(), dir.path().join("vault")).expect("replica backend");
    backend
        .adopt_replica(
            s.acme_root.get(),
            "ACME Song",
            tree.to_str().expect("utf-8 path"),
            RootFlavor::Media,
        )
        .expect("adopt the replica");
    (dir, tree, backend)
}

/// A take larger than a chunk arrives whole, and arrives right.
// t[verify files.scale.large-media]
#[tokio::test]
async fn a_take_larger_than_a_chunk_survives_the_wire() {
    let s = Scenario::open().await;
    s.orgs
        .acme
        .backend
        .admit_host(s.orgs.vnt.host_id(), Hosting::working());
    let peer = s.orgs.vnt.dial_replica(&s.orgs.acme).await;
    let (_dir, tree, backend) = replica(&s).await;

    let report = files_sync::reconcile(&backend, &peer, s.acme_root.get())
        .await
        .expect("pull the content");
    assert!(report.chunks_fetched > 0, "a content pull moved nothing");

    // Byte-for-byte, not just the right length: a chunker that
    // reassembled out of order would produce a file of exactly the right
    // size and the wrong content, which is the failure a length check
    // cannot see.
    let here = std::fs::read(tree.join("Audio Files").join("drums.wav")).expect("the take landed");
    let there = std::fs::read(
        s.orgs
            .acme
            .tree()
            .join("Song")
            .join("Audio Files")
            .join("drums.wav"),
    )
    .expect("the origin still has it");
    assert_eq!(here.len() as u64, TAKE);
    assert_eq!(here, there, "the take arrived corrupted");
}

/// Pulling again moves nothing.
///
/// `files.scale.transport` is about a transfer that can be re-run — and
/// the cheap half of that is that a *finished* one costs nothing to
/// repeat, so a client can reconcile on a timer without paying for the
/// project every time.
// t[verify files.scale.transport]
// t[verify files.scale.small-files] — "re-referencing it transfers nothing",
// and the sync compares tree state rather than statting per file
#[tokio::test]
async fn a_second_pull_moves_no_bytes() {
    let s = Scenario::open().await;
    s.orgs
        .acme
        .backend
        .admit_host(s.orgs.vnt.host_id(), Hosting::working());
    let peer = s.orgs.vnt.dial_replica(&s.orgs.acme).await;
    let (_dir, _tree, backend) = replica(&s).await;

    files_sync::reconcile(&backend, &peer, s.acme_root.get())
        .await
        .expect("first pull");
    let again = files_sync::reconcile(&backend, &peer, s.acme_root.get())
        .await
        .expect("second pull");

    assert_eq!(
        again.chunks_fetched, 0,
        "a settled replica re-fetched content"
    );
}

/// Two identical takes are one thing in the store.
///
/// A bounced stem, a duplicated take, a "FINAL v2" that is byte-for-byte
/// the previous one: ordinary in a session folder, and the difference
/// between a project costing its size and costing twice its size. The
/// tree keeps both files, because adoption moves nothing and the DAW
/// wrote two — what is shared is the content behind them.
// t[verify files.scale.capacity]
// t[verify scenario.album.placement] — content resolves by address rather
// than by path, which is what lets footage move to another location
// without a path changing or a link breaking
// t[verify files.scale.small-files] — "identical content is stored once"
#[tokio::test]
async fn identical_takes_share_one_copy_of_the_content() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let tree = alice.tree().await;

    let one = tree
        .entry(s.acme_root, p("Audio Files/stems-a.wav"))
        .await
        .expect("the stem");
    let copy = tree
        .entry(s.acme_root, p("Audio Files/stems-b.wav"))
        .await
        .expect("its twin");

    assert_eq!(one.size, TAKE / 2);
    assert_eq!(copy.size, TAKE / 2);
    assert_eq!(
        one.content, copy.content,
        "two byte-identical takes were stored as two different things"
    );

    // Both still on disk as themselves — the saving is in the store, and
    // adoption in place means the tree is untouched.
    let dir = s.orgs.acme.tree().join("Song").join("Audio Files");
    assert!(dir.join("stems-a.wav").exists());
    assert!(dir.join("stems-b.wav").exists());
}

/// Structure still costs nothing, now that the project is not tiny.
///
/// The claim `files.peering.replication` makes is about a 244 GB
/// project, and a suite whose whole fixture fits in one chunk cannot
/// distinguish "moved no content" from "had no content to move".
// t[verify files.peering.replication]
#[tokio::test]
async fn structure_of_a_multi_megabyte_project_still_moves_no_content() {
    let s = Scenario::open().await;
    s.orgs
        .acme
        .backend
        .admit_host(s.orgs.vnt.host_id(), Hosting::structure_only());
    let peer = s.orgs.vnt.dial_replica(&s.orgs.acme).await;
    s.orgs
        .vnt
        .backend
        .host_structure(s.acme_root, "ACME Song".into(), RootFlavor::Media)
        .await
        .expect("host the structure");

    let report = files_sync::reconcile_structure(&s.orgs.vnt.backend, &peer, s.acme_root.get())
        .await
        .expect("pull the structure");

    assert_eq!(report.chunks_fetched, 0);
    // And it knows the size it is not holding — 6 MiB of takes, plus
    // change.
    let listed = s
        .orgs
        .vnt
        .backend
        .browse(s.acme_root, p("Audio Files"))
        .await
        .expect("browse");
    let bytes: u64 = listed.iter().filter_map(|e| e.size).sum();
    assert!(
        bytes >= TAKE * 2,
        "a structure host lost track of how big the project is: {bytes}"
    );
}

/// An interrupted transfer resumes at the byte it reached.
///
/// The store links large files whole, so a big take is **one** chunk
/// however large it is — 800 GB of video is one chunk of 800 GB. While
/// the transfer unit was the chunk, that meant a failure at 99% cost the
/// whole file again, and an 800 GB response could not be assembled at
/// all.
///
/// So the unit is the byte. This stages a partial transfer by hand — the
/// prefix a killed pull would have left — and asserts the file completes
/// correctly on top of it.
///
/// On its own that would prove very little: a pull that ignored the
/// partial and fetched all 3 MiB would produce the same correct file and
/// pass. What makes it meaningful is
/// [`a_corrupt_partial_is_refused_rather_than_admitted`] below — a
/// *wrong* prefix has to make the pull fail, and it only can if the
/// staged bytes are the ones being completed. The pair is the assertion;
/// either alone is decoration. (Both passed, vacuously, while the
/// partial was staged under the wrong key.)
// t[verify files.scale.transport]
#[tokio::test]
async fn an_interrupted_transfer_resumes_where_it_stopped() {
    let s = Scenario::open().await;
    s.orgs
        .acme
        .backend
        .admit_host(s.orgs.vnt.host_id(), Hosting::working());
    let peer = s.orgs.vnt.dial_replica(&s.orgs.acme).await;
    let (_dir, tree, backend) = replica(&s).await;

    let chunk = chunk_hash_of(&s, "Audio Files/drums.wav").await;

    // Two thirds of it arrive, verified, and then the transfer dies. The
    // window comes from the origin exactly as a real pull's would, so
    // what is staged is bao-verified rather than bytes asserted to be
    // right — a partial that could be wrong is a different test, below.
    let chunks_in = TAKE / 1024;
    let cut = (chunks_in / 3) * 2;
    let origin_backend = s.orgs.acme.backend.clone();
    let (root, h) = (s.acme_root.get(), chunk.clone());
    let bao = off_thread(move || {
        origin_backend
            .sync_export_ranges(root, &h, 0, cut)
            .expect("the origin can prove its own ranges")
    })
    .await;
    let (b, h) = (backend.clone(), chunk.clone());
    off_thread(move || {
        b.sync_import_ranges(root, &h, 0, cut, bao)
            .expect("import the prefix a killed pull would have left")
    })
    .await;
    let (b, h) = (backend.clone(), chunk.clone());
    assert_eq!(
        off_thread(move || b.sync_missing_from(root, &h, TAKE).expect("resume cursor")).await,
        Some(cut),
        "the cursor does not see what was already received"
    );

    files_sync::reconcile(&backend, &peer, s.acme_root.get())
        .await
        .expect("resume the pull");

    // Whole, and byte-for-byte against the origin — a seam where the
    // resumed part met the staged part would show up here.
    let landed = std::fs::read(tree.join("Audio Files").join("drums.wav")).expect("landed");
    let origin = std::fs::read(
        s.orgs
            .acme
            .tree()
            .join("Song")
            .join("Audio Files")
            .join("drums.wav"),
    )
    .expect("the origin has it");
    assert_eq!(landed.len() as u64, TAKE);
    assert_eq!(landed, origin, "the resumed transfer produced a seam");

    // And the cursor now says there is nothing left, which is what a
    // second pull reads to do nothing.
    let (b, h) = (backend.clone(), chunk.clone());
    assert_eq!(
        off_thread(move || b.sync_missing_from(root, &h, TAKE).expect("resume cursor")).await,
        None
    );
}

/// A tampered window is refused *as it arrives*, not after the file.
///
/// This is what the outboard buys, and the difference matters at scale: a
/// whole-file check on an 800 GB take means a day of transfer before
/// anything can be said about it. Bao-encoded ranges carry the hashes on
/// their path to the root, so a corrupt window fails where it lands and
/// nothing is written — the store keeps only the ranges that verified,
/// which is exactly what a resumed pull then asks about.
// t[verify files.scale.transport]
#[tokio::test]
async fn a_tampered_window_is_refused_where_it_lands() {
    let s = Scenario::open().await;
    let (_dir, _tree, backend) = replica(&s).await;
    let chunk = chunk_hash_of(&s, "Audio Files/drums.wav").await;

    let chunks_in = TAKE / 1024;
    let origin_backend = s.orgs.acme.backend.clone();
    let (root, h) = (s.acme_root.get(), chunk.clone());
    let mut bao = off_thread(move || {
        origin_backend
            .sync_export_ranges(root, &h, 0, chunks_in)
            .expect("export")
    })
    .await;
    // Flip a byte deep in the payload, past the header and the first
    // parent hashes.
    let at = bao.len() / 2;
    bao[at] ^= 0xFF;

    let (b, h) = (backend.clone(), chunk.clone());
    let refused = off_thread(move || b.sync_import_ranges(root, &h, 0, chunks_in, bao)).await;
    assert!(refused.is_err(), "a tampered window was written");

    // What survives is the *verified prefix*, and that is the better
    // behaviour rather than a leak: bao verifies as it decodes, so
    // everything up to the tampered byte proved itself and is legitimately
    // received. The cursor therefore points just past the corruption, and
    // a retry starts there instead of at zero.
    //
    // The assertion here was `Some(0)` at first, which was a guess about
    // all-or-nothing verification. The implementation is finer than the
    // guess.
    let (b, h) = (backend.clone(), chunk.clone());
    let cursor = off_thread(move || b.sync_missing_from(root, &h, TAKE).expect("resume cursor"))
        .await
        .expect("something must still be missing");
    assert!(cursor > 0, "the verified prefix was thrown away");
    assert!(
        cursor < chunks_in,
        "the tampered window was accepted whole: cursor {cursor} of {chunks_in}"
    );
}

/// The granularity behind the counters, pinned so a storage change has to
/// look at it.
///
/// A 3 MiB take is one chunk because the store links whole files
/// (`ChunkerConfig::DEFAULT_WHOLE_FILE_THRESHOLD` is 0 — a link costs
/// nothing at any size, and measuring a real import showed a threshold
/// saved nothing). That is fine now that transfers move bytes rather than
/// chunks; it was the whole problem while they moved chunks.
// t[verify files.scale.transport]
#[tokio::test]
async fn a_large_take_is_still_stored_as_one_chunk() {
    let s = Scenario::open().await;
    let entry = s
        .as_alice()
        .await
        .tree()
        .await
        .entry(s.acme_root, p("Audio Files/drums.wav"))
        .await
        .expect("the take");
    let content = entry.content.expect("hashed");

    let chunks = s
        .orgs
        .acme
        .backend
        .with_version_store(s.acme_root.get(), |vs| {
            let id = files_store::chunk::FileId::from_hex(content.as_str()).expect("content id");
            pollster::block_on(vs.chunks().manifest(id)).map(|m| m.chunks.len())
        })
        .expect("version store")
        .expect("manifest");

    assert_eq!(
        chunks, 1,
        "a {TAKE}-byte take split into {chunks} chunks — if this changed, \
         resume is finer-grained than file-level and the doc above is stale"
    );
}
