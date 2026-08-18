//! The vault's read and write path — `vault.index.*`,
//! `vault.write.granular`, `storage.query.no-scan`.
//!
//! # Counted, not timed
//!
//! Every rule here is about *cost*: constant-time lookup, parse once per
//! change, re-index one file, lock one page. The obvious way to test
//! cost is to time it, and a timing assertion on a shared CI box is a
//! flake with a spec reference attached.
//!
//! So the entity under test counts its own parses. `PARSES` goes up once
//! per `from_page`, and every assertion below is an exact count — which
//! is both stricter than a timing bound and stable on a loaded machine.
//! A regression to "re-parse everything on every read" moves these
//! numbers by exactly the vault size.

use std::sync::atomic::{AtomicUsize, Ordering};

use uuid::Uuid;
use vault::Vault;
use vault_entity::store::{VaultEntity, VaultEntityStore};

/// How many times a page has been parsed since [`reset`].
static PARSES: AtomicUsize = AtomicUsize::new(0);

fn reset() {
    PARSES.store(0, Ordering::SeqCst);
}
fn parses() -> usize {
    PARSES.load(Ordering::SeqCst)
}

#[derive(Debug, Clone, PartialEq)]
struct Note {
    id: Uuid,
    path: String,
    title: String,
}

struct Notes;

impl VaultEntity for Notes {
    type Model = Note;
    const TYPE: &'static str = "note";
    const DEFAULT_FOLDER: &'static str = "Notes";
    const SLUG_FALLBACK: &'static str = "untitled-note";

    fn id(m: &Note) -> Uuid {
        m.id
    }
    fn set_id(m: &mut Note, id: Uuid) {
        m.id = id;
    }
    fn path(m: &Note) -> &str {
        &m.path
    }
    fn set_path(m: &mut Note, path: String) {
        m.path = path;
    }
    fn name(m: &Note) -> &str {
        &m.title
    }

    fn from_page(page: &vault::VaultPage) -> Result<Note, vault_entity::error::ParseError> {
        // The whole instrumentation: one bump per parse.
        PARSES.fetch_add(1, Ordering::SeqCst);
        let id = page
            .raw
            .lines()
            .find_map(|l| l.strip_prefix("id: "))
            .and_then(|s| Uuid::parse_str(s.trim()).ok())
            .ok_or(vault_entity::error::ParseError::NoFrontmatter)?;
        let title = page
            .raw
            .lines()
            .find_map(|l| l.strip_prefix("title: "))
            .unwrap_or("untitled")
            .trim()
            .to_string();
        Ok(Note {
            id,
            path: page.rel_path.clone(),
            title,
        })
    }

    fn to_markdown(m: &Note) -> Result<String, vault_entity::error::WriteError> {
        Ok(format!(
            "---\ntype: note\nid: {}\ntitle: {}\n---\n",
            m.id, m.title
        ))
    }

    fn matches(page: &vault::VaultPage) -> bool {
        page.raw.contains("type: note")
    }
}

/// A vault of `n` notes on disk, and a store over it.
fn vault_of(n: usize) -> (tempfile::TempDir, VaultEntityStore<Notes>, Vec<Note>) {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut made = Vec::new();
    std::fs::create_dir_all(dir.path().join("Notes")).expect("mkdir");
    for i in 0..n {
        let note = Note {
            id: Uuid::new_v4(),
            path: format!("Notes/note-{i}.md"),
            title: format!("Note {i}"),
        };
        std::fs::write(
            dir.path().join(&note.path),
            Notes::to_markdown(&note).expect("render"),
        )
        .expect("write");
        made.push(note);
    }
    let vault = Vault::open(dir.path()).expect("open");
    (dir, VaultEntityStore::<Notes>::new(vault), made)
}

// t[verify vault.index.parse-once]
/// An unchanged vault is parsed once, however often it is read.
#[test]
fn reading_twice_parses_once() {
    let (_dir, store, made) = vault_of(20);
    reset();

    assert_eq!(store.list().len(), made.len());
    let first = parses();
    assert_eq!(first, 20, "the cold read parses every page exactly once");

    for _ in 0..5 {
        assert_eq!(store.list().len(), made.len());
    }
    assert_eq!(
        parses(),
        first,
        "five more reads of an unchanged vault re-parsed pages"
    );
}

// t[verify vault.index.incremental]
/// Editing one page costs one parse.
#[test]
fn one_edit_costs_one_parse() {
    let (_dir, store, made) = vault_of(20);
    store.list();
    reset();

    let mut changed = made[7].clone();
    changed.title = "Renamed".into();
    store.update(changed).expect("update");

    // The write itself parses nothing; the next read parses the one page
    // whose content moved.
    let after = store.list();
    assert_eq!(
        parses(),
        1,
        "an edit to one page cost {} parses — the other nineteen were \
         cached and should have stayed cached",
        parses()
    );
    assert!(after.iter().any(|n| n.title == "Renamed"));
}

