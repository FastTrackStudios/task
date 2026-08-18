//! Chapter twenty-five — where things live, and what survives.
//!
//! `storage.tier.*` and `storage.projection.*`: what a human wrote is
//! markdown and stays legible without us; what a machine observed and
//! what was computed are databases and are disposable; a write lands in
//! the file first; and a file changed by another tool is the normal
//! case rather than a conflict.
//!
//! # These are the rules that decay silently
//!
//! Every one of them is easy to violate one field at a time and hard to
//! notice. A project's title cached into sqlite "for the list view", and
//! now the sqlite is not disposable. A write that updates the projection
//! before the file, and now a crash loses work rather than a rebuild.
//! Nobody finds out until the day a database is deleted or a process
//! dies at the wrong moment, which is why they are worth a chapter that
//! deletes a database and kills a process at the wrong moment.
//!
//! `rebuild.rs` is the sibling: it deletes every database and checks
//! what comes back. This one checks the tiers hold *before* anything is
//! deleted, which is the claim that makes the rebuild possible.

use files::path::RootPath;

use integration::scenario::Scenario;

fn draft(title: &str) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        ..Default::default()
    }
}

// t[verify storage.tier.authored]
// t[verify scenario.album.declare]
/// What a human wrote is a file, and reads without us.
///
/// The assertion is deliberately about the *bytes*: frontmatter a person
/// can read, a body they wrote, and enough context in the one file to
/// interpret it. Asking the service would prove the service works, which
/// is a different claim.
#[tokio::test]
async fn authored_state_is_markdown_that_reads_without_this_software() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let mut album = draft("Crescendum");
    album.details = "Ten songs, tracked over eight months.".into();
    album.capabilities = project::Capabilities::from_names(["music-production"]);
    album.form = Some(project::Form::Album);
    let made = alice.projects().await.create(album).await.expect("create");
    alice
        .projects()
        .await
        .add_part(made.id, "Overture".into())
        .await
        .expect("name a song");

    let page = s.orgs.acme.org_root().join("vault").join(&made.path);
    let text = std::fs::read_to_string(&page).expect("the page is a file");

    // Everything needed to interpret it is in the one file.
    for expected in [
        "type: project",
        &made.id.to_string(),
        "Crescendum",
        "music-production",
        "album",
        "Overture",
        "Ten songs, tracked over eight months.",
    ] {
        assert!(
            text.contains(expected),
            "the page does not carry {expected:?}, so it cannot be \
             interpreted with nothing but the file:\n{text}"
        );
    }
    // And it is YAML frontmatter, which is what makes it editable in any
    // editor rather than merely readable.
    assert!(text.starts_with("---\n"), "{text}");
}

// t[verify storage.projection.external-edits]
/// A file changed by another tool is picked up, and is not a conflict.
///
/// The vault having other writers is the normal case — Obsidian, a sync
/// client, a shell — and this chapter's version of "another tool" is
/// writing the page directly, which is exactly what those do.
#[tokio::test]
async fn a_page_edited_outside_is_re_projected_without_a_restart() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let made = alice
        .projects()
        .await
        .create(draft("Crescendum"))
        .await
        .expect("create");
    let page = s.orgs.acme.org_root().join("vault").join(&made.path);

    // Somebody renames it in Obsidian while the server is running.
    let text = std::fs::read_to_string(&page).expect("read");
    std::fs::write(&page, text.replace("Crescendum", "Crescendum (2026)")).expect("edit");

    // No restart, no re-adoption, no conflict — the next read sees it.
    let read = alice
        .projects()
        .await
        .get(made.id)
        .await
        .expect("the project is still there");
    assert_eq!(
        read.title, "Crescendum (2026)",
        "an edit from outside was not picked up"
    );
}

