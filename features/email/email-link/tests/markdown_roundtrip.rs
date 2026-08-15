//! End-to-end: a Project's markdown body contains
//! `[[email://...]]` wikilinks. Parse them, write each into the
//! link store, then query both directions and assert the result
//! matches what the source markdown declared.

use chrono::Utc;
use email_link::{
    EmailLink, EmailWikilink, EntityRef, LinkStore, format_wikilink, parse_wikilinks,
};

#[test]
fn project_markdown_drives_link_index() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = LinkStore::open(dir.path()).unwrap();

    // Pretend this is the body of `montreal-album.md`.
    let project_body = "\
# Montreal Album

Discussion threads:

- Studio booking: [[email://booking@montreal.test|Studio confirmation]]
- Mastering: [[email://master@montreal.test|Mastering quote]]
- Mix notes: [[email://<mix-v3@montreal.test>|Mix v3 notes]]

Reply to the studio booking is in [[email://booking@montreal.test]].
";

    let links = parse_wikilinks(project_body);
    // 3 distinct ids despite 4 wikilinks (booking referenced twice).
    assert_eq!(links.len(), 3);

    let project = EntityRef::project("montreal-album");
    for EmailWikilink {
        message_id,
        label: _,
    } in &links
    {
        store
            .upsert(&EmailLink {
                message_id: message_id.clone(),
                entity: project.clone(),
                linked_at: Some(Utc::now()),
                linked_by: Some("user".into()),
                user_tags: vec![],
            })
            .unwrap();
    }

    // Forward: project has 3 emails linked.
    let from_project = store.links_for_entity(&project).unwrap();
    assert_eq!(from_project.len(), 3);
    let ids: Vec<_> = from_project.iter().map(|l| l.message_id.as_str()).collect();
    assert!(ids.contains(&"booking@montreal.test"));
    assert!(ids.contains(&"master@montreal.test"));
    assert!(ids.contains(&"mix-v3@montreal.test"));

    // Reverse: the booking email knows it's linked to the project.
    let from_email = store.links_for_message("booking@montreal.test").unwrap();
    assert_eq!(from_email.len(), 1);
    assert_eq!(from_email[0].entity.id, "montreal-album");
}

#[test]
fn one_email_linked_to_multiple_entities() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = LinkStore::open(dir.path()).unwrap();

    // The "mastering quote" email is relevant to both the
    // project AND a person record.
    let project = EntityRef::project("montreal-album");
    let person = EntityRef::new(email_link::EntityKind::person(), "mastering-engineer");
    let mid = "master@montreal.test";

    store
        .upsert(&EmailLink {
            message_id: mid.into(),
            entity: project.clone(),
            linked_at: Some(Utc::now()),
            linked_by: Some("user".into()),
            user_tags: vec![],
        })
        .unwrap();
    store
        .upsert(&EmailLink {
            message_id: mid.into(),
            entity: person.clone(),
            linked_at: Some(Utc::now()),
            linked_by: Some("user".into()),
            user_tags: vec![],
        })
        .unwrap();

    let entities = store.links_for_message(mid).unwrap();
    assert_eq!(entities.len(), 2);
    let ids: Vec<_> = entities.iter().map(|l| l.entity.id.as_str()).collect();
    assert!(ids.contains(&"montreal-album"));
    assert!(ids.contains(&"mastering-engineer"));
}

#[test]
fn format_then_parse_is_idempotent() {
    let formatted = format_wikilink("<round-trip@example.com>", Some("Re: hello"));
    let body = format!("This is a wikilink: {formatted} in the middle.");
    let parsed = parse_wikilinks(&body);
    assert_eq!(parsed.len(), 1);
    assert_eq!(parsed[0].message_id, "round-trip@example.com");
    assert_eq!(parsed[0].label.as_deref(), Some("Re: hello"));
}
