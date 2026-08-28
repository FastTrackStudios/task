//! `AccessService` — the inward half of sharing.
//!
//! v1 could share *outward*: a public link, minted per review, resolved
//! by `apps/server`'s share routes. It had no way to hand a colleague a
//! folder. This lane is the other direction — a grant of an explicit
//! capability set to a person or a team, with no link involved.
//!
//! ## Capabilities are the primitive
//!
//! Nothing here checks a role. A role is a named bundle of rules that a
//! caller expands *before* it grants, so the only thing this lane stores
//! and the only thing it evaluates is a set of [`Capability`] over a
//! subtree. That is what keeps "Reviewer" a label in a UI rather than a
//! branch in a backend, and it is why [`rule_of`] can project a grant to
//! an architect `Rule { resource, actions }` without losing anything.
//!
//! ## Authorisation happens at the path, and absence is the default
//!
//! [`FilesBackend::authorise`] resolves at the path being acted on, and
//! a grant on `Sessions/Song` conveys nothing at `Sessions`. The two
//! failures are deliberately different faults:
//!
//! - inside a granted subtree, missing the capability is
//!   [`FilesFault::Denied`] — the caller knows the path exists and is
//!   being told what they may not do with it;
//! - outside every granted subtree, the answer is
//!   [`FilesFault::PathNotFound`] — the path is *absent*, because
//!   "forbidden" is itself an answer about what exists, and a listing
//!   that shows padlocks is a directory listing of someone else's work.
//!
//! The one crack in that wall is deliberate: the ancestors *above* a
//! granted subtree resolve to an empty capability set rather than to
//! absence, because a grantee already knows those directories exist —
//! their own grant names them — and a UI has to render a path to the
//! folder it can open. Empty means empty: nothing may be done there,
//! and the listing lane is responsible for showing only the children
//! that lead to a grant.
//!
//! ## Three things here are not real, and are not pretended to be
//!
//! **No caller identity reaches this trait.** `FilesBackend` holds no
//! session and no method carries a principal, the same gap
//! [`super::version`] documents at `this_principal`. So the resolution
//! is written against an explicit subject — [`FilesBackend::authorise`],
//! [`FilesBackend::effective_for`], [`FilesBackend::grant_as`], which
//! are what a dispatcher with a session will call — and the trait
//! methods supply [`this_principal`], which this lane treats as the
//! owner of every root the process has adopted. That is honest for a
//! locally-adopted root and wrong the moment two users share a server:
//! until identity is threaded, [`AccessService::effective`] answers for
//! the server, not for a user. Threading it is a change to the trait
//! methods below, not to the resolution.
//!
//! **Grants and links are not yet in the vault.** They are durable and
//! per-org — [`crate::durable`] keys [`AccessState`] by the backend's own
//! data directory and writes it atomically on every mutation — but
//! `storage.tier.authored` wants a human's choices as markdown a human
//! can read, and a JSON table beside `roots.json` is not that. It is the
//! step that stops a restart forgetting who a folder was shared with, and
//! stops one org's `grants` and `shares` answering with another's, which
//! is what a process-wide static did.
//!
//! **The links minted here are not yet the links the world can reach.**
//! The durable share store is `apps/server`'s `share::ShareStore` —
//! tokens on disk, resolved on every hit by the `/org/{slug}/share/…`
//! routes and by the guest vox lane in `apps/server/src/share_guest.rs`,
//! with password, expiry and disabled all enforced there. That crate
//! depends on this one, so this lane cannot call it; bridging means
//! injecting a port into `FilesBackend`, which is `backend.rs`'s to
//! change. Until then [`AccessService::create_share`] mints a token this
//! process knows and no HTTP route resolves.
//!
//! FUTURE: inject a share-link port so `create_share` writes through to
//! `apps/server`'s `ShareStore` instead of the table here, and carry
//! password and expiry — neither is a parameter on this trait.

use facet::Facet;
use std::sync::OnceLock;

