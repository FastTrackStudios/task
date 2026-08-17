//! One concept: the people.
//!
//! Four accounts, and what each was given. They are deliberately not
//! the servers' own principals: the access lane's owner shortcut hands
//! the *process's* principal everything everywhere — the documented
//! identity gap — so a scenario resting on it would prove nothing about
//! grants. Everyone here holds only what they were explicitly given.
//!
//! | who | what |
//! |---|---|
//! | Alice  | owns ACME Audio |
//! | Victor | owns VNT Video |
//! | Sam    | works at ACME, on this project |
//! | Casey  | the client — here to look at deliverables |
//!
//! Sam and Casey are the two that carry the test. Sam has the run of
//! the session and cannot hand it on; Casey can see the mix, say what
//! they think of it, and not walk away with it.

use files::RootId;
use files::id::PrincipalId;
use files::path::RootPath;
use files::service::access::{Capability, Subject};

use crate::orgs::Orgs;

/// The four accounts, and what each was given.
///
/// Held as [`Subject`]s because that is what the access lane takes: a
/// grant is to a principal, and a principal is not a member of an org
/// here — `files.access.granularity` puts access on the *content*, so a
/// collaborator on another server is addressed the same way as a
/// colleague.
pub struct People {
    /// Owns ACME Audio.
    pub alice: Subject,
    /// Owns VNT Video.
    pub victor: Subject,
    /// Works at ACME, on this project.
    pub sam: Subject,
    /// The client. Here to look at deliverables.
    pub casey: Subject,
}

/// Everything an owner holds.
pub fn owner_capabilities() -> Vec<Capability> {
    vec![
        Capability::Read,
        Capability::Write,
        Capability::History,
        Capability::Comment,
        Capability::Download,
        Capability::Share,
    ]
}

/// What an employee holds: the work, but not the guest list.
///
/// Everything an owner has except [`Capability::Share`]. Withholding it
/// is what makes "Sam cannot widen the client's reach" a property of
/// the system rather than of Sam.
pub fn employee_capabilities() -> Vec<Capability> {
    vec![
        Capability::Read,
        Capability::Write,
        Capability::History,
        Capability::Comment,
        Capability::Download,
    ]
}

/// What a client holds: read the mix, say what they think of it.
///
/// Note what is absent. `Comment` without `Download` is the whole
/// distinction — a client who can review the deliverable is not thereby
/// a client who can keep it.
pub fn client_capabilities() -> Vec<Capability> {
    vec![Capability::Read, Capability::Comment]
}

impl People {
    /// Mint the four accounts and give each what they are owed.
    ///
    /// Owners are granted by their own server, because
    /// `AccessService::grant` mints as the caller and a locally adopted
    /// root has no earlier grant to bootstrap from. Everyone else is
    /// granted *by a person*, through `grant_as`, so the chain is real:
    /// each grant is attenuated by what the granter already holds.
    pub async fn hire(orgs: &Orgs, acme_root: RootId, vnt_root: RootId) -> Self {
        use files::service::access::AccessService;

        let alice = Subject::Person(PrincipalId::generate());
        let victor = Subject::Person(PrincipalId::generate());
        let sam = Subject::Person(PrincipalId::generate());
        let casey = Subject::Person(PrincipalId::generate());

        orgs.acme
            .backend
            .grant(
                alice.clone(),
                acme_root,
                RootPath::root(),
                owner_capabilities(),
            )
            .await
            .expect("ACME grants Alice her own org");
        orgs.vnt
            .backend
            .grant(
                victor.clone(),
                vnt_root,
                RootPath::root(),
                owner_capabilities(),
            )
            .await
            .expect("VNT grants Victor his own org");

        orgs.acme
            .backend
            .grant_as(
                &alice,
                sam.clone(),
                acme_root,
                RootPath::root(),
                employee_capabilities(),
            )
            .expect("Alice hires Sam onto the project");

        // `Deliverables` and nothing above it.
        orgs.acme
            .backend
            .grant_as(
                &alice,
                casey.clone(),
                acme_root,
                RootPath::parse("Deliverables").unwrap(),
                client_capabilities(),
            )
            .expect("Alice sends the client the deliverables");

        Self {
            alice,
            victor,
            sam,
            casey,
        }
    }
}
