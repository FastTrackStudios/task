//! Chapter twenty-seven — the vault is a root.
//!
//! `project.vault.write-path`: "the production vault becomes a File Root
//! and all writes to it go through the Files API, markdown included. Its
//! live tree stays ordinary files on disk — greppable, `cp -r`-able,
//! editable by any other tool — so migration changes the write path and
//! not the on-disk result."
//!
//! Three claims, each its own test, because each fails differently:
//!
//! - the vault *is* a root — from boot, by nobody's action, and the same
//!   root after a restart;
//! - a page write *is* a Files write — the catalogue hears about it as a
//!   delta, the way it hears about a `create_dirs`, without anyone
//!   re-listing the tree;
//! - and the on-disk result did not change — the page is the same
//!   markdown `storage.rs` reads without this software, at the same
//!   path, with nothing left beside it.
//!
//! The last one is the half of the rule that is easy to lose. A migration
//! that routed writes through Files and left the vault as a blob store
//! would pass the first two.

use files::path::RootPath;
use files::service::AccessService;
use files::service::tree::EntryKind;
use task_server::example_org::Holds;

use integration::scenario::Scenario;

fn draft(title: &str) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        ..Default::default()
    }
}

fn p(s: &str) -> RootPath {
    RootPath::parse(s).expect("a path")
}

// t[verify project.vault.write-path]
/// The vault is a root from boot, and the same root after a restart.
///
/// Registered by the server as it comes up, not by a person — there is
/// no "make my vault a root" step anywhere in the surface. And the id is
/// stable across a restart, because the marker and the store travel with
/// the directory: a restart re-binds the root it finds, it does not mint
/// another.
#[tokio::test]
async fn the_vault_is_a_root_from_boot_and_stays_one() {
    let s = Scenario::open().await;
    let vault_dir = s
        .orgs
        .acme
        .org_root()
        .join("vault")
        .canonicalize()
        .expect("the vault exists");

    let listed = s
        .as_alice()
        .await
        .roots()
        .await
        .list()
        .await
        .expect("list roots");
    let vault = listed
        .iter()
        .find(|r| r.local_tree().is_some_and(|t| t == vault_dir))
        .unwrap_or_else(|| {
            panic!(
                "the vault at {} is not a root: {listed:?}",
                vault_dir.display()
            )
        });
    assert_eq!(
        s.orgs.acme.backend.vault_root_id().map(|id| id.get()),
        Some(vault.id),
        "the backend does not know its own vault's root"
    );

    let before = vault.id;
    let acme = s.orgs.acme.restart().await;
    assert_eq!(
        acme.backend.vault_root_id().map(|id| id.get()),
        Some(before),
        "a restart gave the vault a different root id"
    );
}

// t[verify project.vault.write-path]
/// A page write reaches the catalogue as a delta, like any Files write.
///
/// The catalogue is loaded *before* the write, which is what makes the
/// claim about the delta rather than about the walk: a catalogue built
/// afterwards would find the file by listing, and a write that bypassed
/// Files would pass. `adoption.rs` makes the same distinction for
/// `create_dirs`.
#[tokio::test]
async fn a_page_write_is_a_files_write() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let vault = s
        .orgs
        .acme
        .backend
        .vault_root_id()
        .expect("the vault is a root");

    // The vault root has no grants of its own — the pages on it are
    // written by the server, not by a caller — so to *browse* it through
    // the tree lane Alice is granted it here, the way the harness grants
    // her the session root.
    s.orgs
        .acme
        .backend
        .grant(
            s.people.alice.subject.clone(),
            vault,
            RootPath::root(),
            Holds::Owner.capabilities(),
        )
        .await
        .expect("grant Alice the vault");

    // Load the catalogue first.
    alice
        .tree()
        .await
        .browse(vault, RootPath::root())
        .await
        .expect("browse the vault");

    let made = alice
        .projects()
        .await
        .create(draft("Crescendum"))
        .await
        .expect("create");

    let entry = alice
        .tree()
        .await
        .entry(vault, p(&made.path))
        .await
        .expect("the catalogue did not hear about the page");
    assert_eq!(entry.kind, EntryKind::File);
    let on_disk = std::fs::metadata(s.orgs.acme.org_root().join("vault").join(&made.path))
        .expect("the page is a file")
        .len();
    assert_eq!(
        entry.size, on_disk,
        "the catalogue's size is not the file's"
    );

    // And a delete is heard the same way.
    alice
        .projects()
        .await
        .delete(made.id)
        .await
        .expect("delete");
    assert!(
        alice
            .tree()
            .await
            .entry(vault, p(&made.path))
            .await
            .is_err(),
        "the catalogue still lists a deleted page"
    );
}

// t[verify project.vault.write-path]
// t[verify storage.tier.authored]
/// The on-disk result did not change.
///
/// Same path, same markdown, nothing left beside it — not a temp file,
/// not a sidecar. `storage.rs` proves the page reads without this
/// software; this proves routing the write through Files did not cost
/// that. Rewritten several times, because the atomic write leaves its
/// temp file in the same directory and a leak would show up as a
/// sibling.
#[tokio::test]
async fn the_page_on_disk_is_the_same_markdown_it_always_was() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    let made = alice
        .projects()
        .await
        .create(draft("Crescendum"))
        .await
        .expect("create");
    for n in 0..5 {
        let mut next = alice.projects().await.get(made.id).await.expect("read");
        next.details = format!("revision {n}");
        alice.projects().await.update(next).await.expect("write");
    }

    let page = s.orgs.acme.org_root().join("vault").join(&made.path);
    let text = std::fs::read_to_string(&page).expect("the page is a file");
    assert!(text.starts_with("---\n"), "not frontmatter:\n{text}");
    assert!(
        text.contains("revision 4"),
        "the last write is not in the file:\n{text}"
    );

    let dir = page.parent().expect("a parent");
    let siblings: Vec<String> = std::fs::read_dir(dir)
        .expect("list the folder")
        .filter_map(Result::ok)
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.contains(".tmp"))
        .collect();
    assert!(
        siblings.is_empty(),
        "temp files left beside the page: {siblings:?}"
    );
}