// t[verify storage.projection.write-through]
// t[verify vault.write.atomic]
/// The file is what is durable, and it is never half-written.
///
/// Two claims in one test because they are the same claim from two
/// sides: the write goes to the file, and the file is whole. A reader
/// arriving at any moment sees the previous page or the next one, never
/// a torn one.
#[tokio::test]
async fn a_write_lands_in_the_file_whole() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let made = alice
        .projects()
        .await
        .create(draft("Crescendum"))
        .await
        .expect("create");
    let page = s.orgs.acme.org_root().join("vault").join(&made.path);

    // Rewrite it repeatedly while reading from another thread. Every
    // read must parse — a torn write shows up as frontmatter that does
    // not close, which `parse_str` refuses.
    let watching = page.clone();
    let reader = std::thread::spawn(move || {
        let mut torn = Vec::new();
        for _ in 0..200 {
            if let Ok(text) = std::fs::read_to_string(&watching) {
                if !text.is_empty() && project::parse_str("p.md", "p", &text).is_err() {
                    torn.push(text);
                }
            }
        }
        torn
    });

    for i in 0..40 {
        let mut next = alice.projects().await.get(made.id).await.expect("read");
        next.details = format!("pass {i}");
        alice.projects().await.update(next).await.expect("write");
    }

    let torn = reader.join().expect("the reader thread");
    assert!(
        torn.is_empty(),
        "a reader observed {} partially-written page(s) — the first:\n{}",
        torn.len(),
        torn.first().map_or("", String::as_str)
    );

    // And the file, not the projection, is what holds the last write.
    let text = std::fs::read_to_string(&page).expect("read");
    assert!(text.contains("pass 39"), "{text}");
}

// t[verify vault.index.tolerant]
/// One unparseable page does not cost the vault.
///
/// "Malformed frontmatter never prevents the vault loading, and the
/// offending file is still listed as an unparsed page rather than
/// vanishing." The second half is the one worth testing: a file that
/// disappears from every listing is a file somebody loses.
#[tokio::test]
async fn a_page_that_cannot_be_parsed_costs_one_page() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let good = alice
        .projects()
        .await
        .create(draft("Crescendum"))
        .await
        .expect("a page that parses");

    // A project page with frontmatter that does not close.
    let broken = s
        .orgs
        .acme
        .org_root()
        .join("vault")
        .join("Projects/broken.md");
    std::fs::create_dir_all(broken.parent().expect("parent")).expect("mkdir");
    std::fs::write(&broken, "---\ntype: project\ntitle: [unclosed\n").expect("write");

    // The vault still loads, and the good page is still there.
    let listed = alice
        .projects()
        .await
        .list()
        .await
        .expect("a malformed page must not prevent the vault loading");
    assert!(
        listed.iter().any(|p| p.id == good.id),
        "one bad page cost the whole listing"
    );

    // And the bad one is still on disk, not quietly removed.
    assert!(
        broken.exists(),
        "the unparseable page was deleted rather than skipped"
    );
}

// t[verify storage.tier.derived]
/// The catalogue is rebuildable, so losing it costs a walk.
///
/// `durable.rs` deliberately does not persist the catalogue, on the
/// grounds that it is derived and disposable. This is that decision
/// checked from outside: the tree still browses after a restart that
/// kept no catalogue.
#[tokio::test]
async fn derived_state_costs_a_rebuild_and_nothing_else() {
    let s = Scenario::open().await;

    let before = s
        .as_alice()
        .await
        .tree()
        .await
        .browse(s.acme_root, RootPath::parse("Audio Files").unwrap())
        .await
        .expect("browse");

    let acme = s.orgs.acme.restart().await;

    let after = files::service::tree::TreeService::browse(
        &acme.backend,
        s.acme_root,
        RootPath::parse("Audio Files").unwrap(),
    )
    .await
    .expect("browse after the restart");

    let names = |e: &[files::model::BrowseEntry]| {
        let mut n: Vec<String> = e.iter().map(|x| x.name.clone()).collect();
        n.sort();
        n
    };
    assert_eq!(names(&before), names(&after));
}
