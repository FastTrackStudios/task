#![allow(clippy::large_futures)]
//! Chapter — **a project is a directory, and a sub-project is a shelf
//! inside it.**
//!
//! ADR 0004 declares four roots and this is the fourth. Three sentences
//! it makes true, each of which was false one commit ago:
//!
//! > `<org>/projects/<slug>/` **is** the project. Its `project.md`, its
//! > sessions, its sub-projects, its deliverables. `cp -r` of that
//! > directory is a copy of the project.
//!
//! > A sub-project is **its own shelf**, nested — a git submodule. The
//! > parent holds a reference to it rather than swallowing it.
//!
//! > A project is **referenced as needed**. It left the vault, so a task
//! > or a note or another org naming it has to go on resolving.
//!
//! `song_library.rs` is the sibling chapter: an asset group is a shelf
//! and a shelf is subscribable. This one asks the harder version of the
//! same question, because a project **contains** shelves, and every
//! interesting property here follows from that.
//!
//! # Why the prune is the thing being tested, not an implementation
//! detail
//!
//! Two shelves nested on disk must not overlap **as walks**. A shelf is
//! registered on `vault::Backend`, `GraphBackend` and `VaultCollab`; if
//! the album's walk reached into the song, every file under the song
//! would be in two roots at once — two link graphs claiming it, two
//! watchers announcing each write, and two CRDT document ids over one
//! file. Two people editing that file would then converge on two
//! different documents and clobber each other, silently.
//!
//! That is not a failure a functional test notices. Every RPC would
//! answer correctly; the manifest is where it shows. So the test below
//! reads the album's manifest **over the wire** and asserts the song's
//! files are absent from it — which is the same shape
//! `files::scan::walk_live_tree` has asserted one layer down since File
//! Roots learned to nest, and its doc says the allow and the prune must
//! stay together. Now both layers say it.
//!
//! # Surface-only, and why that is the safe default
//!
//! A subscription carries a `Depth` beside its `Selection`, and the
//! default is `Surface`: taking the album is not taking its songs.
//! A deep default would pull an unbounded amount of somebody else's
//! disk on the strength of a request that never mentioned it — an album
//! with fifteen promoted songs is fifteen shelves of multitracks — and
//! the failure would be silent, expensive and remote.
//!
//! Surface-only is not a loss. The sub-project is still *there*: a
//! named reference that does not resolve, which ADR 0004 makes the
//! ordinary state of anything not resident locally rather than an
//! error. A person who sees it can ask for it; a person whose laptop
//! silently filled cannot un-ask.

use links_proto::{NodeKind, NodeRef, Reach};
use org_proto::{Depth, Selection};
use wiki_proto::subscription::{SourceKind, Subscriber, Subscription};

use integration::client::Session;
use integration::scenario::Scenario;

/// The guest musician's own organisation — the example's personal org,
/// publishing under `alice.test`. The same one `song_library.rs` uses,
/// so the two chapters describe one world rather than two.
const GUEST_ORG: &str = "alice-personal";
const GUEST_DOMAIN: &str = "alice.test";

fn draft(title: &str) -> project::ProjectInfo {
    project::ProjectInfo {
        title: title.into(),
        ..Default::default()
    }
}

fn source(slug: &str) -> Subscription {
    Subscription {
        domain: GUEST_DOMAIN.into(),
        slug: slug.into(),
        kind: SourceKind::Projects,
        title: slug.into(),
        core: false,
        declined: false,
        selection: Selection::All,
    }
}

async fn resolve(who: &Session, node: NodeRef) -> links_proto::ResolvedNode {
    who.links()
        .await
        .resolve_nodes(vec![node])
        .await
        .expect("resolve")
        .pop()
        .expect("one answer per node")
}

