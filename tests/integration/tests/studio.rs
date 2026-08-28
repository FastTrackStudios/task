//! Chapter eleven — a studio's disk, read as a tree.
//!
//! Everything before this built a world and then asked questions of it.
//! This one starts from a disk that already exists, organised by a person
//! years before any of this software read it, and asks whether the model
//! can say what is on it.
//!
//! The tree it reads is committed at `examples/studio`. Its
//! names are invented so a failure names the case; its *shapes* are all
//! ones a real 6 TB archive produced, and every awkward one was copied
//! from there after breaking the first reader. `archive.rs` runs the same
//! reader against that archive when a machine has it.

use files_domain::collaboration::Collaboration;
use files_domain::layout::{Entry, InboxScope, NotAProject};
use files_domain::peering::OrgId;

use integration::archive::{self, Read};

fn data() -> Vec<Read> {
    archive::read_orgs(&archive::example_data(), 5)
}

fn at(reads: &[Read], path: &str) -> Entry {
    reads
        .iter()
        .find(|r| r.path() == path)
        .unwrap_or_else(|| panic!("{path} is not in the example data"))
        .entry
        .clone()
}

/// The model explains every directory in the example data.
///
/// `Unknown` is a legitimate answer for an archive nobody curated — but
/// not here. This tree exists to be understood, so anything the model
/// shrugs at is either a case worth handling or a folder worth removing.
#[tokio::test]
async fn nothing_in_the_example_data_is_unexplained() {
    let unexplained: Vec<String> = data()
        .iter()
        .filter(|r| r.entry == Entry::Unknown)
        .map(Read::path)
        .collect();
    assert!(
        unexplained.is_empty(),
        "the model has nothing to say about: {unexplained:#?}"
    );
}

/// An org's directory says where files are, not whose they are.
#[tokio::test]
async fn the_org_directory_is_a_placement() {
    let v = data();
    assert_eq!(at(&v, "acme-audio"), Entry::Org("acme-audio".into()));
    assert_eq!(at(&v, "vnt-video"), Entry::Org("vnt-video".into()));
}

/// An org has more than projects, and each part is recognised.
///
/// The vault and the wiki are separate because the two age differently: a
/// session note is true of one week, and a page on mic placement is meant
/// to outlast every project that used it. Assets are a third kind —
/// content the org owns that belongs to no project at all.
#[tokio::test]
async fn an_org_holds_more_than_its_projects() {
    let d = data();
    assert_eq!(
        at(&d, "acme-audio/Vault"),
        Entry::Vault("acme-audio".into())
    );
    assert_eq!(at(&d, "acme-audio/Wiki"), Entry::Wiki("acme-audio".into()));
    assert_eq!(
        at(&d, "acme-audio/Assets"),
        Entry::Assets("acme-audio".into())
    );
    assert_eq!(
        at(&d, "acme-audio/Projects"),
        Entry::Projects("acme-audio".into())
    );

    // And the second org has its own of each — none of this is shared.
    assert_eq!(at(&d, "vnt-video/Vault"), Entry::Vault("vnt-video".into()));
    assert_eq!(at(&d, "vnt-video/Wiki"), Entry::Wiki("vnt-video".into()));
}

/// The ordinary project: work, client, one session.
#[tokio::test]
async fn a_folder_name_carries_the_work_and_the_client() {
    let Entry::Project(p) = at(&data(), "acme-audio/Projects/First Single - Example Client") else {
        panic!("not read as a project");
    };
    assert_eq!(p.work, "First Single");
    assert_eq!(p.clients, ["Example Client"]);
    assert_eq!(p.org, "acme-audio");
}

/// A documentary is for two people, and the folder is where that was
/// written down.
#[tokio::test]
async fn several_clients_come_off_one_folder_name() {
    let Entry::Project(p) = at(
        &data(),
        "vnt-video/Projects/Example Documentary - First Client, Second Client",
    ) else {
        panic!("not read as a project");
    };
    assert_eq!(p.work, "Example Documentary");
    assert_eq!(p.clients, ["First Client", "Second Client"]);
}

