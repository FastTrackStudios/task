//! Chapter twenty-one — what leaves the building.
//!
//! `project.deliverable.*` and the two client-facing stages that stand
//! on them: `scenario.album.deliver` ("five deliverables, not forty
//! files named individually") and `scenario.album.client-link` ("nothing
//! internal is reachable").
//!
//! # Declarations expand; they are not expanded once
//!
//! The rule says per-part deliverables "stay in step as parts are added
//! or removed". The tempting implementation writes out one row per song
//! when the declaration is made, and it is wrong in a way that takes
//! months to notice: the album grows an eleventh song and quietly owes
//! ten deliverables. So the expansion is derived on every read, and the
//! test that matters here adds a song *after* declaring and checks the
//! count moved.
//!
//! # The client view is a surface, not a flag
//!
//! `client_deliverables` takes no audience parameter. A parameter is a
//! thing a caller gets wrong once, quietly, and the consequence is a
//! client seeing session stems. There is nothing to pass, so there is
//! nothing to pass incorrectly — the same shape `files.review.scope`
//! asks for on the review lane.

use project::{Audience, Deliverable, Medium, Scope};

use integration::scenario::Scenario;

fn draft(title: &str) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        ..Default::default()
    }
}

fn declare(name: &str, medium: Medium, scope: Scope, audience: Audience) -> Deliverable {
    Deliverable {
        id: uuid::Uuid::nil(),
        name: name.into(),
        medium,
        scope,
        audience,
    }
}

/// An album with three songs and the concert's five declarations.
async fn concert(s: &Scenario) -> (integration::client::Session, project::ProjectInfo) {
    let alice = s.as_alice().await;
    let album = alice
        .projects()
        .await
        .create(draft("Crescendum"))
        .await
        .expect("create the album");
    for song in ["Overture", "Daybreak", "Finale"] {
        alice
            .projects()
            .await
            .add_part(album.id, song.into())
            .await
            .expect("name a song");
    }
    (alice, album)
}

// t[verify project.deliverable.kind]
// t[verify project.deliverable.scope]
// t[verify scenario.album.deliver]
/// Five declarations cover eleven things, and stay in step.
#[tokio::test]
async fn a_concert_declares_five_deliverables_not_twenty_one_files() {
    let s = Scenario::open().await;
    let (alice, album) = concert(&s).await;

    // The five the rule names: whole-project video and audio, per-part
    // audio and video, and a set of excerpts.
    for d in [
        declare(
            "Concert film",
            Medium::Video,
            Scope::WholeProject,
            Audience::Client,
        ),
        declare(
            "Album master",
            Medium::Audio,
            Scope::WholeProject,
            Audience::Client,
        ),
        declare(
            "Per-song audio",
            Medium::Audio,
            Scope::PerPart,
            Audience::Client,
        ),
        declare(
            "Per-song video",
            Medium::Video,
            Scope::PerPart,
            Audience::Client,
        ),
        declare(
            "Promo clips",
            Medium::Video,
            Scope::Excerpt,
            Audience::Public,
        ),
    ] {
        alice
            .projects()
            .await
            .declare_deliverable(album.id, d)
            .await
            .expect("declare");
    }

    let declared = alice
        .projects()
        .await
        .deliverables(album.id)
        .await
        .expect("read the declarations");
    assert_eq!(declared.len(), 5, "five declarations, not twenty-one");

    // Which expand to: two whole-project + (3 songs × 2 per-part) = 8.
    // Excerpts expand to nothing until one is chosen.
    let items = alice
        .projects()
        .await
        .deliverable_items(album.id)
        .await
        .expect("expand");
    assert_eq!(items.len(), 8, "{items:#?}");
    assert_eq!(
        items.iter().filter(|i| i.part.is_some()).count(),
        6,
        "one per song per per-part declaration"
    );
}

// t[verify project.deliverable.scope]
/// An eleventh song means an eleventh deliverable, without being told.
///
/// This is the assertion the whole "derived on read" decision exists
/// for. An implementation that expanded once at declaration time passes
/// every other test in this file.
#[tokio::test]
async fn a_new_song_is_owed_the_same_deliverables_as_the_others() {
    let s = Scenario::open().await;
    let (alice, album) = concert(&s).await;

    alice
        .projects()
        .await
        .declare_deliverable(
            album.id,
            declare(
                "Per-song audio",
                Medium::Audio,
                Scope::PerPart,
                Audience::Client,
            ),
        )
        .await
        .expect("declare");

    let before = alice
        .projects()
        .await
        .deliverable_items(album.id)
        .await
        .expect("expand")
        .len();
    assert_eq!(before, 3);

    // A song is added *after* the declaration.
    alice
        .projects()
        .await
        .add_part(album.id, "Encore".into())
        .await
        .expect("name a fourth song");

    let after = alice
        .projects()
        .await
        .deliverable_items(album.id)
        .await
        .expect("expand again");
    assert_eq!(
        after.len(),
        4,
        "the album grew a song and did not grow the deliverable it owes for it"
    );
    assert!(after.iter().any(|i| i.title == "Encore"), "{after:#?}");
}

