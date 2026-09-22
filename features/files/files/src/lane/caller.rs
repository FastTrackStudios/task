//! Who is asking, and what they may do — the one answer every lane uses.
//!
//! Before this module each lane that needed a "who" minted its own
//! process-wide placeholder (`this_principal()` in access, organise and
//! version, each a *different* random id), so a hold, a favourite and a
//! grant made by the same person named three strangers. The gate in
//! front of the org router already knows the caller —
//! [`architect::permissions_gate::caller`] — and this is the single place
//! that turns it into the lanes' vocabulary.
//!
//! ## Two sources of capability
//!
//! What a person may do at a path is the union of:
//!
//! - **Their org role**, from the membership row the server consults to
//!   let them into the org at all ([`Memberships`]). An owner or admin
//!   holds everything; a member holds the working set; any other role
//!   reads. This is what makes the lanes usable by the apps that sit on
//!   Task: a musician signed into Keyflow is a member of their own org,
//!   and must not need a grant per folder to open their own charts.
//! - **Explicit grants**, from the access lane — `files.access.granularity`.
//!   These are for people the role does not cover: a client granted one
//!   `Deliverables` folder and nothing else, who has an account on this
//!   org's auth store but no membership row.
//!
//! No membership row means no baseline, never a default one. That is the
//! rule `memberships::role_for` states on the server side — "`None` means
//! NOT A MEMBER" — kept here so a client who signed up to leave a review
//! comment does not inherit the run of the org.
//!
//! ## The process itself
//!
//! A call with no caller at all is the server acting on its own behalf —
//! an in-process call with no transport and no gate in front of it. It
//! holds everything. Every call that arrives over a transport passes
//! through the gate, so that case is not reachable from outside.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use files_proto::error::FilesFault;
use files_proto::id::{PrincipalId, RootId};
use files_proto::path::RootPath;
use files_proto::service::access::{Capability, Subject};

use crate::backend::FilesBackend;

/// Every capability, in the order the access lane reports them.
pub(crate) const ALL: [Capability; 7] = [
    Capability::Read,
    Capability::Write,
    Capability::History,
    Capability::Comment,
    Capability::Download,
    Capability::Deposit,
    Capability::Share,
];

/// What a role that is not a working role may do: look, trace history,
/// say something, take a copy.
const READER: [Capability; 4] = [
    Capability::Read,
    Capability::History,
    Capability::Comment,
    Capability::Download,
];

/// A person's standing in the org this backend serves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrgRole {
    /// Owns the org. Everything, and root lifecycle.
    Owner,
    /// Runs the org. Everything, and root lifecycle.
    Admin,
    /// Works in the org. Everything on content; not root lifecycle.
    Member,
    /// Any other role the membership table carries — a viewer, a guest.
    /// Reads.
    Reader,
}

impl OrgRole {
    /// A membership row's role string. A row with no role is a member —
    /// the row is what admits them, and the org's own default role is
    /// `member`. An unrecognised role reads, which fails towards less.
    #[must_use]
    pub fn from_row(role: Option<&str>) -> Self {
        match role.map(str::trim) {
            None | Some("" | "member") => Self::Member,
            Some("owner") => Self::Owner,
            Some("admin") => Self::Admin,
            Some(_) => Self::Reader,
        }
    }

    /// What this role holds on every path of every root in the org.
    #[must_use]
    pub fn capabilities(self) -> &'static [Capability] {
        match self {
            Self::Owner | Self::Admin | Self::Member => &ALL,
            Self::Reader => &READER,
        }
    }

    /// May this role create, adopt, rename and release roots?
    #[must_use]
    pub fn stewards(self) -> bool {
        matches!(self, Self::Owner | Self::Admin)
    }
}

/// A boxed future, so [`Memberships`] stays object-safe.
pub type RoleFuture<'a> = Pin<Box<dyn Future<Output = Option<OrgRole>> + Send + 'a>>;

