//! `SearchService` against a real `FilesBackend` and a real root on
//! disk.
//!
//! In-process rather than over `LocalServer`, for the same reason
//! `roots_lane.rs` gives: the lane has no `permits.rs` row yet, so it is
//! not mounted on a router and cannot be reached over vox. These call the
//! trait directly, which is what the dispatcher would do.
//!
//! What these pin, deliberately, is the *honest* half of the lane as much
//! as the working half. Two of them assert that speech and vision are
//! refused by name, so that implementing them is a change to a failing
//! test rather than a discovery.

use files::FilesBackend;
use files_proto::id::RootId;
use files_proto::model::RootFlavor;
use files_proto::service::media::Region;
use files_proto::service::roots::{AdoptRequest, RootsService};
use files_proto::service::search::{Extract, Query, SearchService};
use files_proto::service::tree::TreeService;
use files_proto::{FilesFault, RootPath};

const BRIEF: &str = "The brief opens with a line about the dog.\n\
                     \n\
                     The second paragraph is about the cat instead.\n";

/// A backend with one adopted root holding a note, a binary file and a
/// PDF that is not really a PDF (its bytes never get read — the refusal
/// is by extension, which is the point).
async fn adopted() -> (tempfile::TempDir, FilesBackend, RootId, std::path::PathBuf) {
    let data_dir = tempfile::tempdir().expect("data tempdir");
    let dir = data_dir.path().join("session");
    std::fs::create_dir(&dir).unwrap();
    std::fs::create_dir(dir.join("notes")).unwrap();
    std::fs::write(dir.join("notes").join("brief.md"), BRIEF).unwrap();
    // Invalid UTF-8, so text extraction has to refuse it.
    std::fs::write(dir.join("take.wav"), [0xff, 0xfe, 0x00, 0x01, 0x02]).unwrap();
    std::fs::write(dir.join("deck.pdf"), b"%PDF-1.7 pretend").unwrap();

    let backend =
        FilesBackend::new(data_dir.path(), data_dir.path().join("vault")).expect("backend");
    let root = backend
        .adopt(AdoptRequest {
            path: dir.to_string_lossy().into_owned(),
            name: "Session".to_string(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .expect("adopt");
    (data_dir, backend, RootId::new(root.id), dir)
}

fn note() -> RootPath {
    RootPath::parse("notes/brief.md").unwrap()
}

fn query(text: &str, root_id: RootId) -> Query {
    Query {
        text: text.to_string(),
        root_id: Some(root_id),
        under: None,
        kinds: Vec::new(),
        limit: None,
    }
}

// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn extracted_text_makes_a_file_findable() {
    let (_tmp, backend, root_id, _dir) = adopted().await;

    assert!(
        backend
            .search(query("dog", root_id))
            .await
            .unwrap()
            .is_empty(),
        "nothing is findable before it is extracted"
    );

    let states = backend
        .extract(root_id, vec![note()], vec![Extract::Text])
        .await
        .expect("extract");
    assert_eq!(states.len(), 1);
    assert!(states[0].done, "{:?}", states[0].failed);

    let hits = backend.search(query("dog", root_id)).await.expect("search");
    assert_eq!(hits.len(), 1, "one block mentions the dog");
    assert_eq!(hits[0].path, note());
    assert_eq!(hits[0].kind, Extract::Text);
    assert!(hits[0].excerpt.contains("dog"));
}

// t[verify files.index.regions]
#[tokio::test(flavor = "multi_thread")]
async fn a_hit_addresses_a_region_and_not_the_whole_file() {
    let (_tmp, backend, root_id, dir) = adopted().await;
    backend
        .extract(root_id, vec![note()], vec![Extract::Text])
        .await
        .expect("extract");

    let hits = backend.search(query("cat", root_id)).await.expect("search");
    let Region::Bytes { start, end } = hits[0].region else {
        panic!(
            "a text hit is a byte range, not `Whole`: {:?}",
            hits[0].region
        );
    };
    assert!(start > 0, "the second paragraph does not begin at byte 0");

    // The region is an offset into the SOURCE, so it opens there.
    let source = std::fs::read_to_string(dir.join("notes").join("brief.md")).unwrap();
    let addressed = &source[start as usize..end as usize];
    assert!(
        addressed.contains("cat") && !addressed.contains("dog"),
        "the region must be the matching block alone, got {addressed:?}"
    );
}

// t[verify files.index.portable]
#[tokio::test(flavor = "multi_thread")]
async fn the_sidecar_is_an_ordinary_readable_file_beside_the_content() {
    let (_tmp, backend, root_id, dir) = adopted().await;
    let states = backend
        .extract(root_id, vec![note()], vec![Extract::Text])
        .await
        .expect("extract");

    let sidecar = states[0].sidecar.clone().expect("a sidecar path");
    assert_eq!(sidecar.as_str(), "notes/brief.md.text.extract.txt");

    // Read with plain `std::fs`, no application involved.
    let on_disk = dir.join("notes").join("brief.md.text.extract.txt");
    let text = std::fs::read_to_string(&on_disk).expect("the sidecar is a real file");
    assert!(text.starts_with("task-files-extract/1\n"), "{text}");
    assert!(text.contains("source: notes/brief.md"));
    assert!(text.contains("content: b3:"));
    assert!(
        text.contains("@ 0 42"),
        "byte offsets are in the file: {text}"
    );
    assert!(text.contains("about the dog"));
    assert!(
        text.is_ascii() || text.chars().all(|c| !c.is_control() || c == '\n'),
        "plain text, not a binary blob"
    );

    // Derived: deleting it loses nothing unrecoverable.
    std::fs::remove_file(&on_disk).unwrap();
    assert!(
        backend
            .search(query("dog", root_id))
            .await
            .unwrap()
            .is_empty(),
        "findability goes with it, and only that"
    );
    backend
        .extract(root_id, vec![note()], vec![Extract::Text])
        .await
        .expect("re-extract");
    assert_eq!(
        std::fs::read_to_string(&on_disk).unwrap(),
        text.replace(
            text.lines()
                .find(|l| l.starts_with("extracted-at:"))
                .unwrap(),
            std::fs::read_to_string(&on_disk)
                .unwrap()
                .lines()
                .find(|l| l.starts_with("extracted-at:"))
                .unwrap()
        ),
        "regenerated byte-for-byte from the source, bar the timestamp"
    );
    assert_eq!(
        backend.search(query("dog", root_id)).await.unwrap().len(),
        1
    );
}

// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn re_extracting_unchanged_content_is_a_no_op() {
    let (_tmp, backend, root_id, dir) = adopted().await;
    let first = backend
        .extract(root_id, vec![note()], vec![Extract::Text])
        .await
        .expect("extract");

    let again = backend
        .extract(root_id, vec![note()], vec![Extract::Text])
        .await
        .expect("re-extract");
    assert_eq!(
        again[0].updated_at, first[0].updated_at,
        "the content address matched, so nothing was redone"
    );

    // Change the content and the address stops matching.
    std::fs::write(
        dir.join("notes").join("brief.md"),
        "The brief now mentions a heron.\n",
    )
    .unwrap();
    let third = backend
        .extract(root_id, vec![note()], vec![Extract::Text])
        .await
        .expect("re-extract after edit");
    assert!(
        third[0].updated_at > first[0].updated_at,
        "changed content is redone"
    );
    assert_eq!(
        backend.search(query("heron", root_id)).await.unwrap().len(),
        1
    );
    assert!(
        backend
            .search(query("dog", root_id))
            .await
            .unwrap()
            .is_empty(),
        "the stale extract was replaced, not appended to"
    );
}

// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn a_file_that_fails_extraction_does_not_block_browsing_it() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    let binary = RootPath::parse("take.wav").unwrap();

    let states = backend
        .extract(root_id, vec![binary.clone(), note()], vec![Extract::Text])
        .await
        .expect("a batch with one bad file still succeeds");
    let failed = states.iter().find(|s| s.path == binary).expect("a row");
    assert!(!failed.done);
    assert!(
        failed
            .failed
            .as_deref()
            .is_some_and(|r| r.contains("not UTF-8")),
        "the reason is named: {:?}",
        failed.failed
    );
    assert!(
        states.iter().find(|s| s.path == note()).unwrap().done,
        "one file's failure must not cost its neighbour's findability"
    );

    // Storage is untouched: it still browses, and it still reads.
    let entries = backend
        .browse(root_id, RootPath::root())
        .await
        .expect("browse");
    assert!(
        entries.iter().any(|e| e.name == "take.wav"),
        "a file that cannot be extracted is still a file"
    );
}

// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn a_pdf_says_there_is_no_parser_rather_than_pretending() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    let states = backend
        .extract(
            root_id,
            vec![RootPath::parse("deck.pdf").unwrap()],
            vec![Extract::Text],
        )
        .await
        .expect("extract");
    assert!(
        states[0]
            .failed
            .as_deref()
            .is_some_and(|r| r.starts_with("not yet implemented: PDF text extraction")),
        "{:?}",
        states[0].failed
    );
}

/// The decision on record: there is no speech recogniser in this
/// workspace, and a fabricated transcript would be worse than none.
// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn speech_is_refused_by_name() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    let err = backend
        .extract(root_id, vec![note()], vec![Extract::Speech])
        .await
        .expect_err("nothing in this build can transcribe");
    let FilesFault::Internal(reason) = err else {
        panic!("expected an Internal fault naming the gap");
    };
    assert!(
        reason.starts_with("not yet implemented: speech transcription"),
        "{reason}"
    );
}

/// The same decision for vision.
// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn vision_is_refused_by_name() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    let err = backend
        .extract(root_id, vec![note()], vec![Extract::Vision])
        .await
        .expect_err("nothing in this build can describe a shot");
    let FilesFault::Internal(reason) = err else {
        panic!("expected an Internal fault naming the gap");
    };
    assert!(
        reason.starts_with("not yet implemented: visual description"),
        "{reason}"
    );
}

