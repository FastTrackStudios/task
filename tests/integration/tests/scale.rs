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

use files::path::RootPath;
use files::service::roots::RootsService;
use files::service::tree::TreeService;
use files::model::RootFlavor;
use files_domain::Hosting;

use integration::scenario::Scenario;

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("test path")
}

/// The size the fixture writes. Three chunks' worth at the default
/// average, so anything that splits has room to.
const TAKE: u64 = 3 << 20;

/// A replica with a tree, ready to pull content into.
///
/// Distinct from a structure host, and the difference is the whole of
/// `files.peering.replication`: a host with no tree has nowhere for
/// bytes to land and says so rather than pretending to hold them.
async fn replica(s: &Scenario) -> (tempfile::TempDir, std::path::PathBuf, files::FilesBackend) {
    let dir = tempfile::tempdir().expect("replica dir");
    let tree = dir.path().join("replica");
    let backend = files::FilesBackend::new(dir.path(), dir.path().join("vault"))
        .expect("replica backend");
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

    assert_eq!(again.chunks_fetched, 0, "a settled replica re-fetched content");
}

/// Two identical takes are one thing in the store.
///
/// A bounced stem, a duplicated take, a "FINAL v2" that is byte-for-byte
/// the previous one: ordinary in a session folder, and the difference
/// between a project costing its size and costing twice its size. The
/// tree keeps both files, because adoption moves nothing and the DAW
/// wrote two — what is shared is the content behind them.
// t[verify files.scale.capacity]
#[tokio::test]
async fn identical_takes_share_one_copy_of_the_content() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let tree = alice.tree().await;

    let one = tree
        .entry(s.acme_root, p("Audio Files/drums.wav"))
        .await
        .expect("the take");
    let copy = tree
        .entry(s.acme_root, p("Audio Files/drums-copy.wav"))
        .await
        .expect("the copy");

    assert_eq!(one.size, TAKE);
    assert_eq!(copy.size, TAKE);
    assert_eq!(
        one.content, copy.content,
        "two byte-identical takes were stored as two different things"
    );

    // Both still on disk as themselves — the saving is in the store, and
    // adoption in place means the tree is untouched.
    let dir = s.orgs.acme.tree().join("Song").join("Audio Files");
    assert!(dir.join("drums.wav").exists());
    assert!(dir.join("drums-copy.wav").exists());
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

/// What a transfer actually resumes from.
///
/// `chunks_fetched`/`chunks_skipped` are documented as the resumability
/// counters, and this measures the granularity behind them: a 3 MiB take
/// is **one** chunk, because the store links whole files rather than
/// splitting them (`ChunkerConfig::DEFAULT_WHOLE_FILE_THRESHOLD` is 0 —
/// a link costs nothing at any size, and measuring a real import showed
/// a threshold saved nothing).
///
/// The consequence is worth being explicit about rather than discovering
/// during an incident: resume is **file-level**. An interrupted transfer
/// re-sends whole files, not the tail of one, so a 6 GiB take that fails
/// at 99% costs 6 GiB to retry. That is a fine trade for a session of
/// stems and a bad one for a single enormous video file, and which of
/// those a deployment has is not something this decision currently asks.
// t[verify files.scale.transport]
#[tokio::test]
async fn a_large_take_is_one_chunk_so_resume_is_file_level() {
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
