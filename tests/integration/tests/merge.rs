//! Chapter twenty-three — two halves of one job, started separately.
//!
//! `project.lifecycle.merge` and `project.lifecycle.merge-identity`, and
//! the scenario stages that need them: the video company started their
//! own project for the concert film before anyone thought to coordinate,
//! and two projects now exist for one job with no common ancestor.
//!
//! # Merging is normal, not a recovery
//!
//! The rule says so in as many words, and it changes what the code has
//! to do. A recovery tool may reasonably refuse a messy input and demand
//! it be cleaned up first; a normal operation may not. So a merge whose
//! halves disagree about the title still merges, and hands the
//! disagreement back.
//!
//! # The absorbed id is the hard part
//!
//! "Share links already in a client's hands" have to keep working. That
//! rules out deleting the absorbed project, and it rules out a mapping
//! table too — a table is a thing that can be missing on the machine the
//! link was opened against. So the absorbed page stays, as an alias, and
//! every read follows it.

use project::Form;

use integration::scenario::Scenario;

fn draft(title: &str, form: Option<Form>) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        form,
        ..Default::default()
    }
}

// t[verify project.lifecycle.merge]
// t[verify scenario.piano.merge]
/// Two independent projects become one, and nothing is discarded.
#[tokio::test]
async fn two_projects_started_separately_become_one() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    // The audio company's album.
    let mut album = draft("Journey", Some(Form::Album));
    album.capabilities = project::Capabilities::from_names(["music-production"]);
    let album = projects.create(album).await.expect("the album");
    for piece in ["Prelude", "Nocturne"] {
        projects
            .add_part(album.id, piece.into())
            .await
            .expect("name a piece");
    }

    // The video company's concert film, started without coordinating.
    let mut film = draft("Journey — concert film", Some(Form::Concert));
    film.capabilities = project::Capabilities::from_names(["video-production"]);
    let film = projects.create(film).await.expect("the film");
    projects
        .add_part(film.id, "Finale".into())
        .await
        .expect("the film names a piece the album does not");

    let merged = projects
        .merge(album.id, film.id)
        .await
        .expect("two halves of one job merge");

    let one = projects.get(album.id).await.expect("the merged project");
    // Capabilities union.
    assert_eq!(
        one.capabilities.held,
        vec![
            project::Capability::MusicProduction,
            project::Capability::VideoProduction
        ]
    );
    // Parts combine.
    let pieces: Vec<String> = projects
        .pieces(album.id)
        .await
        .expect("pieces")
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(pieces, ["Prelude", "Nocturne", "Finale"]);

    // And the disagreements come back rather than being settled quietly.
    let fields: Vec<&str> = merged.conflicts.iter().map(|c| c.field.as_str()).collect();
    assert!(fields.contains(&"title"), "{:?}", merged.conflicts);
    assert!(fields.contains(&"form"), "{:?}", merged.conflicts);
    // Both values survive, which is what "nothing is silently discarded"
    // means for a field only one of which can win.
    let title = merged
        .conflicts
        .iter()
        .find(|c| c.field == "title")
        .expect("a title conflict");
    assert_eq!(title.kept, "Journey");
    assert_eq!(title.absorbed, "Journey — concert film");
}

// t[verify project.lifecycle.merge-identity]
// t[verify scenario.piano.merge-identity]
/// The link the client already had still works.
#[tokio::test]
async fn a_former_identity_resolves_to_the_merged_project() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    let album = projects
        .create(draft("Journey", Some(Form::Album)))
        .await
        .expect("the album");
    let film = projects
        .create(draft("Journey — concert film", Some(Form::Concert)))
        .await
        .expect("the film");

    // What the client was sent a week ago: the film's id.
    let link = film.id;

    projects.merge(album.id, film.id).await.expect("merge");

    let opened = projects
        .get(link)
        .await
        .expect("a link held before the merge still opens");
    assert_eq!(
        opened.id, album.id,
        "the former identity dangled instead of resolving to the merge"
    );
    assert_eq!(opened.title, "Journey");

    // And the absorbed half is no longer a second project in every
    // listing — which is the duplication the merge was performed to end.
    let listed = projects.list().await.expect("list");
    assert!(
        !listed.iter().any(|p| p.id == film.id),
        "the absorbed project is still listed as its own project"
    );
    assert!(listed.iter().any(|p| p.id == album.id));
}

// t[verify project.lifecycle.merge-identity]
/// The merge records what it absorbed.
///
/// "So the history stays legible to someone who only knew one half" —
/// a person opening the film's page a year later should learn where it
/// went, not find an empty file or a 404.
#[tokio::test]
async fn the_merge_leaves_a_record_of_what_it_absorbed() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    let album = projects
        .create(draft("Journey", Some(Form::Album)))
        .await
        .expect("the album");
    let film = projects
        .create(draft("Journey — concert film", None))
        .await
        .expect("the film");
    projects.merge(album.id, film.id).await.expect("merge");

    // Read the page off disk, not through the lane: the lane follows the
    // alias, and what is being checked is what a person opening the file
    // in Obsidian sees.
    let page = s.orgs.acme.org_root().join("vault").join(&film.path);
    let text = std::fs::read_to_string(&page)
        .unwrap_or_else(|e| panic!("the absorbed page is gone from {page:?}: {e}"));
    assert!(
        text.contains(&album.id.to_string()),
        "the page should say where it went: {text}"
    );
    assert!(text.contains("Merged into"), "{text}");
}

// t[verify project.lifecycle.merge]
/// A subproject of the absorbed half is reparented, not orphaned.
#[tokio::test]
async fn what_hung_off_the_absorbed_project_hangs_off_the_merged_one() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    let album = projects
        .create(draft("Journey", Some(Form::Album)))
        .await
        .expect("the album");
    let film = projects
        .create(draft("Journey — concert film", None))
        .await
        .expect("the film");

    let mut cut = draft("Opening titles", None);
    cut.parent_id = Some(film.id);
    let cut = projects
        .create(cut)
        .await
        .expect("the film has a subproject");

    projects.merge(album.id, film.id).await.expect("merge");

    let reparented = projects.get(cut.id).await.expect("the subproject survives");
    assert_eq!(
        reparented.parent_id,
        Some(album.id),
        "the subproject still points at an alias, so every listing that \
         walks parentage loses it"
    );
}

/// A project cannot absorb itself.
#[tokio::test]
async fn a_project_cannot_merge_with_itself() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;
    let album = projects
        .create(draft("Journey", None))
        .await
        .expect("the album");

    assert!(
        projects.merge(album.id, album.id).await.is_err(),
        "merging a project into itself would alias it to itself and make \
         every read of it a chain that never settles"
    );
}
