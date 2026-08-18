//! Chapter twenty — an album's songs, which are not projects.
//!
//! `project.part.unit` and `scenario.piano.parts`: a project's work
//! divides into named parts, and "creating seven projects for seven
//! regions of a single recording is refused as ceremony".
//!
//! # The refusal is the feature
//!
//! There is nothing to stop anyone making seven projects — `create` is
//! right there. What the rule asks for is that they should not *have*
//! to, and the way to establish that is to show the cheaper thing
//! existing and costing nothing: no page, no directory, no marker.
//!
//! So the assertions here are mostly about what does **not** appear on
//! disk. A parts implementation that quietly wrote seven files would
//! pass every functional test and fail the rule entirely.
//!
//! # Over the wire, because the permit table is the other half
//!
//! Four new methods on a mounted lane is four new ways to fail closed.
//! `permits_cover_router` catches a lane with no table; only a call
//! catches a method the table forgot.

use integration::client::Session;
use integration::scenario::Scenario;

fn draft(title: &str) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        ..Default::default()
    }
}

/// The album, and the seven pieces it was chopped into.
const PIECES: [&str; 7] = [
    "Prelude",
    "Nocturne",
    "Interlude",
    "Waltz",
    "Elegy",
    "Scherzo",
    "Finale",
];

async fn album(s: &Scenario) -> (Session, project::ProjectInfo) {
    let alice = s.as_alice().await;
    let made = alice
        .projects()
        .await
        .create(draft("Journey"))
        .await
        .expect("create the album");
    (alice, made)
}

// t[verify project.part.unit]
// t[verify scenario.piano.parts]
/// Seven pieces, seven parts, and not one new file.
#[tokio::test]
async fn an_album_divides_into_parts_without_creating_projects() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let projects_before = alice.projects().await.list().await.expect("list").len();

    for piece in PIECES {
        alice
            .projects()
            .await
            .add_part(album.id, piece.into())
            .await
            .unwrap_or_else(|e| panic!("name the piece {piece}: {e:?}"));
    }

    let parts = alice
        .projects()
        .await
        .parts(album.id)
        .await
        .expect("read the parts back");
    assert_eq!(parts.len(), 7, "{parts:?}");
    let names: Vec<&str> = parts.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, PIECES, "parts come back in declaration order");

    // The rule, stated as an absence: no project was created.
    let projects_after = alice.projects().await.list().await.expect("list").len();
    assert_eq!(
        projects_before, projects_after,
        "dividing an album into pieces created projects — `project.part.unit` \
         says a part costs no page"
    );

    // And nothing landed on disk beside the album's own page.
    let vault = s.orgs.acme.org_root().join("vault");
    let pages: Vec<String> = walkdir(&vault)
        .into_iter()
        .filter(|p| PIECES.iter().any(|piece| p.contains(piece)))
        .collect();
    assert!(pages.is_empty(), "a part wrote itself a file: {pages:?}");
}

// t[verify project.part.unit]
/// A part's id survives a rename, because everything points at the id.
#[tokio::test]
async fn renaming_a_part_keeps_its_id() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let made = alice
        .projects()
        .await
        .add_part(album.id, "Prelude".into())
        .await
        .expect("name a piece");

    let renamed = alice
        .projects()
        .await
        .rename_part(album.id, made.id, "Prelude (revised)".into())
        .await
        .expect("rename it");

    assert_eq!(
        made.id, renamed.id,
        "a rename minted a new id, so everything referencing this piece \
         now points at nothing"
    );
    assert_eq!(renamed.name, "Prelude (revised)");
}

/// One song with two spellings is not two songs.
#[tokio::test]
async fn a_part_cannot_be_named_twice() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    alice
        .projects()
        .await
        .add_part(album.id, "Nocturne".into())
        .await
        .expect("name a piece");

    let again = alice
        .projects()
        .await
        .add_part(album.id, "nocturne".into())
        .await;
    assert!(
        again.is_err(),
        "an album with two Nocturnes is an album whose setlist \
         references are ambiguous"
    );
}