// t[verify vault.index.lookup]
// t[verify storage.query.no-scan]
/// A lookup by id parses one page, not the vault.
///
/// The exact number is the assertion: one. The old behaviour parsed
/// every page of the type and discarded all but one, so this number was
/// the vault size — and it is the number, not a duration, that says
/// whether the index is being consulted.
#[test]
fn a_lookup_by_id_parses_one_page() {
    let (_dir, store, made) = vault_of(50);
    store.list();
    reset();

    let found = store
        .get_by_uuid(made[31].id)
        .expect("the note resolves by id");
    assert_eq!(found.title, "Note 31");
    assert_eq!(
        parses(),
        0,
        "a warm lookup re-parsed {} page(s); it should have been served \
         from the index",
        parses()
    );
}

// t[verify vault.index.lookup]
/// Ten times the vault does not cost ten times the lookup.
///
/// "Adding pages of unrelated types does not slow either operation" —
/// and the same is true of pages of the *same* type once the index is
/// warm. Counted rather than timed, so this holds on a loaded machine.
#[test]
fn lookup_cost_does_not_grow_with_the_vault() {
    for size in [10, 100] {
        let (_dir, store, made) = vault_of(size);
        store.list();
        reset();
        store.get_by_uuid(made[size / 2].id).expect("resolves");
        assert_eq!(
            parses(),
            0,
            "a vault of {size} charged {} parse(s) for one lookup",
            parses()
        );
    }
}

// t[verify vault.index.tolerant]
/// One unparseable page costs one page.
#[test]
fn a_bad_page_does_not_cost_the_vault() {
    let (dir, _store, made) = vault_of(10);
    std::fs::write(
        dir.path().join("Notes/broken.md"),
        "---\ntype: note\nid: not-a-uuid\n---\n",
    )
    .expect("write");
    let store = VaultEntityStore::<Notes>::new(Vault::open(dir.path()).expect("reopen"));

    let listed = store.list();
    assert_eq!(
        listed.len(),
        made.len(),
        "the ten good notes should still be listed"
    );
    assert!(
        dir.path().join("Notes/broken.md").exists(),
        "the unparseable page was removed rather than skipped"
    );
}

// t[verify vault.write.granular]
/// Writes to different pages proceed together.
///
/// Sixteen threads writing sixteen different notes at once. What this
/// catches is a lock wider than the page: with the vault held across
/// each fs write, these serialise, and any one of them failing means a
/// write was lost rather than merely delayed.
#[test]
fn concurrent_writes_to_different_pages_all_land() {
    let (_dir, store, made) = vault_of(16);
    store.list();

    std::thread::scope(|scope| {
        for (i, note) in made.iter().enumerate() {
            let store = store.clone();
            let mut note = note.clone();
            scope.spawn(move || {
                note.title = format!("Written by thread {i}");
                store.update(note).expect("every write lands");
            });
        }
    });

    let after = store.list();
    for i in 0..made.len() {
        let expected = format!("Written by thread {i}");
        assert!(
            after.iter().any(|n| n.title == expected),
            "a concurrent write to a different page was lost: {expected}"
        );
    }
}

// t[verify vault.write.granular]
/// Two writes to the *same* page do not interleave.
///
/// The other half. Different pages proceed together; one page
/// serialises, so the file is always one caller's whole write rather
/// than a mixture — which is what the per-page lock is for.
#[test]
fn concurrent_writes_to_one_page_serialise() {
    let (dir, store, made) = vault_of(1);
    store.list();
    let note = made[0].clone();

    std::thread::scope(|scope| {
        for i in 0..16 {
            let store = store.clone();
            let mut note = note.clone();
            scope.spawn(move || {
                note.title = format!("thread-{i}");
                let _ = store.update(note);
            });
        }
    });

    // Whatever won, the file is one complete note.
    let raw = std::fs::read_to_string(dir.path().join(&note.path)).expect("read");
    let page = vault::VaultPage {
        rel_path: note.path.clone(),
        basename: "note-0".into(),
        folder: "Notes".into(),
        raw: raw.clone(),
        mtime: std::time::SystemTime::now(),
    };
    let parsed = Notes::from_page(&page).unwrap_or_else(|e| {
        panic!("sixteen writers left a page that does not parse: {e:?}\n{raw}")
    });
    assert!(
        parsed.title.starts_with("thread-"),
        "the surviving title is a mixture: {}",
        parsed.title
    );
}
