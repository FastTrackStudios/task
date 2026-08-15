//! End-to-end: a fake vault on disk → `walk_vault` → resolver
//! → `LinkStore::rebuild_from`. Proves the link index is
//! self-populating from any Obsidian-shaped tree.

use email_link::{EntityRef, LinkStore, collect_links, default_resolver};

fn write_md(root: &std::path::Path, rel: &str, body: &str) {
    let path = root.join(rel);
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).unwrap();
    }
    std::fs::write(path, body).unwrap();
}

fn fixture(root: &std::path::Path) {
    // Project with `emails:` frontmatter + a couple body wikilinks.
    write_md(
        root,
        "projects/montreal-album.md",
        "---\n\
         type: project\n\
         id: montreal-album\n\
         emails:\n  - <booking@studio.test>\n  - <master@studio.test>\n\
         ---\n\
         # Montreal\n\n\
         Mix v3: [[email://mix@studio.test|Mix v3 notes]]\n\
         Booking reply: [[email://booking@studio.test]]\n",
    );
    // Task linking via map-shaped frontmatter (EmailRef shape).
    write_md(
        root,
        "tasks/finalize-master.md",
        "---\n\
         type: task\n\
         id: finalize-master\n\
         emails:\n\
         - message_id: <master@studio.test>\n  subject: Mastering quote\n\
         ---\n\
         body\n",
    );
    // Person record — same mastering email is relevant.
    write_md(
        root,
        "people/mastering-engineer.md",
        "---\n\
         type: person\n\
         id: mastering-engineer\n\
         ---\n\
         Discussion: [[email://master@studio.test]]\n",
    );
    // No frontmatter — should be skipped by the default resolver.
    write_md(root, "notes/scratch.md", "loose note, no kind\n");
    // Obsidian metadata — walker skips entirely.
    write_md(root, ".obsidian/config.md", "type: project\n");
}

#[test]
fn vault_walk_populates_link_index() {
    let vault = tempfile::tempdir().unwrap();
    fixture(vault.path());

    let store_dir = tempfile::tempdir().unwrap();
    let mut store = LinkStore::open(store_dir.path()).unwrap();

    let pairs = collect_links(vault.path(), &default_resolver);
    let n = store.rebuild_from(pairs).unwrap();
    assert!(n >= 5, "expected ≥5 link rows; got {n}");

    // Project sees all three messages it referenced (booking is
    // mentioned twice; dedup'd to one row).
    let project = EntityRef::project("montreal-album");
    let project_links = store.links_for_entity(&project).unwrap();
    let project_ids: Vec<_> = project_links
        .iter()
        .map(|l| l.message_id.as_str())
        .collect();
    assert_eq!(project_ids.len(), 3);
    assert!(project_ids.contains(&"booking@studio.test"));
    assert!(project_ids.contains(&"master@studio.test"));
    assert!(project_ids.contains(&"mix@studio.test"));

    // The mastering email is on both the project AND the task
    // AND the person — three reverse hits.
    let reverse = store.links_for_message("master@studio.test").unwrap();
    let kinds: Vec<_> = reverse.iter().map(|l| l.entity.kind.as_str()).collect();
    assert_eq!(reverse.len(), 3);
    assert!(kinds.contains(&"project"));
    assert!(kinds.contains(&"task"));
    assert!(kinds.contains(&"person"));

    // The lone task only sees its one mastering email.
    let task = EntityRef::task("finalize-master");
    assert_eq!(store.count_for_entity(&task).unwrap(), 1);
}

#[test]
fn vault_walk_skips_metadata_and_typeless_files() {
    let vault = tempfile::tempdir().unwrap();
    fixture(vault.path());
    let pairs = collect_links(vault.path(), &default_resolver);

    // No (entity, _) pair should originate from `.obsidian/` or
    // from `scratch.md` (no `type:`).
    for (entity, _) in &pairs {
        assert!(entity.id != "config");
        assert!(entity.id != "scratch");
    }
}

#[test]
fn rebuild_is_idempotent() {
    let vault = tempfile::tempdir().unwrap();
    fixture(vault.path());
    let store_dir = tempfile::tempdir().unwrap();
    let mut store = LinkStore::open(store_dir.path()).unwrap();

    let pairs = collect_links(vault.path(), &default_resolver);
    let first = store.rebuild_from(pairs.clone()).unwrap();
    let second = store.rebuild_from(pairs).unwrap();
    assert_eq!(first, second);

    // After two rebuilds the project still has exactly 3 links.
    let project = EntityRef::project("montreal-album");
    assert_eq!(store.count_for_entity(&project).unwrap(), 3);
}