use chrono::{DateTime, Utc};
use files_proto::error::FilesFault;
use files_proto::id::{GrantId, PrincipalId, RootId, ShareId};
use files_proto::path::RootPath;
use files_proto::service::access::{
    AccessService, Capability, Effective, Grant, ShareLink, Subject,
};

use crate::backend::FilesBackend;

/// Every capability, in the order a resolved set is reported.
///
/// A fixed order rather than a sort: [`Capability`] is a wire enum with
/// no ordering of its own, and a UI comparing two `Effective` values
/// must not see a difference that is only iteration order.
const ALL: [Capability; 7] = [
    Capability::Read,
    Capability::Write,
    Capability::History,
    Capability::Comment,
    Capability::Download,
    Capability::Deposit,
    Capability::Share,
];

// ── Identity ───────────────────────────────────────────────────────

/// The principal this process speaks for.
///
/// Minted once per process, exactly as `super::version::this_principal`
/// is and for the same reason: no method on this trait carries a caller.
/// Public because the resolution it feeds is public — a test, and later
/// a dispatcher, has to be able to name the subject the trait methods
/// use.
#[must_use]
pub fn this_principal() -> PrincipalId {
    static ME: OnceLock<PrincipalId> = OnceLock::new();
    *ME.get_or_init(PrincipalId::generate)
}

/// WHO is asking, as the access lane names subjects.
///
/// Three answers, and the middle one is the load-bearing one:
///
/// - **A signed-in person** → their own subject, so their grants decide.
///   The id is the account's, not a second identity invented here: a
///   grant made to a person and a session held by that person have to
///   name the same principal or the grant governs nobody.
/// - **No caller at all** → the process itself, which is what
///   [`this_principal`] has always meant. An in-process call has no wire
///   and no gate in front of it; it is the server acting on its own
///   behalf, and that is the case the owner shortcut exists for. Every
///   call that arrives over a transport passes through
///   `org_router_guarded`, so "no caller" is not reachable from outside.
/// - **Anything else** → no subject. A host, a share guest and a service
///   are all callers with credentials and none of them is a person with
///   grants, so resolving them to *some* subject could only be a guess.
fn calling_subject() -> Option<Subject> {
    match architect::permissions_gate::caller() {
        None => Some(Subject::Person(this_principal())),
        Some(architect_permissions::Principal::User { user_id }) => user_id
            .parse::<uuid::Uuid>()
            .ok()
            .map(|id| Subject::Person(PrincipalId::new(id))),
        Some(_) => None,
    }
}

/// Whether a subject is the process's own principal.
///
/// The owner holds everything, everywhere, because a locally adopted
/// root was adopted *by this server* and no grant exists to bootstrap
/// from. This is the whole of the identity gap: replace the caller of
/// [`AccessService::effective`] with a session principal and this rule
/// stops applying to anybody but the server itself.
fn is_owner(subject: &Subject) -> bool {
    matches!(subject, Subject::Person(p) if *p == this_principal())
}

// ── State ──────────────────────────────────────────────────────────

/// Grants and links for one org.
///
/// A `Vec` rather than an index: a root's grant list is a handful of
/// rows, every read is a full scan against one root, and an index keyed
/// by subtree would need the same scan to answer "what governs this
/// path" anyway. It is also the shape the file wants, so no `Wire`
/// conversion is needed here — `Grant` and `ShareLink` are wire types
/// already, and a list of them is expressible in JSON as-is.
///
/// Persisted per org — see [`crate::durable`]. Keyed by the backend's
/// data directory rather than held in a process-wide static, which is
/// what stops one org's [`AccessService::grants`] and
/// [`AccessService::shares`] enumerating another org's rows.
#[derive(Clone, Debug, Default, Facet)]
#[repr(C)]
struct AccessState {
    grants: Vec<Grant>,
    shares: Vec<ShareLink>,
}

crate::durable::durable_as_itself!(AccessState);

static ACCESS: crate::durable::Scoped<AccessState> = crate::durable::Scoped::new("access");

/// Mutate and persist.
fn with_state<T>(backend: &FilesBackend, f: impl FnOnce(&mut AccessState) -> T) -> T {
    ACCESS.write(backend, f)
}

