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

    let sam = can(&s, &s.people.sam, "Audio Files").expect("Sam works on this project");
    assert!(sam.contains(&Capability::Write), "{sam:?}");

    assert!(
        can(&s, &s.people.casey, "Audio Files").is_err(),
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
    let caps = can(&s, &s.people.casey, "Deliverables").expect("the client's own folder");

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
        .effective_for(&s.people.victor, s.vnt_root, &RootPath::root())
        .map(|e| e.capabilities)
        .expect("Victor at his own company");
    assert!(his.contains(&Capability::Write), "{his:?}");

    assert!(
        can(&s, &s.people.victor, "Audio Files").is_err(),
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
        &s.people.sam,
        s.people.casey.clone(),
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
        &s.people.casey,
        s.people.sam.clone(),
        s.acme_root,
        RootPath::parse("Deliverables").unwrap(),
        vec![Capability::Write],
    );
    assert!(over.is_err(), "a read-only client granted write");
}