/// The server's membership table, as this crate needs it.
///
/// `files` does not know how memberships are stored, and must not: the
/// server owns that table and injects this with
/// [`FilesBackend::set_memberships`]. `None` means no row — not a
/// member.
pub trait Memberships: Send + Sync + std::fmt::Debug {
    fn role(&self, principal: PrincipalId) -> RoleFuture<'_>;
}

/// Who is on the other end of this call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Caller {
    /// No caller: the server acting for itself.
    Process,
    /// A signed-in person.
    Person(PrincipalId),
    /// A caller with credentials who is not a person — a host, a share
    /// guest, a service. Holds nothing on the content lanes; the lanes
    /// that serve them (replica, review) authorise them their own way.
    Other,
}

/// The server's own principal. One id for the process's life, shared by
/// every lane — it is what an in-process caller acts as, and what a
/// grant the server makes on its own behalf records as its issuer.
#[must_use]
pub fn process_principal() -> PrincipalId {
    static ME: OnceLock<PrincipalId> = OnceLock::new();
    *ME.get_or_init(PrincipalId::generate)
}

tokio::task_local! {
    /// Set while the share-link lane acts for a link it has already
    /// scoped — see [`on_behalf_of_link`].
    static FOR_LINK: ();
}

/// Run `fut` as the server, on behalf of a share link.
///
/// A link holder is not a person: the gate resolves them to a guest, and
/// the lanes hold nothing for a guest. The share-link mount therefore
/// checks the link's own scope — this review, this file, comment but not
/// download — and then calls the lanes through this, as the process. It
/// is the one sanctioned way in-process code acts with the server's
/// authority, and nothing reachable over the org router calls it.
///
/// Inside, [`is_for_link`] is true, so a comment is recorded as a
/// visitor's rather than an org member's.
pub async fn on_behalf_of_link<F: Future>(fut: F) -> F::Output {
    FOR_LINK.scope((), fut).await
}

/// Whether this task is acting for a share link.
#[must_use]
pub fn is_for_link() -> bool {
    FOR_LINK.try_with(|()| ()).is_ok()
}

/// The caller of the request running on this task.
#[must_use]
pub fn current() -> Caller {
    if is_for_link() {
        return Caller::Process;
    }
    match architect::permissions_gate::caller() {
        None => Caller::Process,
        Some(architect_permissions::Principal::User { user_id }) => user_id
            .parse::<uuid::Uuid>()
            .map_or(Caller::Other, |id| Caller::Person(PrincipalId::new(id))),
        Some(_) => Caller::Other,
    }
}

/// The caller as a principal, when they have one. What a hold, a
/// favourite, an upload session and a grant's issuer are keyed by.
#[must_use]
pub fn principal() -> Option<PrincipalId> {
    match current() {
        Caller::Process => Some(process_principal()),
        Caller::Person(p) => Some(p),
        Caller::Other => None,
    }
}

/// The caller as the access lane names subjects.
#[must_use]
pub fn subject() -> Option<Subject> {
    principal().map(Subject::Person)
}

impl FilesBackend {
    /// Install the server's membership table. Until this is called no
    /// person has a role baseline, and only explicit grants convey —
    /// which is how every test that never calls it behaves.
    pub fn set_memberships(&self, memberships: Arc<dyn Memberships>) {
        *self
            .memberships_slot()
            .write()
            .expect("memberships lock poisoned") = Some(memberships);
    }

    fn memberships(&self) -> Option<Arc<dyn Memberships>> {
        self.memberships_slot()
            .read()
            .expect("memberships lock poisoned")
            .clone()
    }
}

/// A person's role in this org, or `None` for no membership row.
pub async fn role_of(backend: &FilesBackend, person: PrincipalId) -> Option<OrgRole> {
    match backend.memberships() {
        Some(m) => m.role(person).await,
        None => None,
    }
}

