#![allow(clippy::large_futures)]
//! End-to-end check for the sermon sync's server half: `ResourcesService
//! .upsert_sermon` over vox against a live `task-server`.
//!
//! Boots the example org on an ephemeral port, upserts a sermon twice —
//! the second time after a hand edit to the manifest body and an
//! annotation added to the sidecar — and asserts the sync kept both,
//! then reads everything back the way the UI does: `list_sermons`,
//! `sermon`, `transcript(rel_path)` (including the one-directory-down
//! lookup the watch view relies on), the `sermon → verse` links in the
//! typed-link store, and — with the scripture plugin — the reader's
//! `chapter_backlinks`, which is where "every sermon that mentions this
//! verse" is answered.

// This binary uses the boot helpers only; the seed constants are for
// the vault suites.
#[allow(dead_code)]
mod support;

use links_proto::{LinksServiceClient, NodeRef};
use resources_proto::{ResourcesServiceClient, SermonResource, TranscriptSegment};

fn seg(start: f32, text: &str) -> TranscriptSegment {
    TranscriptSegment {
        start,
        dur: 4.0,
        text: text.into(),
    }
}

fn sermon(title: &str, segments: Vec<TranscriptSegment>) -> SermonResource {
    SermonResource {
        folder: "crossroads".into(),
        video_id: "YMypVgZXFIU".into(),
        video_url: "https://youtu.be/YMypVgZXFIU".into(),
        title: title.into(),
        channel: "Crossroads Church".into(),
        tags: vec!["sermon".into(), "crossroads".into()],
        published: "2026-06-14".into(),
        duration_secs: 2856,
        caption_kind: "auto".into(),
        language: "en".into(),
        segments,
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_twice_keeps_hand_edits_and_reads_back() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let resources: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    let links: LinksServiceClient = vox::connect_lane(&url).establish().await.unwrap();

    // First sync: three files, two scripture references, two links.
    let first = resources
        .upsert_sermon(sermon(
            "God Restores Broken People",
            vec![
                seg(0.5, "welcome to Crossroads"),
                seg(109.2, "turn to first Peter chapter five verse seven"),
                seg(508.0, "and then 1 Peter 5:1-4 says"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(first.slug, "god-restores-broken-people");
    assert_eq!(
        first.rel_path,
        "sermons/crossroads/god-restores-broken-people.md"
    );
    assert!(first.created);
    assert_eq!(first.scripture, ["1Pet.5.7", "1Pet.5.1-1Pet.5.4"]);
    assert_eq!(first.links, 2);

    // Hand-edit the body and add an annotation, as a reader would.
    let org_root = org_proto::DataRoot::from_env().unwrap().org(support::ORG);
    let base = org_root.resources_dir().join("sermons/crossroads");
    let md = base.join("god-restores-broken-people.md");
    let hand = std::fs::read_to_string(&md)
        .unwrap()
        .replace("## Notes", "## Outline\n- `0:00` — Welcome\n\n## Notes");
    std::fs::write(&md, hand).unwrap();
    let ann = base.join("god-restores-broken-people.annotations.json");
    std::fs::write(
        &ann,
        r#"{"slug":"god-restores-broken-people","annotations":[{"anchor":"t:109","label":"Peter","text":"cast your cares","color":null,"geometry":{"type":"timestamp","secs":109}}]}"#,
    )
    .unwrap();

    // Second sync (retitled, captions revised): same slug, body and
    // annotations kept, links replaced.
    let second = resources
        .upsert_sermon(sermon(
            "God Restores Broken People (HD)",
            vec![
                seg(109.2, "turn to first Peter chapter five verse seven"),
                seg(900.0, "John 21 verses 15 to 17"),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(second.slug, "god-restores-broken-people");
    assert!(!second.created && second.body_kept);
    assert_eq!(second.scripture, ["1Pet.5.7", "John.21.15-John.21.17"]);
    assert_eq!(second.links, 2);

    let text = std::fs::read_to_string(&md).unwrap();
    assert!(
        text.contains("## Outline\n- `0:00` — Welcome"),
        "body kept: {text}"
    );
    assert!(
        text.contains("title: God Restores Broken People\n"),
        "title not sync-owned"
    );
    assert!(
        text.contains("scripture:\n- 1Pet.5.7\n- John.21.15-John.21.17"),
        "{text}"
    );
    let ann_text = std::fs::read_to_string(&ann).unwrap();
    assert!(ann_text.contains("cast your cares"), "annotations kept");

    // Read back: list, one, transcript at its real path and one level up.
    let list = resources.list_sermons().await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].video_id, "YMypVgZXFIU");
    assert_eq!(list[0].folder, "crossroads");
    assert_eq!(list[0].title, "God Restores Broken People");
    let one = resources
        .sermon("god-restores-broken-people".to_owned())
        .await
        .unwrap();
    assert_eq!(
        one.transcript_rel_path,
        "sermons/crossroads/god-restores-broken-people.transcript.json"
    );
    let doc = resources
        .transcript(one.transcript_rel_path.clone())
        .await
        .unwrap();
    assert_eq!(doc.segments.len(), 2);
    assert_eq!(doc.source, "youtube-auto");
    let doc_up = resources
        .transcript("sermons/god-restores-broken-people.transcript.json".to_owned())
        .await
        .unwrap();
    assert_eq!(
        doc_up.segments, doc.segments,
        "watch view's `sermons/<slug>` path resolves"
    );

    // The links store holds exactly the second sync's links, anchored
    // at the moment each verse was named.
    let at_109 = links
        .links_for(NodeRef::sermon("god-restores-broken-people").at(109))
        .await
        .unwrap();
    assert_eq!(at_109.len(), 1);
    assert_eq!(at_109[0].target.to_token(), "verse:1Pet.5.7");
    assert_eq!(at_109[0].provenance.source_ref, "sermon-sync");
    let at_508 = links
        .links_for(NodeRef::sermon("god-restores-broken-people").at(508))
        .await
        .unwrap();
    assert!(at_508.is_empty(), "the first sync's link was replaced");

    // The scripture reader lists the sermon on 1 Peter 5:7 at 1:49.
    #[cfg(feature = "plugin-scripture")]
    {
        use scripture_proto::ScriptureServiceClient;
        let scripture: ScriptureServiceClient = vox::connect_lane(&url).establish().await.unwrap();
        let bl = scripture
            .chapter_backlinks("1 Peter".to_owned(), 5)
            .await
            .unwrap();
        let v7 = bl
            .iter()
            .find(|b| b.verse == 7)
            .expect("1 Peter 5:7 has a backlink");
        let s = &v7.notes[0];
        assert_eq!(s.note_path, "sermon:god-restores-broken-people");
        assert_eq!(s.note_title, "God Restores Broken People");
        assert_eq!(s.source_kind, "sermon");
        assert_eq!(s.secs, 109);
        let john = scripture
            .chapter_backlinks("John".to_owned(), 21)
            .await
            .unwrap();
        assert_eq!(
            john.iter().map(|b| b.verse).collect::<Vec<_>>(),
            vec![15, 16, 17],
            "a range backlinks every verse it covers"
        );
    }
}
