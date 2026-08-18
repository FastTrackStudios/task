//! Chapter twenty-two — the shape a project is, and what it is kept in.
//!
//! `project.form.*`. A form declares what a project contains; a project
//! that does not match is flagged, never rejected; and components attach
//! to a piece and survive its promotion.
//!
//! # Flagged, never rejected, is the whole design
//!
//! A studio's real tree diverges from every grammar anyone writes for
//! it. The archive this model came from has albums with no `project.md`,
//! sessions at project level, and folders named things no vocabulary
//! anticipated. Software that refused those is software they stop using
//! — so every assertion about divergence here is about a *report*, and
//! the write that produced it succeeded.

use project::{Component, ComponentKind, Form};

use integration::scenario::Scenario;

fn draft(title: &str, form: Option<Form>) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        form,
        ..Default::default()
    }
}

// t[verify project.form.closed]
/// An unrecognised form reads as no form, and the project is fine.
///
/// "Unclassified is a real state, not a catch-all member of the enum."
/// A project declaring `form: interpretive-dance` is valid, nestable and
/// complete — it simply says nothing we can act on.
#[tokio::test]
async fn an_unrecognised_form_leaves_a_project_unclassified_and_valid() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let made = alice
        .projects()
        .await
        .create(draft("Something new", None))
        .await
        .expect("a project with no form is a project");
    assert_eq!(made.form, None);

    // And it does everything a formed project does.
    alice
        .projects()
        .await
        .add_part(made.id, "First".into())
        .await
        .expect("an unclassified project still has parts");
    assert!(
        alice
            .projects()
            .await
            .divergences(made.id)
            .await
            .expect("ask")
            .is_empty(),
        "a project with no form has nothing to diverge from"
    );
}

// t[verify project.form.grammar]
/// An album with no songs is flagged, and still an album.
#[tokio::test]
async fn a_project_that_does_not_match_its_form_is_flagged_not_refused() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let album = alice
        .projects()
        .await
        .create(draft("Crescendum", Some(Form::Album)))
        .await
        .expect("declaring a form must not refuse an empty project");

    let flagged = alice
        .projects()
        .await
        .divergences(album.id)
        .await
        .expect("ask what diverges");
    assert_eq!(flagged.len(), 1, "{flagged:?}");
    assert!(
        flagged[0].note.contains("song"),
        "the note should say what an album expects: {:?}",
        flagged[0].note
    );

    // Naming songs settles it.
    for song in ["Overture", "Daybreak"] {
        alice
            .projects()
            .await
            .add_part(album.id, song.into())
            .await
            .expect("name a song");
    }
    assert!(
        alice
            .projects()
            .await
            .divergences(album.id)
            .await
            .expect("ask again")
            .is_empty()
    );
}

// t[verify project.form.components]
/// A song carries a chart and two sessions, and keeps them when promoted.
///
/// "Components survive a part's promotion unchanged." They live on the
/// roster entry and promotion does not touch the roster, so this is true
/// by construction — which is exactly the kind of claim worth a test,
/// since the construction could change.
#[tokio::test]
async fn components_survive_a_parts_promotion() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let album = alice
        .projects()
        .await
        .create(draft("Crescendum", Some(Form::Album)))
        .await
        .expect("create");
    let song = alice
        .projects()
        .await
        .add_part(album.id, "Overture".into())
        .await
        .expect("name a song");

    // A chart, and the two sessions the real archive has — one song
    // tracked in Reaper and finished in Pro Tools.
    for (kind, name) in [
        (ComponentKind::Chart, "Overture.pdf"),
        (ComponentKind::Session, "Overture.rpp"),
        (ComponentKind::Session, "Overture.ptx"),
    ] {
        alice
            .projects()
            .await
            .attach_component(
                album.id,
                song.id,
                Component {
                    kind,
                    name: name.into(),
                },
            )
            .await
            .unwrap_or_else(|e| panic!("attach {name}: {e:?}"));
    }

    let carried = alice
        .projects()
        .await
        .parts(album.id)
        .await
        .expect("read the roster");
    assert_eq!(carried[0].components.len(), 3);

    alice
        .projects()
        .await
        .promote_part(album.id, song.id)
        .await
        .expect("promote the song");

    let after = alice
        .projects()
        .await
        .parts(album.id)
        .await
        .expect("read the roster again");
    assert_eq!(
        after[0].components.len(),
        3,
        "promotion disturbed the song's components: {:?}",
        after[0].components
    );
}

// t[verify project.form.components]
/// Two charts on one song is flagged — and allowed.
///
/// A song carries at most one chart, so a second is a divergence. It is
/// still written: the grammar reports, and a person who genuinely has
/// two charts is not someone the software should argue with.
#[tokio::test]
async fn a_component_the_grammar_does_not_expect_is_written_and_flagged() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let album = alice
        .projects()
        .await
        .create(draft("Crescendum", Some(Form::Album)))
        .await
        .expect("create");
    let song = alice
        .projects()
        .await
        .add_part(album.id, "Overture".into())
        .await
        .expect("name a song");

    for name in ["Overture.pdf", "Overture (piano).pdf"] {
        alice
            .projects()
            .await
            .attach_component(
                album.id,
                song.id,
                Component {
                    kind: ComponentKind::Chart,
                    name: name.into(),
                },
            )
            .await
            .expect("the grammar flags; it does not refuse");
    }

    let flagged = alice
        .projects()
        .await
        .divergences(album.id)
        .await
        .expect("ask");
    assert_eq!(flagged.len(), 1, "{flagged:?}");
    assert_eq!(flagged[0].part, Some(song.id));
    assert!(flagged[0].note.contains("at most one"), "{:?}", flagged[0]);
}
