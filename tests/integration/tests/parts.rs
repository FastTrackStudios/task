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
        id: uuid::Uuid::nil(),
        path: String::new(),
        title: title.into(),
        status: "active".into(),
        priority: "normal".into(),
        project_type: String::new(),
        lead: String::new(),
        tags: project::model::Tags::default(),
        parts: project::Parts::default(),
        capabilities: project::Capabilities::default(),
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
