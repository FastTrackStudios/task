//! Chapter eighteen — delete every database and lose nothing.
//!
//! `scenario.album.rebuild`, and `storage.projection.rebuildable`
//! underneath it: every projection database is deleted, the server
//! restarts, and what a human wrote is still there. Only derived state —
//! indexes, thumbnails, transcripts — has to be rebuilt, and it rebuilds.
//!
//! # Why this is the chapter that justifies the storage tiers
//!
//! `storage.tier.authored` says authored state is markdown and stays
//! legible without us; `storage.tier.derived` says derived state is a
//! database and is disposable. Those two are easy to believe and easy to
//! violate one field at a time — a project's title cached into sqlite
//! "for the list view", and now the sqlite is not disposable and nobody
//! notices until the day it is deleted.
//!
//! The only test that catches that is this one: throw the databases
//! away, and see what a human wrote come back.
//!
//! # What "every database" means here
//!
//! The sqlite files in the org root — auth, timer, finance, threads,
//! prefs, identity — and not the content store or the vault. The store
//! holds the *bytes* and the vault holds the *markdown*, and both are
//! what the rule calls authored or observed rather than projected. A
//! test that deleted those would be testing whether files come back from
//! nowhere, which is not a property anything has.

use files::FilesService;
use files::path::RootPath;

use integration::scenario::Scenario;

/// A project draft. `id` nil and `path` empty means "you assign them" —
/// the same shape `office.rs` uses, and for the same reason: there is no
/// constructor, and spelling every field at each call site hides what the
/// test is about.
fn draft(title: &str) -> project::ProjectInfo {
    project::ProjectInfo {
        id: uuid::Uuid::nil(),
        path: String::new(),
        title: title.into(),
        status: "active".into(),
        priority: "normal".into(),
        project_type: "general".into(),
        lead: String::new(),
        tags: project::model::Tags::default(),
        parent_id: None,
        same_as: None,
        target_date: None,
        progress_percent: -1,
        details: String::new(),
        client_id: None,
        billable_default: false,
        currency: String::new(),
        default_rate_cents: 0,
        estimated_seconds: 0,
        agent_profile: String::new(),
        verify_command: String::new(),
        color: String::new(),
        image: String::new(),
        archived: false,
        states: None,
        date_created: None,
        date_modified: None,
    }
}

/// Every projection database in an org root, deleted.
///
/// Returns how many went, so the test can say it actually did something
/// — a rebuild that passes because nothing was deleted is the failure
/// this chapter is most likely to have.
fn delete_projections(root: &std::path::Path) -> usize {
    let mut gone = 0;
    for entry in std::fs::read_dir(root)
        .expect("read the org root")
        .flatten()
    {
        let path = entry.path();
        let is_db = path.extension().is_some_and(|e| e == "sqlite" || e == "db")
            || path
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.contains(".sqlite-"));
        if is_db {
            std::fs::remove_file(&path).expect("delete a projection");
            gone += 1;
        }
    }
    gone
}

// t[verify storage.projection.rebuildable]
// t[verify storage.tier.authored]
/// The project survives losing every database.
#[tokio::test]
async fn the_tree_comes_back_after_every_database_is_deleted() {
    let s = Scenario::open().await;

    // What a human can see before the disaster.
    let before = s
        .as_alice()
        .await
        .tree()
        .await
        .browse(s.acme_root, RootPath::parse("Audio Files").unwrap())
        .await
        .expect("browse the takes");
    assert!(!before.is_empty(), "nothing to lose");

    // Stop, delete, start — the order an operator has, and the order
    // that makes this a test about losing databases rather than about
    // pulling files out from under a running process.
    let org_root = s.orgs.acme.org_root();
    let mut gone = 0;
    let acme = s
        .orgs
        .acme
        .restart_with(|_| gone = delete_projections(&org_root))
        .await;
    assert!(
        gone > 0,
        "no databases were deleted, so this test proved nothing about \
         losing them"
    );

    // The roots register is reconstructed…
    let roots = FilesService::list_roots(&acme.backend)
        .await
        .expect("list roots");
    assert!(
        roots.iter().any(|r| r.id == s.acme_root.get()),
        "the adopted root did not come back: {roots:?}"
    );

    // …and so is everything under it. Read off the backend rather than
    // over the wire: the accounts went with the auth database this test
    // deliberately deleted, so there is nobody left to sign in as. That
    // is the rule holding rather than failing — an auth store is a
    // projection of nothing, and losing it loses logins, not work.
    let after = files::service::tree::TreeService::browse(
        &acme.backend,
        s.acme_root,
        RootPath::parse("Audio Files").unwrap(),
    )
    .await
    .expect("browse after the rebuild");

    let names = |entries: &[files::model::BrowseEntry]| {
        let mut n: Vec<String> = entries.iter().map(|e| e.name.clone()).collect();
        n.sort();
        n
    };
    assert_eq!(
        names(&before),
        names(&after),
        "the tree a human wrote did not survive losing the databases"
    );
}

// t[verify storage.tier.authored]
/// A project page is markdown, and markdown is what survives.
///
/// The vault half of the same claim: the page is on disk as text, so a
/// rebuild is a re-read rather than a recovery. Asserted by *reading the
/// file*, not by asking the service, because the service answering
/// correctly from a cache is exactly the failure mode.
#[tokio::test]
async fn what_a_human_wrote_is_a_file_on_disk() {
    let s = Scenario::open().await;

    let made = s
        .as_alice()
        .await
        .projects()
        .await
        .create(draft("Crescendum — mix and master"))
        .await
        .expect("create the album's project page");

    let page = s.orgs.acme.org_root().join("vault").join(&made.path);
    let text = std::fs::read_to_string(&page)
        .unwrap_or_else(|e| panic!("the project page is not on disk at {page:?}: {e}"));
    assert!(
        text.contains("Crescendum — mix and master"),
        "the page does not carry what was written: {text}"
    );
}