/// What the caller holds on every path by virtue of who they are, before
/// any grant is consulted.
pub async fn baseline(backend: &FilesBackend) -> Vec<Capability> {
    baseline_as(backend, &current()).await
}

/// [`baseline`] for a caller captured earlier — what a stream that
/// outlives its request task (and so its task-local caller) asks.
pub async fn baseline_as(backend: &FilesBackend, who: &Caller) -> Vec<Capability> {
    match who {
        Caller::Process => ALL.to_vec(),
        Caller::Person(p) => role_of(backend, *p)
            .await
            .map_or_else(Vec::new, |r| r.capabilities().to_vec()),
        Caller::Other => Vec::new(),
    }
}

impl Caller {
    /// This caller as the access lane names subjects.
    #[must_use]
    pub fn subject(&self) -> Option<Subject> {
        match self {
            Self::Process => Some(Subject::Person(process_principal())),
            Self::Person(p) => Some(Subject::Person(*p)),
            Self::Other => None,
        }
    }
}

// t[impl files.access.role-baseline] — role united with grants, at the path
/// Refuse unless the caller may do `capability` at `path`.
///
/// The check every lane makes before acting. A role that covers it
/// answers without reading the grant table; otherwise the caller's
/// grants decide, and outside every grant the path reads as absent
/// rather than forbidden — see `access::resolved`.
pub async fn authorise(
    backend: &FilesBackend,
    root_id: RootId,
    path: &RootPath,
    capability: Capability,
) -> Result<(), FilesFault> {
    crate::lane::root_or_fault(backend, root_id)?;
    let path = path.validate()?;
    if baseline(backend).await.contains(&capability) {
        return Ok(());
    }
    match subject() {
        Some(me) => backend.authorise(&me, root_id, &path, capability),
        None => Err(FilesFault::denied(format!("{capability:?}"), path)),
    }
}

/// [`authorise`] at a root's top — for the calls that act on a root as a
/// whole (its catalogue, its history, its ignore rules).
pub async fn authorise_root(
    backend: &FilesBackend,
    root_id: RootId,
    capability: Capability,
) -> Result<(), FilesFault> {
    authorise(backend, root_id, &RootPath::root(), capability).await
}

/// Whether the caller may see *anything* in a root: a role, or a grant
/// anywhere inside it. What `list` filters by — a root you hold one
/// folder of is a root you can name.
pub async fn can_see_root(backend: &FilesBackend, root_id: RootId) -> bool {
    if baseline(backend).await.contains(&Capability::Read) {
        return true;
    }
    subject().is_some_and(|me| backend.holds_anything_in(&me, root_id))
}

/// Refuse unless the caller may manage roots themselves — create, adopt,
/// rename, release, host. An owner's or admin's call, or the server's.
pub async fn authorise_steward(backend: &FilesBackend) -> Result<(), FilesFault> {
    let ok = match current() {
        Caller::Process => true,
        Caller::Person(p) => role_of(backend, p).await.is_some_and(OrgRole::stewards),
        Caller::Other => false,
    };
    if ok {
        Ok(())
    } else {
        Err(FilesFault::denied("manage roots", RootPath::root()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_row_without_a_role_is_a_member_and_an_odd_role_reads() {
        assert_eq!(OrgRole::from_row(None), OrgRole::Member);
        assert_eq!(OrgRole::from_row(Some("owner")), OrgRole::Owner);
        assert_eq!(OrgRole::from_row(Some("admin")), OrgRole::Admin);
        assert_eq!(OrgRole::from_row(Some("viewer")), OrgRole::Reader);
        assert!(!OrgRole::Reader.capabilities().contains(&Capability::Write));
        assert!(OrgRole::Member.capabilities().contains(&Capability::Write));
        assert!(!OrgRole::Member.stewards());
    }

    #[test]
    fn with_no_gate_the_caller_is_the_process() {
        assert_eq!(current(), Caller::Process);
        assert_eq!(principal(), Some(process_principal()));
    }
}
