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

/// One person: an account, and the subject their grants name.
///
/// **The same id.** `subject` is built from the account's own user id
/// rather than generated, which is what makes a grant and a session
/// refer to one person: the gate resolves a token to
/// `Principal::User { user_id }`, the access lane turns that into this
/// subject, and the grant made to it governs the calls that token
/// signs. They were two unrelated ids until the caller reached the
/// access lane, and while that was true the suite could only assert what
/// capabilities *computed* to — never that anyone was refused.
pub struct Person {
    pub subject: Subject,
    /// The session token. Sign a client with it via `Session::open`.
    pub token: String,
    pub email: String,
}

/// The four accounts, and what each was given.
///
/// Held as [`Subject`]s because that is what the access lane takes: a
/// grant is to a principal, and a principal is not a member of an org
/// here — `files.access.granularity` puts access on the *content*, so a
/// collaborator on another server is addressed the same way as a
/// colleague.
pub struct People {
    /// Owns ACME Audio.
    pub alice: Person,
    /// Owns VNT Video.
    pub victor: Person,
    /// Works at ACME, on this project.
    pub sam: Person,
    /// The client. Here to look at deliverables.
    pub casey: Person,
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

        let alice = sign_up(&orgs.acme, "alice@acme.test", "Alice").await;
        let victor = sign_up(&orgs.vnt, "victor@vnt.test", "Victor").await;
        let sam = sign_up(&orgs.acme, "sam@acme.test", "Sam").await;
        let casey = sign_up(&orgs.acme, "casey@client.test", "Casey").await;

        orgs.acme
            .backend
            .grant(
                alice.subject.clone(),
                acme_root,
                RootPath::root(),
                owner_capabilities(),
            )
            .await
            .expect("ACME grants Alice her own org");
        orgs.vnt
            .backend
            .grant(
                victor.subject.clone(),
                vnt_root,
                RootPath::root(),
                owner_capabilities(),
            )
            .await
            .expect("VNT grants Victor his own org");

        orgs.acme
            .backend
            .grant_as(
                &alice.subject,
                sam.subject.clone(),
                acme_root,
                RootPath::root(),
                employee_capabilities(),
            )
            .expect("Alice hires Sam onto the project");

        // `Deliverables` and nothing above it.
        orgs.acme
            .backend
            .grant_as(
                &alice.subject,
                casey.subject.clone(),
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

/// Create a real account on `server` and take its session.
///
/// A real one, not a `Subject` conjured in the test: the permission
/// gate resolves the token this returns, and a suite whose callers are
/// all anonymous cannot tell a permit table that covers a method from
/// one that does not.
async fn sign_up(server: &crate::server::Server, email: &str, name: &str) -> Person {
    let bundle = server
        .auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: email.to_string(),
            password: "correct-horse-battery-staple".into(),
            name: Some(name.to_string()),
            username: None,
            image: None,
            metadata_json: None,
            ip_address: None,
            user_agent: None,
        })
        .await
        .unwrap_or_else(|e| panic!("sign {name} up: {e:?}"));

    // The account's own id, not a fresh one — see [`Person`].
    let id = bundle
        .user
        .id
        .to_string()
        .parse::<uuid::Uuid>()
        .expect("architect-auth ids are uuids");

    Person {
        subject: Subject::Person(PrincipalId::new(id)),
        token: bundle.token,
        email: email.to_string(),
    }
}