/// A removed part is gone, and nothing else is.
#[tokio::test]
async fn removing_a_part_leaves_the_others_alone() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let mut made = Vec::new();
    for piece in ["Prelude", "Nocturne", "Finale"] {
        made.push(
            alice
                .projects()
                .await
                .add_part(album.id, piece.into())
                .await
                .expect("name a piece"),
        );
    }

    alice
        .projects()
        .await
        .remove_part(album.id, made[1].id)
        .await
        .expect("remove the middle one");

    let left = alice
        .projects()
        .await
        .parts(album.id)
        .await
        .expect("read back");
    let names: Vec<&str> = left.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Prelude", "Finale"]);
}

// t[verify project.part.promotion]
// t[verify project.identity.stable]
/// A promoted part *is* the same thing, with a page.
///
/// The id is the assertion. Everything attached to a piece — links,
/// deliverables, setlist references, time — addresses it by id, so a
/// promotion that minted a new one would break all of it at once, and
/// would do it silently because the name would still be right.
#[tokio::test]
async fn promoting_a_part_keeps_the_id_everything_points_at() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let piece = alice
        .projects()
        .await
        .add_part(album.id, "Nocturne".into())
        .await
        .expect("name a piece");

    let promoted = alice
        .projects()
        .await
        .promote_part(album.id, piece.id)
        .await
        .expect("promote it");

    assert_eq!(
        promoted.id, piece.id,
        "promotion minted a new id, so every reference to this piece now \
         points at nothing"
    );
    assert_eq!(
        promoted.parent_id,
        Some(album.id),
        "and it knows its parent"
    );

    // The id resolves as a project now, and it is the same id that
    // resolved as a part a moment ago.
    let fetched = alice
        .projects()
        .await
        .get(piece.id)
        .await
        .expect("the piece resolves as a project");
    assert_eq!(fetched.title, "Nocturne");
}

// t[verify project.part.listing]
/// Ten songs, three promoted, one list, same order.
///
/// The stage `scenario.album.promote` describes: "three songs are
/// promoted, seven are not... the album is built the same way either
/// way". A caller reading a track listing must not be able to tell.
#[tokio::test]
async fn an_albums_pieces_are_one_list_before_and_after_promotion() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let mut named = Vec::new();
    for piece in PIECES {
        named.push(
            alice
                .projects()
                .await
                .add_part(album.id, piece.into())
                .await
                .expect("name a piece"),
        );
    }

    let before: Vec<String> = alice
        .projects()
        .await
        .pieces(album.id)
        .await
        .expect("read the pieces")
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(before, PIECES, "the roster, in order");

    // Promote the second and the fifth — deliberately not the first, so
    // an implementation that appends promoted pieces to the end fails.
    for i in [1, 4] {
        alice
            .projects()
            .await
            .promote_part(album.id, named[i].id)
            .await
            .expect("promote");
    }

    let after = alice
        .projects()
        .await
        .pieces(album.id)
        .await
        .expect("read the pieces again");
    let names: Vec<&str> = after.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names, PIECES,
        "promotion changed the album's track listing — order is the \
         roster's, and promotion does not touch the roster"
    );

    // The flag is there for whoever asks, and only for them.
    let promoted: Vec<&str> = after
        .iter()
        .filter(|p| p.promoted)
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(promoted, ["Nocturne", "Elegy"]);
}