/// t[verify project.identity.declaration]
/// t[verify project.identity.stable]
///
/// A project is a directory with its page inside it, and `cp -r` of that
/// directory is a copy of the project.
///
/// The last clause is the one worth having a test for. It is what
/// `project.identity.stable` promised — *"a project carried to another
/// machine by `cp -r` arrives intact"* — and what the old layout could
/// only half deliver, because half the project was a note in a vault
/// somewhere else. So this copies the directory and nothing else, opens
/// the copy with nothing but a filesystem, and reads the project back.
#[tokio::test]
async fn a_project_is_one_directory_and_copying_it_copies_the_project() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let tier = s.orgs.acme.org_root().join("projects");

    let album = alice
        .projects()
        .await
        .create(draft("Crescendum"))
        .await
        .expect("create the album");

    // The page is inside the project's own directory.
    assert_eq!(album.path, "crescendum/project.md");
    let dir = tier.join("crescendum");
    assert!(
        dir.join("project.md").is_file(),
        "the declaration is not in the directory it declares"
    );

    // Material the tools that made it would write, beside the page.
    std::fs::create_dir_all(dir.join("Deliverables")).expect("mkdir");
    std::fs::write(dir.join("Deliverables/master.wav"), b"audio").expect("a master");

    // `cp -r`, by hand, and nothing else. No vault, no database, no
    // second directory to remember.
    let elsewhere = s.orgs.acme.org_root().join("carried-away");
    copy_tree(&dir, &elsewhere);

    // What arrived is a project, readable with a filesystem and a
    // markdown parser — `storage.tier.authored`'s actual test.
    let carried = std::fs::read_to_string(elsewhere.join("project.md")).expect("the page came too");
    assert!(carried.contains("type: project"), "{carried:.200}");
    assert!(carried.contains("Crescendum"), "{carried:.200}");
    assert!(
        carried.contains(&album.id.to_string()),
        "the id did not travel, so nothing that pointed at this project \
         still points at it: {carried:.200}"
    );
    assert!(
        elsewhere.join("Deliverables/master.wav").is_file(),
        "the work did not travel with the declaration"
    );
}

/// t[verify project.nesting.uniform]
/// t[verify project.part.promotion]
///
/// A promoted song is a project inside a project, and the parent's shelf
/// does not swallow it.
///
/// Two halves, and the second is the one that cannot be seen from the
/// RPC surface. The first is the model: promotion sets `parent_id`, the
/// child's files land inside the parent's directory, and the album's
/// roster is untouched. The second is the **manifest** — what the
/// parent's shelf claims as its own files — and it is where a missing
/// prune would show up as two roots over one file.
#[tokio::test]
async fn a_subproject_is_a_shelf_the_parent_does_not_swallow() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;
    let projects = alice.projects().await;

    let album = projects
        .create(draft("Crescendum"))
        .await
        .expect("create the album");
    let piece = projects
        .add_part(album.id, "Track Two".into())
        .await
        .expect("name a song");

    // Before promotion the song is a roster entry and nothing else —
    // `project.part.unit`, a part costs no page.
    let tier = s.orgs.acme.org_root().join("projects");
    assert!(!tier.join("crescendum/track-two/project.md").exists());

    // Material the song accumulated, which is what earns it a project.
    std::fs::create_dir_all(tier.join("crescendum/track-two")).expect("mkdir");
    std::fs::write(tier.join("crescendum/track-two/take-one.wav"), b"audio").expect("a take");

    let song = projects
        .promote_part(album.id, piece.id)
        .await
        .expect("promote it");

    // The model: same id, declared parent, page inside the parent's
    // directory. The nesting is where the files are; the parentage is
    // the field — `project.nesting.explicit`, and `parts.rs` proves the
    // two are not read off each other.
    assert_eq!(song.id, piece.id, "promotion minted a new id");
    assert_eq!(song.parent_id, Some(album.id));
    assert_eq!(song.path, "crescendum/track-two/project.md");

    // The roster is untouched — `project.part.listing`. The album has
    // one track before and after.
    let parts = projects.parts(album.id).await.expect("the roster");
    assert_eq!(
        parts.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
        ["Track Two"],
        "promotion edited the parent's roster; it is supposed to add a \
         page and leave the parent alone"
    );

    // ── the prune, over the wire ─────────────────────────────────────
    //
    // The album's shelf is registered under `project:crescendum` and the
    // song's under `project:crescendum/track-two`. Ask the album's shelf
    // what files it holds. The song's are not among them, and if they
    // ever are, two roots hold one file and two CRDT documents will be
    // opened over it.
    let vault = alice.vault().await;
    let album_manifest = vault
        .manifest("project:crescendum".into())
        .await
        .expect("the album's shelf is registered");
    let album_paths: Vec<&str> = album_manifest
        .files
        .iter()
        .map(|e| e.path.as_str())
        .collect();
    assert!(
        album_paths.contains(&"project.md"),
        "the album's shelf does not hold its own page: {album_paths:?}"
    );
    assert!(
        album_paths.iter().all(|p| !p.starts_with("track-two/")),
        "the parent's walk reached into the sub-project's shelf — every \
         file under it is now in two roots, two link graphs and two CRDT \
         documents at once: {album_paths:?}"
    );

    // And the song's own shelf holds exactly what the album's does not.
    let song_manifest = vault
        .manifest("project:crescendum/track-two".into())
        .await
        .expect("a sub-project is a shelf of its own");
    let song_paths: Vec<&str> = song_manifest
        .files
        .iter()
        .map(|e| e.path.as_str())
        .collect();
    assert!(
        song_paths.contains(&"project.md") && song_paths.contains(&"take-one.wav"),
        "the sub-project's shelf is missing its own files: {song_paths:?}"
    );
}