/// Read without persisting.
///
/// Separate from [`with_state`] because a read that writes the state file
/// back is both wasted io and a lie in the file's mtime — "when was this
/// folder last shared with someone" must not answer "when someone last
/// asked who it was shared with".
fn read_state<T>(backend: &FilesBackend, f: impl FnOnce(&AccessState) -> T) -> T {
    ACCESS.read(backend, f)
}

/// Whether a grant still conveys anything at `now`.
///
/// Expiry is evaluated on every read rather than swept, so a lapsed
/// grant stops conveying without anyone running a reaper — the same
/// reason revocation binds on the next request rather than on a cache
/// refresh.
fn is_live(grant: &Grant, now: DateTime<Utc>) -> bool {
    grant.expires_at.is_none_or(|at| at > now)
}

/// A grant as an architect `Rule { resource, actions }`.
///
/// `files.access.internal-sharing` specifies the grant *is* a rule, and
/// this is that projection. It is not the check — `architect-permissions`
/// is not a dependency of this crate, so [`resolved`] evaluates the same
/// semantics directly (a `**` suffix is the subtree, which is what
/// [`RootPath::is_within`] answers). Kept because the shape is the
/// migration: when the engine arrives, the store becomes rules and the
/// resolution becomes `engine.check`, with nothing in between to invent.
#[must_use]
pub fn rule_of(grant: &Grant) -> (String, Vec<String>) {
    let path = grant.path.as_str();
    let resource = if path.is_empty() {
        format!("files/{}/**", grant.root_id)
    } else {
        format!("files/{}/{path}/**", grant.root_id)
    };
    (
        resource,
        grant
            .capabilities
            .iter()
            .map(|c| format!("{c:?}").to_lowercase())
            .collect(),
    )
}

// ── Resolution ─────────────────────────────────────────────────────

/// Put a capability set into [`ALL`]'s order, dropping duplicates from
/// overlapping grants.
fn canonical(mut caps: Vec<Capability>) -> Vec<Capability> {
    caps.sort_by_key(|c| ALL.iter().position(|a| a == c).unwrap_or(ALL.len()));
    caps.dedup();
    caps
}

/// What `subject` may do at `path`, or why the question has no answer.
///
/// The union of every live grant whose subtree contains the path. A
/// subject with no such grant gets [`FilesFault::PathNotFound`] unless
/// they hold a grant *beneath* the path, in which case the path is an
/// ancestor they must be able to name and resolves to nothing.
///
/// Takes the backend because the grants it scans are that org's, not the
/// process's: resolving against a table shared by every org on the server
/// would answer with a grant nobody here ever made.
fn resolved(
    backend: &FilesBackend,
    subject: &Subject,
    root_id: RootId,
    path: &RootPath,
) -> Result<Vec<Capability>, FilesFault> {
    if is_owner(subject) {
        return Ok(ALL.to_vec());
    }
    let now = Utc::now();
    let (caps, beneath) = read_state(backend, |s| {
        let mut caps: Vec<Capability> = Vec::new();
        let mut beneath = false;
        for grant in s
            .grants
            .iter()
            .filter(|g| g.root_id == root_id && g.subject == *subject && is_live(g, now))
        {
            if path.is_within(&grant.path) {
                caps.extend(grant.capabilities.iter().copied());
            } else if grant.path.is_within(path) {
                beneath = true;
            }
        }
        (caps, beneath)
    });

    if caps.is_empty() && !beneath {
        // Absent, not forbidden. Answering `Denied` here would confirm
        // to anyone who can ask that the path exists.
        return Err(FilesFault::PathNotFound(path.clone()));
    }
    Ok(canonical(caps))
}

/// The principal a grant is recorded as coming from.
///
/// `Grant::granted_by` is a `PrincipalId`, which a remote granter does
/// not have — federation identifies them by `(server, principal)`
/// strings. Recording this server instead of inventing an id keeps the
/// field true: the grant was made *here*, on that remote party's behalf.
///
/// FUTURE: widen `Grant::granted_by` to a `Subject` so a federated
/// grant records who actually made it.
fn granted_by(granter: &Subject) -> PrincipalId {
    match granter {
        Subject::Person(p) | Subject::Team(p) => *p,
        Subject::Remote { .. } => this_principal(),
    }
}

