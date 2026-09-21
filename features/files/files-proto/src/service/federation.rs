//! Content across a server boundary — `files.topology.federation`.
//!
//! Two halves, and they run on different servers.
//!
//! The **origin** offers: it holds the bytes and the history, and it
//! grants a subtree to a principal it does not host. The offer is a
//! capability — an endpoint to dial, a root to ask for, and a secret
//! that stands for the grant — and it stays revocable from the
//! originating side, because a grant that only the receiver can end is
//! not a grant.
//!
//! The **receiver** accepts: the subtree becomes a root in its own
//! registry, browsable through [`TreeService`](crate::service::tree)
//! like any other. That is the requirement's sharpest clause — *a
//! first-class item, not a download link* — and it is why this lane has
//! no browse method of its own. Adding one would make federated content
//! a second kind of thing with a second set of verbs, which is the
//! outcome the rule exists to refuse.
//!
//! # What crosses, and what does not
//!
//! Content addressing is shared, so bytes the receiver already holds
//! transfer again as nothing: a collaborator who has the same stems
//! under a different name pays nothing to accept them.
//!
//! Identity does not cross. The federation model — who a remote
//! principal is, and which servers trust each other — belongs to charter
//! [#22](https://github.com/FastTrackStudios/task/issues/22). This lane
//! carries a secret that the origin minted and can withdraw, which is
//! the smallest thing that works and does not pretend to be an identity
//! system.

use chrono::{DateTime, Utc};
use facet::Facet;

use crate::error::FilesFault;
use crate::id::{GrantId, RootId};
use crate::path::RootPath;
use crate::service::access::Capability;
use crate::service::media::ByteTicket;

/// Where a server is, as a peer rather than an address.
///
/// A public key. Registration is pasting one of these into a device —
/// there is no host, no port and no certificate to get wrong, and a
/// server that moves networks keeps its id.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Facet)]
#[repr(C)]
pub struct EndpointId(pub String);

impl std::fmt::Display for EndpointId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// What one server hands another so it can reach a subtree.
///
/// Everything the receiver needs and nothing it does not: it never
/// learns the origin's other roots, its principals, or anything about
/// the subtree's parents.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Offer {
    /// The origin, to dial.
    pub origin: EndpointId,
    /// The root there. Meaningless anywhere else.
    pub root_id: RootId,
    /// The subtree within it. A grant on a folder says nothing about
    /// its parents, so this is the whole of what was offered.
    pub path: RootPath,
    /// A display name, so the receiver has something to call it before
    /// it has dialled anything.
    pub name: String,
    pub capabilities: Vec<Capability>,
    /// Presented on every call back to the origin. The origin minted it
    /// and can withdraw it; holding it is the whole of the receiver's
    /// authority.
    pub secret: String,
    /// The grant this offer stands for, so the origin can revoke by id
    /// without the receiver's cooperation.
    pub grant: GrantId,
    pub expires_at: Option<DateTime<Utc>>,
}

/// An offer this server has accepted.
#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub struct Remote {
    /// The local root the offer became. It is an ordinary `RootId` on
    /// purpose: every other lane addresses it without knowing it is not
    /// ours.
    pub root_id: RootId,
    pub origin: EndpointId,
    /// The root's id *there*, which is what calls back to the origin
    /// name.
    pub origin_root: RootId,
    pub name: String,
    pub capabilities: Vec<Capability>,
    /// False once the origin has been unreachable. Structure stays
    /// browsable; only its content is out of reach.
    pub reachable: bool,
    pub accepted_at: DateTime<Utc>,
}

/// The root here that stands for `origin_root` there.
///
/// # Why this is the missing half of a binding
///
/// An asset's manifest says where its bytes are with a
/// `ContentRef { root_id, path }` (ADR 0003), and that `root_id` is the
/// root's id *in the organisation that wrote it*. Carry the manifest
/// across a server boundary — which is the whole point of it being
/// small — and the id arrives naming a root the reader does not have.
/// Used as a local `RootId` it resolves to nothing, which reads as
/// "the file is missing" rather than "this is somebody else's id".
///
/// [`Remote::origin_root`] is exactly the fact needed to fix that, and
/// was recorded from the start; what was missing was anything that
/// looked a root up *by* it. So an app holding a foreign reference and
/// an accepted offer of the root it names can get from one to the other,
/// and every lane after that point is addressing an ordinary local root
/// — which is what `files.topology.federation` promises when it says an
/// accepted offer arrives "as a first-class item".
///
/// `None` means no offer of that root has been accepted here. That is
/// the honest answer and not an error: the reference is still a valid
/// address, it just is not reachable yet, and the fix is an offer rather
/// than a retry.
///
/// # Prefer [`FederationService::resolve_content`] to read bytes
///
/// This answers *which root*, and that is not enough to read a file. An
/// offer grants a subtree, so the manifest's path has to be rebased onto
/// the accepted root — and [`Remote`] does not carry the offered path, so
/// nothing working from it can do that correctly. Two offers of different
/// subtrees of one root also both match here, first one winning.
/// `resolve_content` answers from the record that kept the subtree, and
/// picks the most specific offer that actually contains the path.
#[must_use]
pub fn adopted_from(remotes: &[Remote], origin_root: RootId) -> Option<RootId> {
    remotes
        .iter()
        .find(|r| r.origin_root == origin_root)
        .map(|r| r.root_id)
}