// t[verify project.deliverable.scope]
/// Promotion does not change what a project owes.
///
/// "Per-part deliverables ... are unaffected by whether a part is
/// promoted." The pieces list made this free; the test is here because
/// "free" is a claim about the current implementation, not a property
/// anyone should have to re-derive.
#[tokio::test]
async fn promoting_a_song_changes_nothing_about_what_is_owed() {
    let s = Scenario::open().await;
    let (alice, album) = concert(&s).await;
    alice
        .projects()
        .await
        .declare_deliverable(
            album.id,
            declare(
                "Per-song video",
                Medium::Video,
                Scope::PerPart,
                Audience::Client,
            ),
        )
        .await
        .expect("declare");

    let before = alice
        .projects()
        .await
        .deliverable_items(album.id)
        .await
        .expect("expand");

    let pieces = alice
        .projects()
        .await
        .pieces(album.id)
        .await
        .expect("pieces");
    alice
        .projects()
        .await
        .promote_part(album.id, pieces[1].id)
        .await
        .expect("promote the middle song");

    let after = alice
        .projects()
        .await
        .deliverable_items(album.id)
        .await
        .expect("expand again");
    assert_eq!(
        before.iter().map(|i| i.title.clone()).collect::<Vec<_>>(),
        after.iter().map(|i| i.title.clone()).collect::<Vec<_>>(),
        "promotion changed what the album owes"
    );
}

// t[verify project.deliverable.client-view]
// t[verify scenario.album.client-link]
/// A client sees deliverables, and never what is internal.
#[tokio::test]
async fn nothing_internal_is_reachable_from_the_client_view() {
    let s = Scenario::open().await;
    let (alice, album) = concert(&s).await;

    for d in [
        declare(
            "Album master",
            Medium::Audio,
            Scope::WholeProject,
            Audience::Client,
        ),
        declare(
            "Per-song audio",
            Medium::Audio,
            Scope::PerPart,
            Audience::Client,
        ),
        // The ones that are nobody's business but the studio's.
        declare(
            "Session stems",
            Medium::Audio,
            Scope::PerPart,
            Audience::Internal,
        ),
        declare(
            "Mix notes",
            Medium::Document,
            Scope::WholeProject,
            Audience::Internal,
        ),
    ] {
        alice
            .projects()
            .await
            .declare_deliverable(album.id, d)
            .await
            .expect("declare");
    }

    let member = alice
        .projects()
        .await
        .deliverable_items(album.id)
        .await
        .expect("the member view");
    assert!(
        member.iter().any(|i| i.name == "Session stems"),
        "the studio can see its own stems"
    );

    let client = alice
        .projects()
        .await
        .client_deliverables(album.id)
        .await
        .expect("the client view");
    assert!(
        client.iter().all(|i| i.audience > Audience::Internal),
        "an internal deliverable is reachable from the client view: {client:#?}"
    );
    assert!(
        !client
            .iter()
            .any(|i| i.name.contains("stems") || i.name.contains("Mix notes")),
        "{client:#?}"
    );
    // And what remains is the client's four: one master, three songs.
    assert_eq!(client.len(), 4, "{client:#?}");

    // Organised whole-project first, which is "one obvious path": the
    // whole performance, then a specific song.
    assert!(
        client[0].part.is_none(),
        "the client view should open with the whole thing: {client:#?}"
    );
}

// t[verify project.deliverable.binding]
/// A declared-and-unbound deliverable is a legible state.
///
/// A project saying what it owes before it owes it is what a deliverable
/// list is *for* at the start of a job — so an unbound item appears in
/// the client view as outstanding rather than being hidden. "The
/// per-song video is not done yet" is the answer the client came for.
#[tokio::test]
async fn a_deliverable_nobody_has_delivered_yet_still_appears() {
    let s = Scenario::open().await;
    let (alice, album) = concert(&s).await;

    alice
        .projects()
        .await
        .declare_deliverable(
            album.id,
            declare(
                "Per-song video",
                Medium::Video,
                Scope::PerPart,
                Audience::Client,
            ),
        )
        .await
        .expect("declare");

    let client = alice
        .projects()
        .await
        .client_deliverables(album.id)
        .await
        .expect("the client view");
    assert_eq!(
        client.len(),
        3,
        "nothing has been bound to any of these, and all three are still \
         owed: {client:#?}"
    );
}

/// Withdrawing a declaration stops it being owed, and touches nothing.
#[tokio::test]
async fn withdrawing_a_declaration_does_not_delete_work() {
    let s = Scenario::open().await;
    let (alice, album) = concert(&s).await;

    let d = alice
        .projects()
        .await
        .declare_deliverable(
            album.id,
            declare(
                "Promo clips",
                Medium::Video,
                Scope::WholeProject,
                Audience::Public,
            ),
        )
        .await
        .expect("declare");

    alice
        .projects()
        .await
        .withdraw_deliverable(album.id, d.id)
        .await
        .expect("withdraw it");

    assert!(
        alice
            .projects()
            .await
            .deliverables(album.id)
            .await
            .expect("read")
            .is_empty()
    );
    // The album's own pieces are untouched — withdrawing what is owed is
    // not a statement about the work.
    let pieces = alice
        .projects()
        .await
        .pieces(album.id)
        .await
        .expect("pieces");
    assert_eq!(pieces.len(), 3);
}