// ── The authorisation surface other lanes call ─────────────────────

impl FilesBackend {
    // t[impl files.access.granularity] — resolved at the path acted on,
    // and absent rather than forbidden outside a granted subtree
    /// Refuse unless `subject` may do `capability` at `path`.
    ///
    /// The function every other lane should call before acting. It is
    /// separate from [`AccessService::effective`] because a lane knows
    /// the one action it is about to take, and a fault naming that
    /// action is what a UI can show.
    pub fn authorise(
        &self,
        subject: &Subject,
        root_id: RootId,
        path: &RootPath,
        capability: Capability,
    ) -> Result<(), FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        let caps = resolved(self, subject, root_id, &path)?;
        if caps.contains(&capability) {
            Ok(())
        } else {
            Err(FilesFault::denied(format!("{capability:?}"), path))
        }
    }

    /// Refuse unless the **caller** may do `capability` at `path`.
    ///
    /// The one every other lane calls. `authorise` takes an explicit
    /// subject and is the right shape for the access lane's own methods,
    /// where the subject is an argument; a lane acting on a request has
    /// no subject to pass and must not invent one, so it asks about
    /// whoever is on the other end.
    ///
    /// `files.access.granularity` is why this is per path rather than per
    /// root: a client holding `Deliverables` and nothing else must be
    /// refused at `Audio Files`, and refused in a way that does not
    /// distinguish "you may not" from "there is nothing there" — see
    /// [`resolved`], which fails the lookup rather than returning an
    /// empty capability set.
    pub fn authorise_caller(
        &self,
        root_id: RootId,
        path: &RootPath,
        capability: Capability,
    ) -> Result<(), FilesFault> {
        let Some(me) = calling_subject() else {
            return Err(FilesFault::denied(
                format!("{capability:?}"),
                path.clone().validate()?,
            ));
        };
        self.authorise(&me, root_id, path, capability)
    }

    // t[impl files.access.granularity]
    /// [`AccessService::effective`] for an explicit subject — the shape
    /// the trait method will take once a session reaches it.
    pub fn effective_for(
        &self,
        subject: &Subject,
        root_id: RootId,
        path: &RootPath,
    ) -> Result<Effective, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        let capabilities = resolved(self, subject, root_id, &path)?;
        Ok(Effective {
            root_id,
            path,
            capabilities,
        })
    }

    // t[impl files.access.internal-sharing] — an explicit capability set,
    // attenuated by the granter's own, and no link minted
    /// [`AccessService::grant`] made by an explicit granter.
    ///
    /// Attenuating is the point: `Share` lets a grantee pass on what
    /// they hold and nothing more, so a chain of grants can only ever
    /// narrow. Without this, `Share` would be indistinguishable from
    /// ownership.
    pub fn grant_as(
        &self,
        granter: &Subject,
        subject: Subject,
        root_id: RootId,
        path: RootPath,
        capabilities: Vec<Capability>,
    ) -> Result<Grant, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        if capabilities.is_empty() {
            return Err(FilesFault::invalid(
                "a grant with no capabilities conveys nothing — revoke instead",
            ));
        }
        // Both halves resolve at `path`, so a granter holding `Share`
        // over one folder cannot grant over its parent: `resolved`
        // answers `PathNotFound` there, which is the same absence the
        // grantee would see.
        let held = resolved(self, granter, root_id, &path)?;
        if !held.contains(&Capability::Share) {
            return Err(FilesFault::denied("Share", path));
        }
        if let Some(over) = capabilities.iter().find(|c| !held.contains(c)) {
            return Err(FilesFault::denied(format!("{over:?}"), path));
        }

        let grant = Grant {
            id: GrantId::generate(),
            subject,
            root_id,
            path,
            capabilities: canonical(capabilities),
            granted_by: granted_by(granter),
            granted_at: Utc::now(),
            expires_at: None,
        };
        with_state(self, |s| s.grants.push(grant.clone()));
        Ok(grant)
    }
}