/// Housekeeping, machine storage and the material inside a project are
/// each recognised for what they are rather than mistaken for projects.
#[tokio::test]
async fn what_is_not_a_project_is_not_read_as_one() {
    let v = data();

    assert_eq!(
        at(&v, "acme-audio/Projects/Z - Duplicates"),
        Entry::NotAProject {
            org: "acme-audio".into(),
            why: NotAProject::Housekeeping,
        },
        "a name containing ` - ` was split into work and client"
    );
    assert_eq!(
        at(&v, "acme-audio/Projects/tasks"),
        Entry::NotAProject {
            org: "acme-audio".into(),
            why: NotAProject::MachineOwned,
        }
    );
    // Including the one whose name also contains the separator.
    assert_eq!(
        at(
            &v,
            "vnt-video/Projects/Example Documentary - First Client, Second Client/Archive - Original Camera Files"
        ),
        Entry::Material {
            org: "vnt-video".into(),
            project: "Example Documentary - First Client, Second Client".into(),
        }
    );
}

/// Inboxes at every level, and each knows which level it is.
#[tokio::test]
async fn unfiled_material_knows_whose_it_is_not_yet() {
    let v = data();
    assert_eq!(
        at(&v, "acme-audio/Inbox"),
        Entry::Inbox(InboxScope::Org("acme-audio".into()))
    );
    assert_eq!(
        at(&v, "vnt-video/Inbox"),
        Entry::Inbox(InboxScope::Org("vnt-video".into()))
    );
    assert_eq!(
        at(&v, "acme-audio/Projects/Example Album/Inbox"),
        Entry::Inbox(InboxScope::Project {
            org: "acme-audio".into(),
            project: "Example Album".into(),
        })
    );
}

/// The sessions are the folders that would become File Roots.
///
/// Three shapes in one tree, which is the point of the fixture: an album
/// whose sessions are subfolders, a single whose session is one subfolder,
/// and a documentary that **is** its own session because the `.drp` sits
/// at project level.
#[tokio::test]
async fn the_sessions_are_found_wherever_the_daw_put_them() {
    let root = archive::example_data();
    let found: Vec<String> = archive::sessions(&root, 6)
        .iter()
        .map(|p| {
            p.strip_prefix(&root)
                .unwrap_or(p)
                .to_string_lossy()
                .into_owned()
        })
        .collect();

    for expected in [
        // An album's sessions are its tracks.
        "acme-audio/Projects/Example Album/Track One",
        "acme-audio/Projects/Example Album/Track Two",
        "acme-audio/Projects/Example Album/Track Three",
        // A single's session is a subfolder.
        "acme-audio/Projects/First Single - Example Client/First Single",
        // A video project is its own session.
        "vnt-video/Projects/Example Documentary - First Client, Second Client",
        "vnt-video/Projects/Shared Project/Shared Cut",
    ] {
        assert!(
            found.iter().any(|f| f == expected),
            "missing {expected}\nfound: {found:#?}"
        );
    }
}

/// A DAW's own saves are not sessions.
///
/// `Track One-2024-10-11_1700.rpp-bak` sits beside `Track One.RPP` and
/// would make its folder a session twice over if the extension check were
/// a prefix check.
// t[verify files.version.native] — a DAW's own saves are recognised as
// the application's, never surfaced as a user-facing version
#[tokio::test]
async fn a_daws_backup_saves_are_not_sessions() {
    let root = archive::example_data();
    let found = archive::sessions(&root, 6);
    let tracks = found
        .iter()
        .filter(|p| p.to_string_lossy().contains("Example Album"))
        .count();
    assert_eq!(
        tracks, 3,
        "an album of three tracks found {tracks} sessions"
    );
}

/// Two orgs on one project, on the disk of neither the one that started
/// it.
///
/// The three facts a single "owner" field cannot hold, asserted as three
/// things: where it sits, who started it, and who is on it.
#[tokio::test]
async fn a_project_can_belong_to_more_than_one_org() {
    // Where it sits: vnt-video's disk.
    let Entry::Project(p) = at(&data(), "vnt-video/Projects/Shared Project") else {
        panic!("not read as a project");
    };
    assert_eq!(p.org, "vnt-video", "placement is the directory");

    // Who is on it, and whose disk is the default for new content. Read
    // from the page rather than the directory, because the directory
    // cannot say.
    let mut collab = Collaboration::started_by(OrgId("acme-audio".into()));
    collab.joined_by(OrgId("vnt-video".into()));

    assert!(collab.is_shared());
    assert_eq!(collab.default_location(), &OrgId("acme-audio".into()));
    assert!(collab.includes(&OrgId("vnt-video".into())));

    // And the origin is only a default: it can hand over, and it can
    // leave, without the project ending.
    collab
        .hand_over(&OrgId("vnt-video".into()))
        .expect("handing over the default");
    collab
        .left_by(&OrgId("acme-audio".into()))
        .expect("the org that started it may leave");
    assert_eq!(collab.default_location(), &OrgId("vnt-video".into()));
}
