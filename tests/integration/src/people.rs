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
//!
//! # Who they are is not decided here
//!
//! The four of them, their emails and what each holds live in
//! [`task_server::example_org::CAST`], beside the tree they work on.
//! `task-server admin demo` creates the same four from the same list, so
//! signing into the demo as Casey gets you what this suite asserts Casey
//! gets.
//!
//! Two lists would have drifted, and the drift would have been silent:
//! a suite proving the client is refused at the session folder, and a
//! demo handing them the whole org, both read as a passing suite.

use files::RootId;
use files::id::PrincipalId;
use files::path::RootPath;
use files::service::access::Subject;
use task_server::example_org::{self, Holds};

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

/// The example's member with this email, or a panic naming what is
/// missing.
///
/// A panic rather than an `Option`: the cast is a compile-time constant
/// two crates share, so a lookup that misses means someone renamed a
/// member on one side, and a `None` quietly threaded through setup would
/// surface as an unexplained denial three chapters later.
fn member(email: &str) -> example_org::Member {
    *example_org::CAST
        .iter()
        .find(|m| m.email == email)
        .unwrap_or_else(|| panic!("`{email}` is not in example_org::CAST"))
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

        let (m_alice, m_victor, m_sam, m_casey) = (
            member("alice@acme.test"),
            member("victor@vnt.test"),
            member("sam@acme.test"),
            member("casey@client.test"),
        );

        let alice = sign_up(&orgs.acme, m_alice).await;
        let victor = sign_up(&orgs.vnt, m_victor).await;
        let sam = sign_up(&orgs.acme, m_sam).await;
        let casey = sign_up(&orgs.acme, m_casey).await;

        orgs.acme
            .backend
            .grant(
                alice.subject.clone(),
                acme_root,
                RootPath::root(),
                Holds::Owner.capabilities(),
            )
            .await
            .expect("ACME grants Alice her own org");
        orgs.vnt
            .backend
            .grant(
                victor.subject.clone(),
                vnt_root,
                RootPath::root(),
                Holds::Owner.capabilities(),
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
                m_sam.holds.capabilities(),
            )
            .expect("Alice hires Sam onto the project");

        // `Deliverables` and nothing above it. The scope comes from the
        // cast, so the demo's Casey is confined to the same folder this
        // chapter asserts they cannot leave.
        orgs.acme
            .backend
            .grant_as(
                &alice.subject,
                casey.subject.clone(),
                acme_root,
                RootPath::parse(m_casey.scope).expect("the client's scope is a path"),
                m_casey.holds.capabilities(),
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
/// The password is the example's, so a token minted here and a sign-in
/// on the demo server are the same credential.
async fn sign_up(server: &crate::server::Server, member: example_org::Member) -> Person {
    let name = member.name;
    let bundle = server
        .auth
        .auth
        .create_email_password_user(architect_auth::CreateEmailPasswordUser {
            email: member.email.to_string(),
            password: example_org::PASSWORD.into(),
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
        email: member.email.to_string(),
    }
}
