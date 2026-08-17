//! Chapter three — four people, and what each of them may do.
//!
//! Everything in the first two chapters was servers talking. These are
//! the humans, and the point of the chapter is that a grant is over a
//! *path*: the same question gets different answers depending on who
//! asks and where.
//!
//! The accounts are minted in [`integration::people`]; this chapter only
//! asks what they can do. They are deliberately not the servers' own
//! principals — the access lane's owner shortcut hands the process's
//! principal everything everywhere, so a chapter resting on it would
//! prove nothing about grants.

use files::path::RootPath;
use files::service::access::{Capability, Subject};

use integration::client::Session;
use integration::scenario::Scenario;

/// What `who` may do at `path` in ACME's session root.
fn can(s: &Scenario, who: &Subject, path: &str) -> Result<Vec<Capability>, files::FilesFault> {
    s.orgs
        .acme
        .backend
        .effective_for(who, s.acme_root, &RootPath::parse(path).unwrap())
        .map(|e| e.capabilities)
}

/// A grant is over a path, so the answer differs by where you ask.
///
/// Note the shape of Casey's answer: not an empty capability list for
/// the session folder, but a lookup that fails outright. That is
/// deliberate and it is the stronger property — absence and
/// no-permission are the same answer, so a client cannot map what they
/// cannot reach by noting which paths refuse them politely.
// t[verify files.access.granularity]
#[tokio::test]
async fn the_same_path_answers_differently_depending_on_who_asks() {
    let s = Scenario::open().await;

    let sam = can(&s, &s.people.sam.subject, "Audio Files").expect("Sam works on this project");
    assert!(sam.contains(&Capability::Write), "{sam:?}");

    assert!(
        can(&s, &s.people.casey.subject, "Audio Files").is_err(),
        "the client could see the session folder"
    );
}

/// `Comment` without `Download` — the clause the client account exists
/// to test. A client who can say what they think of the mix is not
/// thereby a client who can keep it.
// t[verify files.access.granularity]
#[tokio::test]
async fn a_client_may_review_the_mix_without_taking_it() {
    let s = Scenario::open().await;
    let caps = can(&s, &s.people.casey.subject, "Deliverables").expect("the client's own folder");

    assert!(caps.contains(&Capability::Read), "{caps:?}");
    assert!(caps.contains(&Capability::Comment), "{caps:?}");
    assert!(!caps.contains(&Capability::Download), "{caps:?}");
    assert!(!caps.contains(&Capability::Write), "{caps:?}");
}

/// Owning one org conveys nothing at the other.
///
/// The two companies collaborate on one project and are not one org, so
/// Victor's reach into ACME's tree is whatever ACME granted him — which
/// is nothing.
// t[verify files.access.granularity]
#[tokio::test]
async fn owning_one_org_reaches_nothing_in_the_other() {
    let s = Scenario::open().await;

    let his = s
        .orgs
        .vnt
        .backend
        .effective_for(&s.people.victor.subject, s.vnt_root, &RootPath::root())
        .map(|e| e.capabilities)
        .expect("Victor at his own company");
    assert!(his.contains(&Capability::Write), "{his:?}");

    assert!(
        can(&s, &s.people.victor.subject, "Audio Files").is_err(),
        "owning one org reached into the other"
    );
}

/// `Share` is what separates an employee from an owner.
///
/// Withholding it is what makes "Sam cannot widen the guest list" a
/// property of the system rather than of Sam.
// t[verify files.access.internal-sharing]
#[tokio::test]
async fn an_employee_without_share_cannot_widen_the_guest_list() {
    let s = Scenario::open().await;
    let leak = s.orgs.acme.backend.grant_as(
        &s.people.sam.subject,
        s.people.casey.subject.clone(),
        s.acme_root,
        RootPath::root(),
        vec![Capability::Read],
    );
    assert!(
        leak.is_err(),
        "an employee without Share granted access anyway"
    );
}