/// A foreign content reference, resolved: where its bytes are **here**.
///
/// Both halves are local and ordinary — a root every lane addresses
/// without knowing it came from an offer, and a path inside it — so what
/// a caller does next is exactly what it would do with its own file.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct LocalContent {
    pub root_id: RootId,
    pub path: RootPath,
}

/// One accepted offer as far as resolving content is concerned: which
/// origin root it stands for, which subtree of it was offered, and the
/// root here it became.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedSubtree {
    pub origin_root: RootId,
    pub offered: RootPath,
    pub local_root: RootId,
}

/// Resolve `origin_root` / `path` against what this server has accepted.
///
/// The logic behind [`FederationService::resolve_content`], kept pure so
/// the one rule that matters can be tested without a backend: **the most
/// specific offer that contains the path wins**. Offers nest — somebody
/// may be given a whole session and, separately, one folder inside it —
/// and whichever is narrower is the grant that was meant for this file.
/// Ties cannot happen for one path: two offers of the same subtree of the
/// same root differ only in id, and either reaches the same bytes.
///
/// Component-wise throughout ([`RootPath::relative_to`]), so an offer of
/// `Audio Files` never matches `Audio Files 2/take.wav`.
#[must_use]
pub fn resolve_content_ref(
    accepted: impl IntoIterator<Item = AcceptedSubtree>,
    origin_root: RootId,
    path: &RootPath,
) -> Option<LocalContent> {
    accepted
        .into_iter()
        .filter(|a| a.origin_root == origin_root)
        .filter_map(|a| {
            let inside = path.relative_to(&a.offered)?;
            Some((a.offered.components().count(), a.local_root, inside))
        })
        .max_by_key(|(depth, _, _)| *depth)
        .map(|(_, root_id, path)| LocalContent { root_id, path })
}

/// A bounded window of an object, for a relayed read.
///
/// Half-open: `[offset, offset + len)`. `len` is what keeps a relay's
/// memory flat regardless of how large the object is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ByteRange {
    pub offset: u64,
    pub len: u32,
}

impl ByteRange {
    /// The largest chunk an origin will serve in one call.
    ///
    /// A relay's memory ceiling, so it is enforced on the serving side
    /// rather than trusted from the caller.
    pub const MAX_LEN: u32 = 1 << 20;

