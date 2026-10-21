//! Wire contract for cross-org project sharing: the owner's side of a
//! **Sharing Grant** (`docs/spec/sharing.md`).
//!
//! A grant lets ONE person from another org (the subject, an fts-auth
//! user) see one project's subtree in the owning org, read + comment,
//! without a membership row there. This module is the owner's control
//! surface — preview, create, list, revoke — mounted on the owning org's
//! own lane and refused to everyone but an owner or admin of that org
//! (`t[sharing.grant.owner-only]`). The guest's lane that *uses* a grant
//! is a different router (`/org/{owner}/grant/{grant_id}/vox`).
//!
//! Rows are immutable once written, except for the single revoke; a
//! change of scope is a revoke followed by a fresh grant.

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a grant lets its subject do. v1 has exactly one shape; a later
/// `ReadWrite` would be a new variant, never a widening of this one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum GrantCapabilities {
    /// Read the subtree, and comment on objects inside it. Nothing else.
    #[default]
    ReadComment,
}

impl GrantCapabilities {
    /// The stored / audited spelling.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ReadComment => "read_comment",
        }
    }

    /// The inverse of [`Self::as_str`]; `None` for anything unknown,
    /// which a reader must treat as "not live" rather than guess.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "read_comment" => Some(Self::ReadComment),
            _ => None,
        }
    }
}

/// The classes a grant never carries, whatever the subtree holds
/// (`t[sharing.grant.exclusions]`). Shown in every preview so the owner
/// sees what stays home before confirming.
pub const NOT_SHARED: [&str; 4] = ["pricing", "invoices", "timer", "contacts"];

/// The preview's one-line rendering of [`NOT_SHARED`].
#[must_use]
pub fn not_shared_line() -> String {
    format!("not shared: {}", NOT_SHARED.join(", "))
}

/// One object a grant would make visible.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct VisibleItem {
    /// `project` | `part` | `task` | `file_root` | `note`.
    pub kind: String,
    /// The object's id (a uuid, or a vault path for a note).
    pub id: String,
    /// What the owner reads in the preview — a title or a path.
    pub label: String,
}

/// The exact set a grant on `root_id` would expose, and its digest
/// (`t[sharing.grant.preview]`). The owner confirms by sending
/// `digest` back as [`NewSharingGrant::preview_digest`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct GrantPreview {
    pub root_id: Uuid,
    pub subject: Uuid,
    /// Sorted by `(kind, id)`.
    pub visible: Vec<VisibleItem>,
    /// Lower-case hex SHA-256 over the visible set.
    pub digest: String,
    /// [`NOT_SHARED`], always.
    pub not_shared: Vec<String>,
}

/// What an owner asks for. Everything else on the row — id, owner org,
/// who granted it and when — is set by the server from the call itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct NewSharingGrant {
    /// The project whose subtree is shared.
    pub root_id: Uuid,
    /// The person: an fts-auth user uuid. Not a member of the owner org.
    pub subject: Uuid,
    /// The subject's own org (slug), as metadata — where the shared-in
    /// reference shows up. Grants nothing in that org.
    pub guest_org: String,
    pub capabilities: GrantCapabilities,
    /// Unix seconds after which the grant is no longer live. Must be in
    /// the future when given.
    pub expires_at: Option<i64>,
    /// The [`GrantPreview::digest`] the owner confirmed. A grant whose
    /// subtree has changed since the preview is refused.
    pub preview_digest: String,
}

/// One grant row, as the owner's Sharing panel renders it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct SharingGrantInfo {
    pub id: Uuid,
    pub owner_org: String,
    pub root_id: Uuid,
    pub subject: Uuid,
    pub guest_org: String,
    pub capabilities: GrantCapabilities,
    pub expires_at: Option<i64>,
    pub granted_by: Uuid,
    /// Unix seconds.
    pub granted_at: i64,
    pub revoked_at: Option<i64>,
    pub revoked_by: Option<Uuid>,
    pub preview_digest: String,
    /// Unrevoked and unexpired at the moment the server answered.
    pub live: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Facet, thiserror::Error)]
#[repr(u8)]
pub enum SharingError {
    /// The grant, the root — or the whole service, for anyone who is not
    /// an owner or admin of this org. One answer for all three, so a
    /// caller cannot tell "not yours" from "not there"
    /// (`t[sharing.grant.absent]`).
    #[error("not found")]
    NotFound,
    /// The subtree no longer matches the preview the owner confirmed.
    /// `digest` is the current one; preview again before granting.
    #[error("the shared set changed since the preview; review it again")]
    PreviewChanged { digest: String },
    #[error("invalid request: {0}")]
    Invalid(String),
    #[error("sharing storage error: {0}")]
    Storage(String),
}

/// The owner's control surface for Sharing Grants — the `sharing`
/// service on `/org/{slug}/vox`.
#[architect::rpc]
pub trait SharingService {
    /// What a grant on `root_id` for `subject` would expose, and the
    /// digest to confirm it with.
    async fn preview_grant(
        &self,
        root_id: Uuid,
        subject: Uuid,
    ) -> Result<GrantPreview, SharingError>;

    /// Write a grant. Refused unless `preview_digest` matches the
    /// subtree as it is now.
    async fn create_grant(&self, grant: NewSharingGrant) -> Result<SharingGrantInfo, SharingError>;

    /// Revoke a grant: one action, effective on the next request.
    /// Idempotent — revoking a revoked grant returns it unchanged.
    async fn revoke_grant(&self, grant_id: Uuid) -> Result<SharingGrantInfo, SharingError>;

    /// Every grant this org has made, newest first, revoked ones
    /// included.
    async fn list_grants(&self) -> Result<Vec<SharingGrantInfo>, SharingError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_round_trip_and_unknown_is_none() {
        let c = GrantCapabilities::ReadComment;
        assert_eq!(GrantCapabilities::parse(c.as_str()), Some(c));
        assert_eq!(GrantCapabilities::parse("read_write"), None);
    }

    #[test]
    fn the_not_shared_line_names_the_money_classes() {
        assert_eq!(
            not_shared_line(),
            "not shared: pricing, invoices, timer, contacts"
        );
    }
}