/// A chain of grants only ever narrows.
// t[verify files.access.internal-sharing]
#[tokio::test]
async fn a_grant_cannot_convey_what_the_granter_does_not_hold() {
    let s = Scenario::open().await;
    let over = s.orgs.acme.backend.grant_as(
        &s.people.casey.subject,
        s.people.sam.subject.clone(),
        s.acme_root,
        RootPath::parse("Deliverables").unwrap(),
        vec![Capability::Write],
    );
    assert!(over.is_err(), "a read-only client granted write");
}

// ── What a grant actually stops ──────────────────────────────────────
//
// Everything above asks the access lane to compute a capability set,
// which is a calculation. These ask the server to refuse, which is the
// part that protects anything — and until the caller reached the access
// lane, the two were not the same claim: a grant governed a principal
// nobody's session named, so it computed correctly and stopped nobody.

/// The client cannot see the session folder.
// t[verify files.access.granularity]
#[tokio::test]
async fn the_client_is_refused_at_the_session_folder() {
    let s = Scenario::open().await;
    let casey = Session::open(&s.orgs.acme, s.people.casey.token.clone()).await;

    let refused = casey.tree().await.browse(s.acme_root, p("Audio Files")).await;
    assert!(
        refused.is_err(),
        "the client browsed the session folder: {refused:?}"
    );
}

/// And can see the one folder they were sent.
// t[verify files.access.granularity]
#[tokio::test]
async fn the_client_can_see_the_deliverables() {
    let s = Scenario::open().await;
    let casey = Session::open(&s.orgs.acme, s.people.casey.token.clone()).await;

    let listed = casey
        .tree()
        .await
        .browse(s.acme_root, p("Deliverables"))
        .await
        .expect("the client's own folder");
    assert!(
        listed.iter().any(|e| e.name == "mix-v1.wav"),
        "{listed:?}"
    );
}

/// `Comment` without `Download`, as a refusal rather than a capability
/// list.
///
/// This is the clause the client account exists for, and it is the one
/// that could not be tested before: reviewing the mix and keeping it are
/// different acts, and only the server saying no makes them different.
// t[verify files.access.granularity]
#[tokio::test]
async fn a_client_who_may_review_the_mix_may_not_keep_it() {
    let s = Scenario::open().await;
    let casey = Session::open(&s.orgs.acme, s.people.casey.token.clone()).await;

    // Read: allowed — a ticket for the mix.
    casey
        .media()
        .await
        .read(s.acme_root, p("Deliverables/mix-v1.wav"))
        .await
        .expect("the client may play the mix");

    // Download: refused. An archive is how a whole selection leaves.
    let refused = casey
        .write()
        .await
        .archive(s.acme_root, vec![p("Deliverables")])
        .await;
    assert!(
        refused.is_err(),
        "the client archived the deliverables: {refused:?}"
    );
}

/// The client cannot write to the folder they can read.
// t[verify files.access.granularity]
#[tokio::test]
async fn the_client_cannot_write_where_they_may_read() {
    let s = Scenario::open().await;
    let casey = Session::open(&s.orgs.acme, s.people.casey.token.clone()).await;

    let refused = casey
        .write()
        .await
        .create_dirs(s.acme_root, vec![p("Deliverables/Notes")])
        .await;
    assert!(refused.is_err(), "the client wrote to the deliverables");
}

/// The employee can work in the session; the client cannot. Same call,
/// same path, different person.
// t[verify files.access.granularity]
#[tokio::test]
async fn the_employee_works_where_the_client_cannot() {
    let s = Scenario::open().await;
    let sam = Session::open(&s.orgs.acme, s.people.sam.token.clone()).await;

    sam.write()
        .await
        .create_dirs(s.acme_root, vec![p("Audio Files/Comps")])
        .await
        .expect("the employee works on this project");
}

fn p(s: &str) -> files::path::RootPath {
    files::path::RootPath::parse(s).expect("test path")
}
