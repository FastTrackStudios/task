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
        wiki: String::new(),
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

/// A sermon synced with `wiki` is a page of that wiki
/// (`wikis/<wiki>/Resources/Sermons/<folder>/`), the wiki gets its
/// `Sermons.base`, and a folder synced into the org-wide tier moves
/// into the wiki with `relocate_sermons` — slugs and links intact.
#[tokio::test(flavor = "multi_thread")]
async fn wiki_sermons_live_in_the_wiki_and_a_tier_folder_relocates() {
    let (url, _tmp) = support::boot_ws().await.unwrap();
    let resources: ResourcesServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    let links: LinksServiceClient = vox::connect_lane(&url).establish().await.unwrap();
    let org_root = org_proto::DataRoot::from_env().unwrap().org(support::ORG);
    let wiki_dir = org_root.named_wiki_dir("bible");
    std::fs::create_dir_all(&wiki_dir).unwrap();

    // Straight into the wiki.
    let mut into_wiki = sermon(
        "Hope In Exile",
        vec![seg(
            30.0,
            "turn to Jeremiah chapter twenty nine verse eleven",
        )],
    );
    into_wiki.wiki = "bible".into();
    into_wiki.video_id = "WIKI000001".into();
    let out = resources.upsert_sermon(into_wiki).await.unwrap();
    assert_eq!(
        out.rel_path,
        "wikis/bible/Resources/Sermons/crossroads/hope-in-exile.md"
    );
    let sermons_root = wiki_dir.join("Resources/Sermons");
    assert!(sermons_root.join("crossroads/hope-in-exile.md").is_file());
    assert!(sermons_root.join("Sermons.base").is_file());
    assert!(!org_root.resources_dir().join("sermons").exists());

    // Into the org-wide tier, then moved.
    let mut into_tier = sermon("Tier Talk", vec![seg(5.0, "Psalm 23:1")]);
    into_tier.video_id = "TIER000001".into();
    let tier = resources.upsert_sermon(into_tier).await.unwrap();
    assert_eq!(tier.rel_path, "sermons/crossroads/tier-talk.md");
    let moved = resources
        .relocate_sermons("crossroads".into(), "bible".into())
        .await
        .unwrap();
    assert_eq!(moved, 1);
    assert!(sermons_root.join("crossroads/tier-talk.md").is_file());
    assert!(
        sermons_root
            .join("crossroads/tier-talk.transcript.json")
            .is_file()
    );
    assert!(!org_root.resources_dir().join("sermons/crossroads").exists());

    // Both read back as the wiki's, and transcripts resolve either way.
    let list = resources.list_sermons().await.unwrap();
    let mut slugs: Vec<(String, String)> = list
        .iter()
        .map(|s| (s.slug.clone(), s.wiki.clone()))
        .collect();
    slugs.sort();
    assert_eq!(
        slugs,
        [
            ("hope-in-exile".to_string(), "bible".to_string()),
            ("tier-talk".to_string(), "bible".to_string()),
        ]
    );
    for s in &list {
        let doc = resources
            .transcript(s.transcript_rel_path.clone())
            .await
            .unwrap();
        assert_eq!(doc.slug, s.slug);
    }
    // The verse links survived the move (keyed by slug, not path).
    let on_psalm = links.links_for(NodeRef::verse("Ps.23.1")).await.unwrap();
    assert!(
        on_psalm.iter().any(|l| l.source.id == "tier-talk"),
        "link kept: {on_psalm:?}"
    );
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