/// A batch that mixes the possible with the impossible gets its possible
/// half — refusing the whole request would cost a caller its text.
// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn an_impossible_kind_does_not_take_a_possible_one_down_with_it() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    let states = backend
        .extract(root_id, vec![note()], vec![Extract::Speech, Extract::Text])
        .await
        .expect("the text half is servable");
    assert!(states.iter().any(|s| s.kind == Extract::Text && s.done));
    assert!(
        states
            .iter()
            .any(|s| s.kind == Extract::Speech && s.failed.is_some())
    );
    assert_eq!(
        backend.search(query("dog", root_id)).await.unwrap().len(),
        1
    );
}

// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn technical_metadata_is_extracted_and_searchable() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    backend
        .extract(root_id, Vec::new(), vec![Extract::Technical])
        .await
        .expect("extract the whole root");

    let mut q = query("take.wav", root_id);
    q.kinds = vec![Extract::Technical];
    let hits = backend.search(q).await.expect("search");
    let hit = hits
        .iter()
        .find(|h| h.path.as_str() == "take.wav")
        .expect("the wav's metadata");
    assert_eq!(
        hit.region,
        Region::Whole,
        "metadata describes the file as a whole; a byte range would be invented"
    );
    assert!(hit.excerpt.contains("extension: wav"));
}

/// `pending` walks the tree rather than reading the state file, so a file
/// nothing has attempted is outstanding rather than absent.
// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn pending_names_what_is_not_searchable_yet() {
    let (_tmp, backend, root_id, _dir) = adopted().await;

    let before = backend.pending(root_id).await.expect("pending");
    assert!(
        before
            .iter()
            .any(|s| s.path == note() && s.kind == Extract::Text),
        "an untouched file is outstanding: {before:?}"
    );

    backend
        .extract(root_id, vec![note()], vec![Extract::Text])
        .await
        .expect("extract");
    let after = backend.pending(root_id).await.expect("pending");
    assert!(
        !after
            .iter()
            .any(|s| s.path == note() && s.kind == Extract::Text),
        "what succeeded drops out of the queue"
    );

    // A failure stays visible: it is not searchable and it is not coming
    // back on its own.
    backend
        .extract(
            root_id,
            vec![RootPath::parse("take.wav").unwrap()],
            vec![Extract::Text],
        )
        .await
        .expect("extract");
    assert!(
        backend
            .pending(root_id)
            .await
            .unwrap()
            .iter()
            .any(|s| s.path.as_str() == "take.wav"
                && s.kind == Extract::Text
                && s.failed.is_some()),
        "`is it all searchable yet` must not answer yes about a file that failed"
    );
}

// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn state_for_one_file_covers_every_kind_this_build_can_do() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    let states = backend.extract_state(root_id, note()).await.expect("state");
    let kinds: Vec<_> = states.iter().map(|s| s.kind).collect();
    assert_eq!(kinds, vec![Extract::Text, Extract::Technical]);
    assert!(states.iter().all(|s| !s.done && s.failed.is_none()));
}

/// A sidecar is not itself content to index — otherwise extraction never
/// reaches a fixed point.
// t[verify files.index.portable]
#[tokio::test(flavor = "multi_thread")]
async fn a_derived_index_is_not_extracted_in_turn() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    backend
        .extract(root_id, Vec::new(), Vec::new())
        .await
        .expect("extract everything");
    let again = backend
        .extract(root_id, Vec::new(), Vec::new())
        .await
        .expect("and again");
    assert!(
        !again
            .iter()
            .any(|s| s.path.as_str().ends_with(".extract.txt")),
        "the walk skips sidecars: {:?}",
        again.iter().map(|s| s.path.as_str()).collect::<Vec<_>>()
    );
    assert!(
        backend
            .pending(root_id)
            .await
            .unwrap()
            .iter()
            .all(|s| s.failed.is_some()),
        "after a full pass the only outstanding rows are the ones that failed"
    );
}

// t[verify files.index.regions]
#[tokio::test(flavor = "multi_thread")]
async fn a_query_is_narrowed_by_subtree_and_by_kind() {
    let (_tmp, backend, root_id, _dir) = adopted().await;
    backend
        .extract(root_id, Vec::new(), Vec::new())
        .await
        .expect("extract everything");

    let mut q = query("brief", root_id);
    q.under = Some(RootPath::parse("notes").unwrap());
    assert!(
        backend
            .search(q)
            .await
            .unwrap()
            .iter()
            .all(|h| h.path.as_str().starts_with("notes/")),
        "`under` is a subtree restriction"
    );

    let mut q = query("dog", root_id);
    q.kinds = vec![Extract::Technical];
    assert!(
        backend.search(q).await.unwrap().is_empty(),
        "the dog is in the text, not in the metadata"
    );
}

// t[verify files.index.extraction]
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_root_is_a_typed_fault_rather_than_an_empty_answer() {
    let (_tmp, backend, _root_id, _dir) = adopted().await;
    let ghost = RootId::new(uuid::Uuid::from_bytes([9; 16]));
    assert!(matches!(
        backend.pending(ghost).await,
        Err(FilesFault::RootNotFound(_))
    ));
    assert!(matches!(
        backend.search(query("dog", ghost)).await,
        Err(FilesFault::RootNotFound(_))
    ));
}