/// t[verify project.location.federated]
///
/// One org subscribes to another's project. The project resolves; its
/// sub-project stays a reference.
///
/// This is "referenced as needed" as a mechanism rather than a
/// sentence. Before `NodeKind::Project` existed, `project:` did not even
/// parse into a `NodeRef` — a project was reachable only as
/// `note:Projects/X.md`, which stopped being true the moment a project
/// left the vault.
///
/// The second half is `Depth::Surface`, the default: taking the album is
/// not taking the song. The song is not hidden — it is addressable, and
/// the refusal says so — it is simply not yours until you ask.
#[tokio::test]
async fn a_subscribed_project_resolves_and_its_subproject_stays_a_reference() {
    let s = Scenario::open().await;
    let alice = s.as_alice().await;

    // ── the guest org, on the same disk ──────────────────────────────
    let guest = s
        .orgs
        .acme
        .start_beside("Alice Personal", GUEST_ORG, |_| {})
        .await;
    let owner = integration::people::account(&guest, "alice@alice.test", "Alice").await;
    let guest_session = Session::open(&guest, owner.token.clone()).await;

    // ── the guest makes a project with a promoted song in it ─────────
    let guest_projects = guest_session.projects().await;
    let album = guest_projects
        .create(draft("Crescendum"))
        .await
        .expect("the guest's album");
    let piece = guest_projects
        .add_part(album.id, "Track Two".into())
        .await
        .expect("name a song");
    guest_projects
        .promote_part(album.id, piece.id)
        .await
        .expect("promote it");

    let album_ref = NodeRef::new(NodeKind::Project, "crescendum").in_domain(GUEST_DOMAIN);
    let song_ref = NodeRef::new(NodeKind::Project, "crescendum/track-two").in_domain(GUEST_DOMAIN);

    // ── before subscribing: addressable, and not readable ────────────
    //
    // Writing somebody's domain into a reference grants nothing. That is
    // the whole safety of the qualified form, and it is asserted here
    // rather than assumed because this chapter is where a new kind was
    // added to the resolver.
    assert_eq!(
        resolve(&alice, album_ref.clone()).await.reach,
        Reach::NotPermitted,
        "naming a project granted a read of it"
    );

    // ── the subscription ─────────────────────────────────────────────
    //
    // The slug is the project's own path, not a library holding many —
    // a project IS the unit somebody publishes. A mix engineer is given
    // a song, not "the projects library".
    let subs = alice.wiki_subscriptions().await;
    subs.subscribe(Subscriber::Vault, source("crescendum"))
        .await
        .expect("a project is a shelf, and a shelf is subscribable");

    let resolved = resolve(&alice, album_ref).await;
    assert_eq!(resolved.reach, Reach::Reachable);
    assert_eq!(resolved.org, GUEST_ORG);
    assert_eq!(
        resolved.rel_path, "projects/crescendum/project.md",
        "the answer names the publisher's own path, as every other kind's does"
    );

    // ── and the sub-project stays a reference ────────────────────────
    //
    // `Depth::Surface`. The song is named, addressable and unresolved —
    // the state ADR 0004 calls ordinary rather than an error. Taking the
    // album did not quietly take fifteen shelves of multitracks with it.
    assert_eq!(
        resolve(&alice, song_ref.clone()).await.reach,
        Reach::NotPermitted,
        "subscribing to a project pulled in its sub-projects — the depth \
         default is Surface precisely so it cannot"
    );
    assert!(
        !Depth::default().reaches_nested(),
        "the default stopped being the shallow one, which is the \
         assumption the assertion above rests on"
    );

    // Asking for the song is a second subscription, by its own path.
    subs.subscribe(Subscriber::Vault, source("crescendum/track-two"))
        .await
        .expect("a sub-project is subscribable on its own terms");
    assert_eq!(
        resolve(&alice, song_ref).await.reach,
        Reach::Reachable,
        "a sub-project asked for by name did not become reachable"
    );
}

/// `cp -r`, without shelling out.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).expect("mkdir");
    for entry in std::fs::read_dir(from).expect("read").flatten() {
        let path = entry.path();
        let dest = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &dest);
        } else {
            std::fs::copy(&path, &dest).expect("copy");
        }
    }
}