// t[verify project.part.promotion]
/// Demotion gives the id back, and the album never noticed.
#[tokio::test]
async fn a_demoted_subproject_rejoins_its_album_where_it_was() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let mut named = Vec::new();
    for piece in ["Prelude", "Nocturne", "Finale"] {
        named.push(
            alice
                .projects()
                .await
                .add_part(album.id, piece.into())
                .await
                .expect("name a piece"),
        );
    }
    alice
        .projects()
        .await
        .promote_part(album.id, named[1].id)
        .await
        .expect("promote the middle one");

    let back = alice
        .projects()
        .await
        .demote_project(named[1].id)
        .await
        .expect("demote it again");
    assert_eq!(back.id, named[1].id, "demotion minted a new id");

    let pieces = alice
        .projects()
        .await
        .pieces(album.id)
        .await
        .expect("read the pieces");
    let names: Vec<&str> = pieces.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(
        names,
        ["Prelude", "Nocturne", "Finale"],
        "a round trip through promotion moved the piece"
    );
    assert!(
        pieces.iter().all(|p| !p.promoted),
        "the page should be gone: {pieces:?}"
    );
    // And it no longer resolves as a project.
    assert!(
        alice.projects().await.get(named[1].id).await.is_err(),
        "the subproject's page outlived its demotion"
    );
}

// t[verify project.part.demotable]
/// A subproject with a subproject cannot become a part, and says so.
#[tokio::test]
async fn demotion_refuses_what_a_part_cannot_hold() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let piece = alice
        .projects()
        .await
        .add_part(album.id, "Nocturne".into())
        .await
        .expect("name a piece");
    let song = alice
        .projects()
        .await
        .promote_part(album.id, piece.id)
        .await
        .expect("promote it");

    // The song grows a subproject of its own — the video treatment.
    let mut cut = draft("Nocturne — concert film");
    cut.parent_id = Some(song.id);
    alice
        .projects()
        .await
        .create(cut)
        .await
        .expect("the song gains its own subproject");

    let refused = alice.projects().await.demote_project(song.id).await;
    let Err(e) = refused else {
        panic!("a subproject with children was demoted, orphaning them");
    };
    let said = format!("{e:?}");
    assert!(
        said.contains("concert film"),
        "the refusal should name the child the caller has to deal with: {said}"
    );
}

// t[verify project.capability.multiple]
// t[verify project.capability.closed]
/// A project holds a set of capabilities, and only real ones.
#[tokio::test]
async fn capabilities_are_a_set_drawn_from_a_closed_vocabulary() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let mut with = album.clone();
    with.capabilities = project::Capabilities::from_names([
        "music-production",
        "video-production",
        "interpretive-dance",
    ]);
    let saved = alice
        .projects()
        .await
        .update(with)
        .await
        .expect("give the album its capabilities");

    assert_eq!(
        saved.capabilities.held,
        vec![
            project::Capability::MusicProduction,
            project::Capability::VideoProduction
        ],
        "a set, not a type"
    );

    // Re-read from the server: the unrecognised one was never written,
    // so it does not survive the round trip.
    let read = alice
        .projects()
        .await
        .get(album.id)
        .await
        .expect("read it back");
    assert_eq!(read.capabilities.held.len(), 2);
    assert!(
        read.capabilities.unrecognised.is_empty(),
        "an unrecognised capability was written back to the page: {:?}",
        read.capabilities.unrecognised
    );
}

