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
use serde::{Deserialize, Serialize};

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
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
#[serde(transparent)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
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

/// A bounded window of an object, for a relayed read.
///
/// Half-open: `[offset, offset + len)`. `len` is what keeps a relay's
/// memory flat regardless of how large the object is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
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
    async fn read_offered(
        &self,
        secret: String,
        path: RootPath,
    ) -> Result<ByteTicket, FilesFault>;

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