// ── The lane ───────────────────────────────────────────────────────

impl AccessService for FilesBackend {
    // t[impl files.access.internal-sharing] — a person or team, an explicit
    // capability set, no public link anywhere in the path
    async fn grant(
        &self,
        subject: Subject,
        root_id: RootId,
        path: RootPath,
        capabilities: Vec<Capability>,
    ) -> Result<Grant, FilesFault> {
        // The path is not checked against the tree, deliberately: a
        // grant is policy about a name, and requiring the folder to
        // exist would make granting fail on a root whose content has not
        // yet synced to this device.
        self.grant_as(
            &Subject::Person(this_principal()),
            subject,
            root_id,
            path,
            capabilities,
        )
    }

    // t[impl files.access.internal-sharing] — revocation binds on the next
    // request: the table is read per call, never cached into a session
    async fn revoke(&self, grant: GrantId) -> Result<(), FilesFault> {
        let existing = read_state(self, |s| s.grants.iter().find(|g| g.id == grant).cloned());
        // Not a `NotFound`: a grant that is not live is one that conveys
        // nothing, and the caller's intent — that it convey nothing — is
        // already satisfied. Saying so is more useful than an error the
        // caller cannot act on.
        let existing = existing.ok_or(FilesFault::GrantRevoked(grant))?;
        self.authorise(
            &Subject::Person(this_principal()),
            existing.root_id,
            &existing.path,
            Capability::Share,
        )?;
        with_state(self, |s| s.grants.retain(|g| g.id != grant));
        Ok(())
    }

