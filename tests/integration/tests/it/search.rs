//! Chapter fourteen — finding a thing by what is in it.
//!
//! `scenario.album.search` asks for a phrase spoken inside a two-hour
//! recording to come back as the seconds it occupies, extracted on the
//! studio's own hardware, with the transcript beside the media as a
//! plain file.
//!
//! Speech is not implemented yet (`IMPLEMENTED` in the search lane is
//! `Text` and `Technical`), so what this chapter can honestly assert is
//! the *shape* the rule is about — a hit is a region, extraction needs
//! no credential, and nothing leaves the machine — against the kind that
//! does work. The moment speech lands, these tests want a take with
//! words in it and one more assertion; nothing else here changes.
//!
//! # Why "no credential" is worth a test
//!
//! `files.index.local` is the rule most easily broken by a change that
//! looks like an improvement. Reaching for a hosted model is one line,
//! makes extraction better, and turns "your files became searchable"
//! into "your files were uploaded" — silently, because the feature still
//! works. A test that runs with an empty environment and expects results
//! is the thing that notices.

use files::path::RootPath;
use files::service::media::Region;
use files::service::search::{Extract, Query};

use integration::scenario::Scenario;

/// A note with a findable phrase in it, in ACME's session folder.
///
/// Written to disk and then pinned, rather than pushed through a lane.
/// That is what `files.adopt.in-place` describes — the applications that
/// wrote this tree keep writing it — and it is how a session note
/// actually appears: a DAW or a person put it there. `pin` is the step
/// that makes the store see it.
async fn write_note(s: &Scenario, path: &str, body: &str) {
    let file = s.orgs.acme.tree().join("Song").join(path);
    std::fs::create_dir_all(file.parent().expect("a parent")).expect("mkdir");
    std::fs::write(&file, body).expect("write the note");
    integration::scenario::pin(&s.orgs.acme, s.acme_root, "session notes").await;
}

// t[verify files.index.local]
// t[verify files.index.extraction]
/// Extraction runs with nothing configured.
///
/// No API key, no endpoint, no account — the process environment this
/// test runs in has none, and that is the point. A `failed` row here
/// with "no credential" in it would be the rule broken.
#[tokio::test]
async fn a_file_becomes_searchable_with_no_credential_anywhere() {
    let s = Scenario::open().await;
    write_note(
        &s,
        "Audio Files/session-notes.txt",
        "take four is the keeper, the intro lands at last",
    )
    .await;

    let states = s
        .as_alice()
        .await
        .search()
        .await
        .extract(
            s.acme_root,
            vec![RootPath::parse("Audio Files/session-notes.txt").unwrap()],
            vec![Extract::Text],
        )
        .await
        .expect("extraction is available on this machine, unconfigured");

    assert!(
        !states.is_empty(),
        "asking for text extraction of a text file returned nothing"
    );
    for state in &states {
        assert!(
            !format!("{state:?}").to_lowercase().contains("credential"),
            "extraction asked for a credential — `files.index.local` says \
             every rule holds with none set: {state:?}"
        );
    }
}

// t[verify files.index.regions]
// t[verify scenario.album.search] — a search returns the region inside
// the file, not the file
/// A hit is a region, not a file.
///
/// The whole difference between this and grep: "it is in this file
/// somewhere" is what a user already knew.
#[tokio::test]
async fn a_hit_addresses_a_region_rather_than_a_file() {
    let s = Scenario::open().await;
    write_note(
        &s,
        "Audio Files/session-notes.txt",
        "take four is the keeper, the intro lands at last",
    )
    .await;

    let search = s.as_alice().await.search().await;
    search
        .extract(
            s.acme_root,
            vec![RootPath::parse("Audio Files/session-notes.txt").unwrap()],
            vec![Extract::Text],
        )
        .await
        .expect("extract");

    let hits = search
        .search(Query {
            text: "keeper".into(),
            root_id: Some(s.acme_root),
            under: None,
            kinds: vec![Extract::Text],
            limit: None,
        })
        .await
        .expect("search");

    let Some(hit) = hits.first() else {
        // Not an assertion failure with a bare `unwrap`: the useful
        // report is which file was searched and found nothing.
        panic!("no hit for a phrase that is in the extracted text");
    };
    assert_eq!(hit.path.to_string(), "Audio Files/session-notes.txt");
    assert!(
        !matches!(hit.region, Region::Whole),
        "a hit must land where the phrase is, not on the file as a whole: \
         {:?}",
        hit.region
    );
    assert!(
        hit.excerpt.to_lowercase().contains("keeper"),
        "the excerpt should carry the match: {:?}",
        hit.excerpt
    );
}

// t[verify files.index.local]
/// Nothing about searching reaches outside this org's own root.
///
/// The negative half: a query scoped to ACME must not answer with VNT's
/// content, whatever either server happens to hold. Extraction being
/// local is only half the promise — the other half is that becoming
/// searchable does not widen who can find it.
#[tokio::test]
async fn searching_one_org_never_answers_with_anothers_content() {
    let s = Scenario::open().await;
    write_note(&s, "Audio Files/acme-only.txt", "chorus doubling notes").await;

    let hits = s
        .as_victor()
        .await
        .search()
        .await
        .search(Query {
            text: "chorus".into(),
            root_id: None,
            under: None,
            kinds: vec![Extract::Text],
            limit: None,
        })
        .await
        .expect("VNT may search its own world");

    assert!(
        hits.iter().all(|h| h.root_id != s.acme_root),
        "VNT's search reached into ACME's root: {hits:?}"
    );
}
