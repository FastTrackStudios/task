//! Chapter — the manifest is in the resources tier, the bytes are in a
//! File Root.
//!
//! ADR 0003 splits an asset in two on purpose: `resources/samples/<slug>/`
//! holds what the sample *is* — the identity a `sample:` reference
//! addresses, small enough that a subscription carries every one of them
//! across an org boundary — and a `ContentRef { root_id, path }` says
//! where its audio sits, in the Files layer that has the versioning, the
//! selective sync, the renditions and the chunked streaming.
//!
//! The risk in a split like that is a reference to nothing: a manifest
//! naming a root that was never adopted, or a path nobody wrote. Nothing
//! validates it — "stored as written and never followed" is the lane's
//! stated contract, and it is the right contract, because the Files lane
//! is what owns the bytes.
//!
//! So what this chapter proves is the round trip and the target: a root
//! is adopted, a file is in it, a sample is bound to that file, and the
//! reference that comes back off the wire resolves — through the Files
//! lane, over the wire, as the same person — to an entry that is really
//! there and is the size of what was written. No byte transfer is built
//! here; that is the Files lane's chapter and it has several.

use files::path::RootPath;
use integration::scenario::Scenario;
use resources_proto::{ContentRef, SampleDoc};

/// The audio, such as it is. Small on purpose — this chapter is about a
/// binding, and `scale.rs` is where size is the subject.
const KICK: &[u8] = b"RIFF....WAVEfmt room kick, take two";

fn sample(title: &str, content: ContentRef) -> SampleDoc {
    SampleDoc {
        slug: String::new(),
        title: title.into(),
        tags: vec!["kick".into(), "room".into()],
        duration_secs: 2,
        sample_rate: 48_000,
        body: "Captured in the live room, one mic up.".into(),
        content,
        updated_at: "2026-09-06T10:00:00Z".into(),
    }
}

#[tokio::test]
async fn a_sample_is_bound_to_a_file_that_is_really_there() {
    let s = Scenario::open().await;

    // A root somebody adopted, with the audio already in it — the
    // ordinary case, since the sample library was a folder on a disk
    // before it was anything else. Arranged through the backend, like
    // every other setup in this suite; everything asserted below goes
    // over the wire.
    let library = s.orgs.acme.tree().join("Samples");
    std::fs::create_dir_all(library.join("Kicks")).expect("the sample folder");
    std::fs::write(library.join("Kicks").join("room-kick.wav"), KICK).expect("the audio");
    let root = integration::orgs::adopt(&s.orgs.acme, "Samples").await;
    // A freshly adopted root is nobody's yet: access is on the content
    // (`files.access.granularity`), so the person who is going to read
    // it has to be given it, exactly as `People::hire` gives Alice the
    // session root.
    files::service::access::AccessService::grant(
        &s.orgs.acme.backend,
        s.people.alice.subject.clone(),
        root,
        RootPath::root(),
        task_server::example_org::Holds::Owner.capabilities(),
    )
    .await
    .expect("ACME grants Alice the sample library");

    let alice = s.as_alice().await;
    let path = RootPath::parse("Kicks/room-kick.wav").expect("a root-relative path");

    // ── the binding, written the way Signal writes it ────────────────
    let bound = ContentRef {
        root_id: root.to_string(),
        path: path.to_string(),
    };
    let saved = alice
        .resources()
        .await
        .upsert_sample(sample("Room Kick Take Two", bound.clone()))
        .await
        .expect("declare the sample");
    assert_eq!(saved.slug, "room-kick-take-two");
    assert_eq!(saved.rel_path, "samples/room-kick-take-two/sample.md");

    // ── and read back, unchanged ─────────────────────────────────────
    let read = alice
        .resources()
        .await
        .sample("room-kick-take-two".into())
        .await
        .expect("read the sample back");
    assert_eq!(
        read.content, bound,
        "the binding did not survive the round trip"
    );
    assert!(read.content.is_bound());

    // A library list carries it too, so a screen can say which rows are
    // playable without a read per row.
    let listed = alice
        .resources()
        .await
        .list_samples()
        .await
        .expect("list the samples");
    let row = listed
        .iter()
        .find(|x| x.slug == "room-kick-take-two")
        .expect("the sample is in the library listing");
    assert_eq!(row.content, bound);

    // ── the reference names something real ───────────────────────────
    //
    // Followed the only way it can be: through the Files lane, by the
    // root id and path the manifest carries, as the same person.
    let entry = alice
        .tree()
        .await
        .entry(root, path.clone())
        .await
        .unwrap_or_else(|e| {
            panic!("the sample's `ContentRef` names {path} in {root}, and the Files lane has no such entry: {e:?}")
        });
    assert_eq!(entry.root_id, root);
    assert_eq!(entry.path, path);
    assert_eq!(
        entry.size,
        KICK.len() as u64,
        "the bound path resolves to a different file than the one written"
    );

    // ── and the audio is not in the resources tier ───────────────────
    //
    // The whole reason for the split: subscribing to a sample library
    // moves manifests, not gigabytes.
    let home = s
        .orgs
        .acme
        .org_root()
        .join("resources/samples/room-kick-take-two");
    let mut names: Vec<String> = std::fs::read_dir(&home)
        .expect("the sample's home")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["sample.json", "sample.md"],
        "the resources tier grew a copy of the audio it deliberately does not hold"
    );
}

/// An unbound manifest is the ordinary state of a freshly declared
/// asset, and it says so rather than naming a file that is not there.
#[tokio::test]
async fn a_declared_sample_starts_with_nothing_bound() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    alice
        .resources()
        .await
        .upsert_sample(sample("Room Snare", ContentRef::default()))
        .await
        .expect("declare a sample with no audio yet");

    let read = alice
        .resources()
        .await
        .sample("room-snare".into())
        .await
        .expect("read it back");
    assert!(
        !read.content.is_bound(),
        "an unbound sample came back claiming bytes: {:?}",
        read.content
    );
}