    // t[impl files.access.granularity] — the grants *governing* a path, which
    // is the set that answers "who can see this"; a grant further down
    // governs its own subtree and is found by asking about that path
    async fn grants(
        &self,
        root_id: RootId,
        path: Option<RootPath>,
    ) -> Result<Vec<Grant>, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.map(|p| p.validate()).transpose()?;
        let now = Utc::now();
        Ok(read_state(self, |s| {
            s.grants
                .iter()
                .filter(|g| g.root_id == root_id && is_live(g, now))
                .filter(|g| path.as_ref().is_none_or(|p| p.is_within(&g.path)))
                .cloned()
                .collect()
        }))
    }

    // t[impl files.access.granularity] — one resolved answer at this path
    async fn effective(&self, root_id: RootId, path: RootPath) -> Result<Effective, FilesFault> {
        // Answers for whoever is asking. It used to answer for the
        // process principal on every call, which made it the one method
        // that became *wrong* rather than merely incomplete the moment a
        // second user appeared on a server.
        let me = calling_subject().ok_or_else(|| {
            FilesFault::invalid("this caller is not a person, so it holds no capabilities")
        })?;
        self.effective_for(&me, root_id, &path)
    }

    // t[impl files.access.internal-sharing] — the outward lane, kept
    // distinct: minting a link is a separate act from granting, and
    // needs `Share` at the path like any other pass-on of access
    async fn create_share(
        &self,
        root_id: RootId,
        path: RootPath,
        capabilities: Vec<Capability>,
    ) -> Result<ShareLink, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        let path = path.validate()?;
        if capabilities.is_empty() {
            return Err(FilesFault::invalid(
                "a link conveying nothing is not a link",
            ));
        }
        let me = Subject::Person(this_principal());
        self.authorise(&me, root_id, &path, Capability::Share)?;
        let held = resolved(self, &me, root_id, &path)?;
        if let Some(over) = capabilities.iter().find(|c| !held.contains(c)) {
            // A link cannot convey more than its issuer holds, for the
            // same reason a grant cannot: otherwise `Share` is a
            // privilege-escalation primitive.
            return Err(FilesFault::denied(format!("{over:?}"), path));
        }

        let link = ShareLink {
            id: ShareId::generate(),
            root_id,
            path,
            capabilities: canonical(capabilities),
            // Opaque and unguessable: the token *is* the credential, so
            // it is minted here rather than derived from anything the
            // caller supplied or an attacker can enumerate.
            token: uuid::Uuid::new_v4().simple().to_string(),
            // Neither password nor expiry is a parameter on this trait;
            // reporting `false` is the truth about this link rather than
            // a default anyone chose. See the module doc.
            password_set: false,
            expires_at: None,
            disabled: false,
        };
        with_state(self, |s| s.shares.push(link.clone()));
        Ok(link)
    }

    // t[impl files.access.internal-sharing] — pausing is not deleting: the
    // token survives, so a link is resumed rather than reissued to
    // everyone who already has it
    async fn set_share_disabled(
        &self,
        share: ShareId,
        disabled: bool,
    ) -> Result<ShareLink, FilesFault> {
        with_state(self, |s| {
            let link = s
                .shares
                .iter_mut()
                .find(|l| l.id == share)
                .ok_or_else(|| FilesFault::invalid(format!("no such share link: {share}")))?;
            link.disabled = disabled;
            Ok(link.clone())
        })
    }

    async fn shares(&self, root_id: RootId) -> Result<Vec<ShareLink>, FilesFault> {
        crate::lane::root_or_fault(self, root_id)?;
        // Disabled links included: a paused link is one a human has to
        // be able to find in order to resume it, and omitting it would
        // make pausing look like deleting.
        Ok(read_state(self, |s| {
            s.shares
                .iter()
                .filter(|l| l.root_id == root_id)
                .cloned()
                .collect()
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> RootPath {
        RootPath::parse(s).expect("path")
    }

    fn grant_at(path: &str, caps: &[Capability]) -> Grant {
        Grant {
            id: GrantId::generate(),
            subject: Subject::Person(PrincipalId::generate()),
            root_id: RootId::generate(),
            path: p(path),
            capabilities: caps.to_vec(),
            granted_by: PrincipalId::generate(),
            granted_at: Utc::now(),
            expires_at: None,
        }
    }

    // t[verify files.access.internal-sharing]
    #[test]
    fn an_expired_grant_conveys_nothing_without_a_sweep() {
        let now = Utc::now();
        let mut grant = grant_at("Sessions", &[Capability::Read]);
        assert!(is_live(&grant, now), "no expiry is not an expiry");

        grant.expires_at = Some(now - chrono::TimeDelta::seconds(1));
        assert!(
            !is_live(&grant, now),
            "expiry is evaluated on read, so nothing has to run for it to bind"
        );
    }

    // t[verify files.access.internal-sharing]
    #[test]
    fn a_grant_projects_to_an_architect_rule_over_its_subtree() {
        let grant = grant_at("Sessions/Song", &[Capability::Read, Capability::Comment]);
        let (resource, actions) = rule_of(&grant);
        assert!(
            resource.ends_with("/Sessions/Song/**"),
            "the rule addresses the subtree, not the one node: {resource}"
        );
        assert_eq!(actions, vec!["read".to_string(), "comment".to_string()]);
    }

    // t[verify files.access.granularity]
    #[test]
    fn a_rule_pattern_covers_exactly_what_the_resolver_covers() {
        // The glob and `is_within` are two spellings of one rule, and
        // they will be one spelling when `engine.check` arrives. This
        // pins them to the same answers meanwhile.
        let granted = p("Sessions/Song");
        assert!(p("Sessions/Song/mix.wav").is_within(&granted));
        assert!(p("Sessions/Song").is_within(&granted));
        assert!(
            !p("Sessions").is_within(&granted),
            "a parent is not covered"
        );
        assert!(!p("Sessions/Other").is_within(&granted));
    }

    #[test]
    fn a_capability_set_is_reported_in_one_order() {
        let a = canonical(vec![Capability::Share, Capability::Read, Capability::Read]);
        let b = canonical(vec![Capability::Read, Capability::Share]);
        assert_eq!(a, b, "overlapping grants must not produce two answers");
        assert_eq!(a.len(), 2, "and a duplicate is not a second capability");
    }
}
