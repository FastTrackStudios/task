//! Chapter twenty-four — the band takes it on tour, and a tree becomes
//! a project.
//!
//! `project.setlist.source` and `project.identity.adoption` — the two
//! remaining rules that are about a project's relationship to things
//! that already exist: songs somebody else owns, and a directory
//! somebody else made.
//!
//! # By reference is the whole of the setlist rule
//!
//! "A setlist is assembled by reference rather than by copying.
//! Reordering or re-scoping a setlist changes no project, and a song may
//! appear in any number of setlists." Every clause there is a statement
//! about what a setlist does *not* own, so the tests are mostly about
//! the album being untouched.

use project::Form;

use integration::scenario::Scenario;

fn draft(title: &str, form: Option<Form>) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        form,
        ..Default::default()
    }
}

// t[verify project.setlist.source]
// t[verify scenario.album.setlist]
/// A setlist references the album's songs, promoted and not alike.
#[tokio::test]
async fn a_setlist_is_assembled_by_reference() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    let album = projects
        .create(draft("Crescendum", Some(Form::Album)))
        .await
        .expect("the album");
    let mut songs = Vec::new();
    for name in ["Overture", "Daybreak", "Finale"] {
        songs.push(
            projects
                .add_part(album.id, name.into())
                .await
                .expect("name a song"),
        );
    }
    // One of them is promoted, and the setlist must not care.
    projects
        .promote_part(album.id, songs[1].id)
        .await
        .expect("promote");

    let set = projects
        .create(draft("Spring tour", Some(Form::LiveSet)))
        .await
        .expect("the live set");
    projects
        .set_setlist(set.id, vec![songs[2].id, songs[0].id, songs[1].id])
        .await
        .expect("assemble the setlist");

    let performed: Vec<String> = projects
        .setlist(set.id)
        .await
        .expect("read it back")
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(
        performed,
        ["Finale", "Overture", "Daybreak"],
        "the setlist's order is the setlist's, not the album's"
    );

    // And the album is exactly as it was — same order, same pieces.
    let album_order: Vec<String> = projects
        .pieces(album.id)
        .await
        .expect("the album's pieces")
        .into_iter()
        .map(|p| p.name)
        .collect();
    assert_eq!(
        album_order,
        ["Overture", "Daybreak", "Finale"],
        "assembling a setlist reordered the album"
    );
}

// t[verify project.setlist.source]
/// A song plays in two shows, and neither owns it.
#[tokio::test]
async fn one_song_appears_in_any_number_of_setlists() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    let album = projects
        .create(draft("Crescendum", Some(Form::Album)))
        .await
        .expect("the album");
    let song = projects
        .add_part(album.id, "Overture".into())
        .await
        .expect("name a song");

    for show in ["Spring tour", "Album release show"] {
        let set = projects
            .create(draft(show, Some(Form::LiveSet)))
            .await
            .expect("a show");
        projects
            .set_setlist(set.id, vec![song.id])
            .await
            .unwrap_or_else(|e| panic!("{show}: {e:?}"));
    }

    // The song is still one song, in one album.
    let pieces = projects.pieces(album.id).await.expect("the album");
    assert_eq!(pieces.len(), 1);
    assert_eq!(pieces[0].name, "Overture");
}

// t[verify project.identity.adoption]
// t[verify scenario.album.adopt]
/// A directory that already exists becomes a project without moving.
///
/// The premise the whole adoption story rests on: the tree was written
/// by other applications and they keep writing it. So the assertion is
/// that everything that was there is still there, in the same place,
/// with one page added.
#[tokio::test]
async fn an_existing_directory_becomes_a_project_in_place() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    // A tree somebody else made, in the org's vault.
    let vault = s.orgs.acme.org_root().join("vault");
    let dir = vault.join("Recordings/Journey");
    std::fs::create_dir_all(dir.join("Takes")).expect("mkdir");
    std::fs::write(dir.join("Notes.md"), "# Journey\n\nrecorded in one pass\n")
        .expect("a note somebody wrote");
    std::fs::write(dir.join("Takes/take-one.wav"), b"audio").expect("a take");

    let before = tree(&dir);

    let adopted = projects
        .adopt("Recordings/Journey".into(), String::new())
        .await
        .expect("adopt the tree");
    assert_eq!(
        adopted.title, "Journey",
        "with no title given, the directory's own name is what a person called it"
    );

    // Nothing moved, nothing was renamed, nothing was copied away.
    let after = tree(&dir);
    for path in &before {
        assert!(
            after.contains(path),
            "adoption disturbed {path}\nbefore: {before:#?}\nafter: {after:#?}"
        );
    }
    assert_eq!(
        after.len(),
        before.len() + 1,
        "adoption should add exactly one page: {after:#?}"
    );

    // And it resolves as a project.
    let fetched = projects.get(adopted.id).await.expect("resolves");
    assert_eq!(fetched.title, "Journey");
}

// t[verify project.identity.adoption]
/// Adopting twice is adopting once.
#[tokio::test]
async fn adopting_a_tree_that_is_already_a_project_returns_it() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    let vault = s.orgs.acme.org_root().join("vault");
    std::fs::create_dir_all(vault.join("Recordings/Journey")).expect("mkdir");

    let first = projects
        .adopt("Recordings/Journey".into(), "Journey".into())
        .await
        .expect("adopt");
    let again = projects
        .adopt("Recordings/Journey".into(), "Journey".into())
        .await
        .expect("adopting twice must not refuse, and must not duplicate");

    assert_eq!(
        first.id, again.id,
        "a second adoption produced a second project over one tree"
    );
}

/// Every file under `root`, relative, sorted.
fn tree(root: &std::path::Path) -> Vec<String> {
    let mut out = Vec::new();
    walk(root, root, &mut out);
    out.sort();
    out
}

fn walk(base: &std::path::Path, at: &std::path::Path, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(at) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(base, &path, out);
        } else if let Ok(rel) = path.strip_prefix(base) {
            out.push(rel.display().to_string());
        }
    }
}