    #[must_use]
    pub fn new(offset: u64, len: u32) -> Self {
        Self {
            offset,
            len: len.min(Self::MAX_LEN),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(u8)]
pub enum FederationEvent {
    Offered(Offer),
    Accepted(Remote),
    /// The origin withdrew it, or it lapsed.
    Withdrawn(RootId),
    /// The origin's reachability changed. Not a change to the tree.
    ReachabilityChanged(Remote),
}

#[architect::rpc]
pub trait FederationService {
    /// Offer a subtree to a principal on another server.
    ///
    /// The origin side. Mints the grant and the secret that stands for
    /// it; the caller passes the resulting [`Offer`] to the other server
    /// by whatever means it likes, which is deliberately not this lane's
    /// problem — an offer is inert until accepted, and carrying it is a
    /// message, not a protocol.
    async fn offer(
        &self,
        root_id: RootId,
        path: RootPath,
        to: EndpointId,
        capabilities: Vec<Capability>,
    ) -> Result<Offer, FilesFault>;

    /// Withdraw an offer, from the originating side.
    ///
    /// Binds on the receiver's next call, because the secret is checked
    /// on every one. A receiver that is offline when this runs finds out
    /// the moment it comes back, which is the only ordering a federated
    /// revocation can honestly promise.
    async fn withdraw(&self, grant: GrantId) -> Result<(), FilesFault>;

    /// What this server has offered, and to whom.
    async fn offered(&self) -> Result<Vec<Offer>, FilesFault>;

    /// Accept an offer, making it a root here.
    ///
    /// The receiving side. From here the subtree is browsable through
    /// the ordinary lanes; nothing else needs to know it is not local.
    async fn accept(&self, offer: Offer) -> Result<Remote, FilesFault>;

    /// Remotes this server has accepted.
    async fn remotes(&self) -> Result<Vec<Remote>, FilesFault>;

    /// Where a **foreign** content reference's bytes are, here.
    ///
    /// The receiving half of an asset whose manifest crossed a server
    /// boundary. A manifest names its bytes as `ContentRef { root_id,
    /// path }` in the *publisher's* terms (ADR 0003). This turns that pair
    /// into one this server can read: the accepted root that stands for
    /// `origin_root`, and `path` rebased inside it.
    ///
    /// # Why this is a call and not a client-side helper
    ///
    /// Because the rebase needs a fact only the server holds. An offer
    /// grants a **subtree** — offer `Audio Files` and the receiver's root
    /// begins there, so the manifest's `Audio Files/lead-vocal.wav` is
    /// `lead-vocal.wav` inside it. [`Remote`] does not carry the offered
    /// path, so an app working from [`Self::remotes`] and
    /// [`adopted_from`] could not rebase at all, and two offers of
    /// different subtrees of one root would both match with the first one
    /// winning — handing back a file from the wrong folder. This answers
    /// with the most specific offer that actually contains `path`.
    ///
    /// Every app needs it the same way — Signal loading a sample's audio,
    /// Session a take, Keyflow nothing yet — and a browser app reaches it
    /// with one call through `task-dial` instead of reimplementing it.
    ///
    /// # `None` is an answer
    ///
    /// No accepted offer of that root, or none whose subtree holds `path`:
    /// the reference is a valid address that is not reachable from here,
    /// and the fix is an offer rather than a retry. Not an error, and
    /// deliberately not "not found" — the file is exactly where the
    /// manifest says, on a server this org has not been let into.
    async fn resolve_content(
        &self,
        origin_root: RootId,
        path: RootPath,
    ) -> Result<Option<LocalContent>, FilesFault>;

    /// Stop tracking a remote. The origin keeps its content; this only
    /// forgets the way back to it.
    async fn forget(&self, root_id: RootId) -> Result<(), FilesFault>;

    /// Answer a receiver's call: mint a byte ticket inside an offered
    /// subtree.
    ///
    /// The origin side of a read. The receiver gets length and content
    /// type so it can answer its own caller's `read` truthfully, plus a
    /// token to pull against — it does not get a path it could redeem
    /// for anything else.
    ///
    /// The ticket is the origin's, redeemable only at the origin. That
    /// is why this is paired with [`Self::fetch_offered`] rather than
    /// handed onward: `files.peering.serving` says a host without the
    /// content *fetches* it, and passing the token to the receiver's
    /// caller would make federated media a download link to another
    /// server, which is the outcome `files.topology.federation` refuses.
    async fn read_offered(&self, secret: String, path: RootPath) -> Result<ByteTicket, FilesFault>;

    /// Serve one bounded chunk of a ticket minted by
    /// [`Self::read_offered`].
    ///
    /// Bounded because the receiver is relaying: an unbounded read would
    /// put a 244 GB object through its memory on the way past, which is
    /// the allocation failure the whole ticket design exists to avoid.
    /// The secret is re-checked here, so a revocation lands mid-transfer
    /// rather than at the next file.
    async fn fetch_offered(
        &self,
        secret: String,
        token: String,
        range: ByteRange,
    ) -> Result<Vec<u8>, FilesFault>;

    /// Answer a receiver's call: list a path inside an offered subtree.
    ///
    /// The origin side of a browse. Not a public browse — the secret is
    /// checked, and the path is resolved *inside* the offered subtree,
    /// so a receiver cannot walk out of what it was given.
    async fn browse_offered(
        &self,
        secret: String,
        path: RootPath,
    ) -> Result<Vec<crate::model::BrowseEntry>, FilesFault>;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(origin_root: RootId, local: RootId) -> Remote {
        Remote {
            root_id: local,
            origin: EndpointId("peer".into()),
            origin_root,
            name: "Session".into(),
            capabilities: vec![Capability::Read],
            reachable: true,
            accepted_at: Utc::now(),
        }
    }

    #[test]
    fn a_foreign_root_resolves_to_the_offer_accepted_for_it() {
        let theirs = RootId::generate();
        let ours = RootId::generate();
        // A second remote, so the lookup has to match on `origin_root`
        // rather than just returning whatever it holds.
        let others = remote(RootId::generate(), RootId::generate());
        let remotes = vec![others, remote(theirs, ours)];

        assert_eq!(adopted_from(&remotes, theirs), Some(ours));
    }

    /// Unaccepted is `None`, and specifically not the local id that
    /// happens to be spelled the same way.
    ///
    /// The failure this rules out: treating a foreign `root_id` as local
    /// because both are `RootId`s and nothing in the type says which
    /// server minted it. That resolves to "no such root" — a missing
    /// file, when the truth is an unaccepted offer.
    #[test]
    fn a_root_nobody_offered_is_not_found_rather_than_guessed() {
        let ours = RootId::generate();
        let remotes = vec![remote(RootId::generate(), ours)];

        assert_eq!(adopted_from(&remotes, RootId::generate()), None);
        assert_eq!(
            adopted_from(&remotes, ours),
            None,
            "matched on the local id: a root we accepted is not the same \
             root as the one it stands for, and confusing them would make \
             a local id resolve a reference that names somebody else's"
        );
    }

    #[test]
    fn no_remotes_is_none_not_a_panic() {
        assert_eq!(adopted_from(&[], RootId::generate()), None);
    }

    fn path(p: &str) -> RootPath {
        RootPath::parse(p).unwrap()
    }

    fn offered(origin_root: RootId, subtree: &str, local_root: RootId) -> AcceptedSubtree {
        AcceptedSubtree {
            origin_root,
            offered: path(subtree),
            local_root,
        }
    }

    /// The rebase: the offer began at `Audio Files`, so that is where the
    /// receiver's root begins, and the manifest's path loses that prefix.
    #[test]
    fn a_path_inside_the_offered_subtree_is_rebased_onto_the_accepted_root() {
        let theirs = RootId::generate();
        let ours = RootId::generate();
        let resolved = resolve_content_ref(
            [offered(theirs, "Audio Files", ours)],
            theirs,
            &path("Audio Files/lead-vocal.wav"),
        );
        assert_eq!(
            resolved,
            Some(LocalContent {
                root_id: ours,
                path: path("lead-vocal.wav")
            })
        );
    }

    /// Outside the offered subtree is unreachable — including the folder
    /// *beside* it whose name merely starts the same way, which is what a
    /// string strip would have handed over.
    #[test]
    fn a_path_outside_the_offered_subtree_is_unreachable_not_rebased() {
        let theirs = RootId::generate();
        let ours = RootId::generate();
        let accepted = [offered(theirs, "Audio Files", ours)];
        for outside in ["Session.rpp", "Audio Files 2/take.wav", "Bounces/mix.wav"] {
            assert_eq!(
                resolve_content_ref(accepted.clone(), theirs, &path(outside)),
                None,
                "`{outside}` was resolved into an offer that does not contain it"
            );
        }
    }

    /// Nested offers: the narrower grant is the one meant for the file.
    /// Two offers of one root both match the same path, and picking the
    /// first — what `adopted_from` would do — could send a read through
    /// the wider grant with a path that belongs to the narrower one.
    #[test]
    fn the_most_specific_offer_that_contains_the_path_wins() {
        let theirs = RootId::generate();
        let whole = RootId::generate();
        let kicks = RootId::generate();
        let accepted = [
            offered(theirs, "", whole),
            offered(theirs, "Samples/Kicks", kicks),
        ];
        assert_eq!(
            resolve_content_ref(accepted.clone(), theirs, &path("Samples/Kicks/room.wav")),
            Some(LocalContent {
                root_id: kicks,
                path: path("room.wav")
            })
        );
        // And a file only the wider grant covers still resolves through it.
        assert_eq!(
            resolve_content_ref(accepted, theirs, &path("Samples/Snares/crack.wav")),
            Some(LocalContent {
                root_id: whole,
                path: path("Samples/Snares/crack.wav")
            })
        );
    }

    /// A different origin root with an identical subtree name is not a
    /// match. Offers are scoped to the root they were minted on.
    #[test]
    fn an_offer_of_another_root_never_resolves_this_one() {
        let theirs = RootId::generate();
        let unrelated = RootId::generate();
        assert_eq!(
            resolve_content_ref(
                [offered(unrelated, "Audio Files", RootId::generate())],
                theirs,
                &path("Audio Files/lead-vocal.wav"),
            ),
            None
        );
    }
}