// t[verify project.capability.mutable]
/// A capability is added and withdrawn without touching a byte.
///
/// `scenario.piano.capability-churn`: video is added to a piece and
/// later dropped from the release, "and not one byte goes with it".
/// Removal is not deletion, and re-adding restores the surface over the
/// same content.
#[tokio::test]
async fn a_capability_is_added_and_withdrawn_over_content_that_stays() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    // The album's content is a Files root, not the project page — which
    // is the point: capabilities are declared on the page and the bytes
    // are somewhere else entirely.
    let takes_before = s
        .as_alice()
        .await
        .tree()
        .await
        .browse(
            s.acme_root,
            files::path::RootPath::parse("Audio Files").unwrap(),
        )
        .await
        .expect("browse the takes");

    let mut with_video = album.clone();
    with_video.capabilities =
        project::Capabilities::from_names(["music-production", "video-production"]);
    alice
        .projects()
        .await
        .update(with_video)
        .await
        .expect("video is added to the release");

    let mut without = alice.projects().await.get(album.id).await.expect("read");
    without.capabilities = project::Capabilities::from_names(["music-production"]);
    let dropped = alice
        .projects()
        .await
        .update(without)
        .await
        .expect("video is dropped from the release");
    assert_eq!(
        dropped.capabilities.held,
        vec![project::Capability::MusicProduction]
    );

    let takes_after = s
        .as_alice()
        .await
        .tree()
        .await
        .browse(
            s.acme_root,
            files::path::RootPath::parse("Audio Files").unwrap(),
        )
        .await
        .expect("browse again");
    assert_eq!(
        takes_before
            .iter()
            .map(|e| e.name.clone())
            .collect::<Vec<_>>(),
        takes_after
            .iter()
            .map(|e| e.name.clone())
            .collect::<Vec<_>>(),
        "withdrawing a capability took content with it"
    );

    // And re-adding it restores the declaration over the same content.
    let mut again = alice.projects().await.get(album.id).await.expect("read");
    again.capabilities =
        project::Capabilities::from_names(["music-production", "video-production"]);
    let restored = alice.projects().await.update(again).await.expect("re-add");
    assert!(
        restored
            .capabilities
            .has(project::Capability::VideoProduction)
    );
}

// t[verify project.nesting.uniform]
/// A subproject is a project, including in having parts of its own.
///
/// "Anything true of a project is true of a subproject." The cheapest
/// way to be wrong here is to special-case depth somewhere — parts only
/// at the top, or capabilities only at the top — so the test is that a
/// promoted song does the things an album does.
#[tokio::test]
async fn a_subproject_does_everything_a_project_does() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let piece = alice
        .projects()
        .await
        .add_part(album.id, "Nocturne".into())
        .await
        .expect("name a piece");
    let song = alice
        .projects()
        .await
        .promote_part(album.id, piece.id)
        .await
        .expect("promote it");

    // It has parts of its own — a song divided into movements.
    for movement in ["Andante", "Allegro"] {
        alice
            .projects()
            .await
            .add_part(song.id, movement.into())
            .await
            .unwrap_or_else(|e| panic!("a subproject may have parts too: {e:?}"));
    }
    let movements = alice
        .projects()
        .await
        .pieces(song.id)
        .await
        .expect("read them back");
    assert_eq!(movements.len(), 2);

    // And those promote, which is nesting without limit.
    alice
        .projects()
        .await
        .promote_part(song.id, movements[0].id)
        .await
        .expect("a part of a subproject promotes like any other");
}

// t[verify project.nesting.explicit]
/// Parentage is the declared link, not where the file sits.
///
/// A promoted song's page lands under `Projects/` like every other
/// project — nowhere near its album's page — and is still its child.
/// "Hardcoded directory names express no hierarchy and are not
/// consulted."
#[tokio::test]
async fn parentage_is_declared_rather_than_read_off_the_path() {
    let s = Scenario::open().await;
    let (alice, album) = album(&s).await;

    let piece = alice
        .projects()
        .await
        .add_part(album.id, "Nocturne".into())
        .await
        .expect("name a piece");
    let song = alice
        .projects()
        .await
        .promote_part(album.id, piece.id)
        .await
        .expect("promote it");

    assert_eq!(song.parent_id, Some(album.id));
    // Siblings on disk, parent and child in the model.
    let album_dir = std::path::Path::new(&album.path).parent();
    let song_dir = std::path::Path::new(&song.path).parent();
    assert_eq!(
        album_dir, song_dir,
        "this test assumes both land under Projects/; if that changed, \
         the assertion below is the one that still matters"
    );
    assert!(
        !song.path.contains(
            std::path::Path::new(&album.path)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("album")
        ),
        "the child's path is nested under the parent's, which would make \
         containment look like hierarchy: {}",
        song.path
    );
}

/// Every markdown file under `root`, as paths.
fn walkdir(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            out.extend(walkdir(&path));
        } else {
            out.push(path.display().to_string());
        }
    }
    out
}
