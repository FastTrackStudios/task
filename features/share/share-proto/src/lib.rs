//! Wire contract for the `share` feature — sharing via tracked,
//! individually-configurable links (Samply-style: nothing is
//! fire-and-forget; every link can be edited, disabled, or deleted after
//! creation and the change is retroactive).
//!
//! Targets grew past notes with the Files platform (issue #271): a link
//! can now share a **Root slice** (browse exactly that subtree) or a
//! **Named Version** (the exact curated change). Capabilities are axes
//! (`view` is implicit; `comment` and `download` opt in), links can carry
//! a password and an expiry, minting has an org kill switch, and every
//! resolution is written to a per-link access log — downloads as
//! receipts.
//!
//! The scoped guest lane (`/org/{slug}/share/{token}/vox`) and the
//! file-request inbox arrive with issue #272.

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a link points at.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum ShareTarget {
    /// A vault note (the original target).
    Note { path: String },
    /// A File Root slice — `(root, subpath)`; empty subpath shares the
    /// whole root. The link browses exactly this subtree, nothing above
    /// or beside it (issue #271 AC 1).
    Slice { root_id: Uuid, subpath: String },
    /// A curated Named Version (issue #261 entity) — resolves to the
    /// exact change it names, however the root has moved since.
    NamedVersion { id: Uuid },
    /// A Files Review (issue #270 entity): the guest lane (issue #272)
    /// puts an anonymous visitor in the review — playback, comments,
    /// drawings — scoped to exactly that review's file.
    Review { id: Uuid },
}

/// Capability axes on a link. `view` is what a link IS — the axes are
/// the two opt-ins. Edit is invite-only, never link-based.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ShareCapabilities {
    /// May leave comments (rides the guest lane, issue #272).
    pub comment: bool,
    /// May download originals. Without it a link is view-only: proxy
    /// renditions serve, original bytes never do (issue #271 AC 3).
    pub download: bool,
    /// May upload into the link's per-token incoming area (issue #272:
    /// the file-request inbox) — never into the tree itself; the owner
    /// promotes uploads in.
    #[serde(default)]
    pub file_request: bool,
}

/// The mint/edit options for a link, bundled (RPC methods carry at most
/// 4 params).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct NewShareLink {
    /// Human label ("band link", "client cut").
    pub label: String,
    /// `None` on update keeps the current capabilities (a partial edit
    /// must not silently rewrite the download grant); `None` on create
    /// mints view-only.
    pub capabilities: Option<ShareCapabilities>,
    /// Plaintext over the (TLS) wire; stored hashed, never returned.
    /// `None` on update keeps the current password; `Some("")` clears it.
    pub password: Option<String>,
    /// Unix seconds after which the link stops resolving. `None` on
    /// update keeps the current expiry; `Some(0)` clears it. Negative
    /// values are refused.
    pub expires_unix: Option<i64>,
}

/// One share link, as the panel and the Links registry render it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ShareLinkInfo {
    /// The unguessable URL token (also the link's id).
    pub token: String,
    pub label: String,
    pub target: ShareTarget,
    pub capabilities: ShareCapabilities,
    /// Whether a password gates resolution (the hash never leaves the
    /// server).
    pub password_protected: bool,
    /// Unix seconds after which the link stops resolving; 0 = never.
    pub expires_unix: i64,
    /// Reversible off-switch — a disabled link 410s without being deleted.
    pub disabled: bool,
    /// Absolute URL to hand out (server composes it from its public base).
    pub url: String,
    /// RFC3339 creation stamp.
    pub created_at: String,
}

/// One access-log entry. Downloads are the receipts the spec asks for;
/// views/browses/rendition streams are the ordinary rows.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct ShareAccess {
    /// RFC3339 stamp.
    pub at: String,
    /// `view` (landing) | `browse` | `rendition` | `download`.
    pub kind: String,
    /// Target-relative path touched (empty for the landing view).
    pub path: String,
}

/// One upload sitting in a file-request link's incoming area (issue
/// #272 AC 3), waiting for the owner to promote it into the tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct IncomingFile {
    /// Name inside the link's incoming area (uploads never overwrite —
    /// collisions get suffixed).
    pub name: String,
    pub size: u64,
    /// RFC3339 upload stamp (filesystem mtime).
    pub uploaded_at: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Facet, thiserror::Error)]
#[repr(u8)]
pub enum ShareError {
    #[error("share not found")]
    NotFound,
    #[error("share storage error: {0}")]
    Storage(String),
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("sharing is disabled for this org")]
    SharingDisabled,
}

#[architect::rpc]
pub trait ShareService {
    /// Mint a new link for `target`. Refused while the org kill switch
    /// ([`ShareService::set_sharing_disabled`]) is on.
    async fn create_link(
        &self,
        target: ShareTarget,
        options: NewShareLink,
    ) -> Result<ShareLinkInfo, ShareError>;

    /// Retroactively edit a link's label / capabilities / password /
    /// expiry (issue #271 AC 5) — the next resolution sees the change.
    async fn update_link(
        &self,
        token: String,
        options: NewShareLink,
    ) -> Result<ShareLinkInfo, ShareError>;

    /// Every link in the org (the Links registry), newest first.
    async fn list_links(&self) -> Result<Vec<ShareLinkInfo>, ShareError>;

    /// Links for ONE target (a note's Share panel, a slice's).
    async fn links_for_target(&self, target: ShareTarget)
    -> Result<Vec<ShareLinkInfo>, ShareError>;

    /// Disable (reversible) or re-enable a link — retroactive: a disabled
    /// link stops resolving immediately.
    async fn set_link_disabled(&self, token: String, disabled: bool) -> Result<(), ShareError>;

    /// Delete a link permanently.
    async fn delete_link(&self, token: String) -> Result<(), ShareError>;

    /// One link's access log, newest first — views, browses, rendition
    /// streams, and download receipts (issue #271 AC 4).
    async fn access_log(&self, token: String) -> Result<Vec<ShareAccess>, ShareError>;

    /// The org kill switch: while on, no new link can be minted
    /// (existing links keep resolving — revoke them individually).
    async fn set_sharing_disabled(&self, disabled: bool) -> Result<(), ShareError>;

    async fn sharing_disabled(&self) -> Result<bool, ShareError>;

    /// A file-request link's incoming uploads (issue #272 AC 3),
    /// newest first — the owner's review queue.
    async fn list_incoming(&self, token: String) -> Result<Vec<IncomingFile>, ShareError>;

    /// Promote one incoming upload into the link's root at
    /// `dest_path` (root-relative). Never overwrites; the versioning
    /// cadence captures the arrival like any other save.
    async fn promote_incoming(
        &self,
        token: String,
        name: String,
        dest_path: String,
    ) -> Result<(), ShareError>;
}
