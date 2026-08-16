//! Who may see and change what — `files.access.*`.
//!
//! v1 could share outward and not inward: public links existed, and the
//! only way to give a colleague a folder was to send them one. This lane
//! is the inward half.
//!
//! Capabilities are the primitive and roles are named bundles over them —
//! never something the backend checks directly. Authorisation is
//! evaluated at the path being acted on, not only at the root, and
//! outside a granted subtree paths are **absent** rather than
//! visible-but-forbidden.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::{GrantId, PrincipalId, RootId, ShareId};
use crate::path::RootPath;

/// One thing a principal may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum Capability {
    Read,
    Write,
    /// Read history and restore.
    History,
    Comment,
    Download,
    /// Upload into a drop area without reading what is there.
    Deposit,
    /// Grant to others, within one's own capabilities.
    Share,
}

/// Who a grant is for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum Subject {
    Person(PrincipalId),
    Team(PrincipalId),
    /// A principal resident on another server. Access derives from the
    /// grant on the content, not from membership of whichever org holds
    /// it — which is what lets two companies collaborate without either
    /// joining the other.
    Remote { server: String, principal: String },
}

/// A grant of capabilities over a path.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Grant {
    pub id: GrantId,
    pub subject: Subject,
    pub root_id: RootId,
    /// The subtree. A grant here says nothing about parents above it.
    pub path: RootPath,
    pub capabilities: Vec<Capability>,
    pub granted_by: PrincipalId,
    pub granted_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
}

/// A public link — the outward lane, unchanged in shape from v1.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ShareLink {
    pub id: ShareId,
    pub root_id: RootId,
    pub path: RootPath,
    pub capabilities: Vec<Capability>,
    pub token: String,
    pub password_set: bool,
    pub expires_at: Option<DateTime<Utc>>,
    /// Reversible: disabling is not deleting, so a link can be paused
    /// and resumed without reissuing it to everyone.
    pub disabled: bool,
}

/// What the caller may do at a path — one answer rather than a rules
/// dump, so a UI can enable and disable without re-deriving policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Effective {
    pub root_id: RootId,
    pub path: RootPath,
    pub capabilities: Vec<Capability>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum AccessEvent {
    Granted(Grant),
    /// Revoked. Takes effect on the next request.
    Revoked(GrantId),
    ShareCreated(ShareLink),
    ShareChanged(ShareLink),
    ShareRevoked(ShareId),
}

#[architect::rpc]
pub trait AccessService {
    /// Give a person or team capabilities over a path, without minting a
    /// public link.
    async fn grant(
        &self,
        subject: Subject,
        root_id: RootId,
        path: RootPath,
        capabilities: Vec<Capability>,
    ) -> Result<Grant, FilesFault>;

    /// Withdraw a grant. Effective on the subject's next request.
    async fn revoke(&self, grant: GrantId) -> Result<(), FilesFault>;

    /// Grants over a path, or over a whole root.
    async fn grants(
        &self,
        root_id: RootId,
        path: Option<RootPath>,
    ) -> Result<Vec<Grant>, FilesFault>;

    /// What the caller may do here, resolved. The question a UI actually
    /// has.
    async fn effective(&self, root_id: RootId, path: RootPath)
    -> Result<Effective, FilesFault>;

    /// Mint a public link.
    async fn create_share(
        &self,
        root_id: RootId,
        path: RootPath,
        capabilities: Vec<Capability>,
    ) -> Result<ShareLink, FilesFault>;

    /// Disable or re-enable a link without reissuing it.
    async fn set_share_disabled(
        &self,
        share: ShareId,
        disabled: bool,
    ) -> Result<ShareLink, FilesFault>;

    /// Links issued from this root.
    async fn shares(&self, root_id: RootId) -> Result<Vec<ShareLink>, FilesFault>;
}
