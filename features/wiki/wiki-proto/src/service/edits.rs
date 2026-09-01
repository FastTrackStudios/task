//! Edit Requests — how someone without Editor changes a wiki.
//!
//! The unit of contribution is one proposed change to one wiki
//! (`wiki.edit.request`): the edited pages themselves, against the
//! version the proposer saw, not a message describing them. Opening
//! one never mutates the wiki; only an Editor's acceptance does
//! (`wiki.edit.gate`), and that acceptance lands as a version
//! attributed to the proposer (`wiki.edit.reviewable`).
//!
//! An Edit Request *is* an issue on the owning org's tracker
//! (`wiki.edit.tracked`): the backend opens a row there when the request
//! is opened and closes it when the request resolves, and reads the
//! row's status back so a request closed from the issue surface is
//! closed here. What the tracker cannot hold — the change itself, the
//! claim, the diff — lives with the wiki. The **Edit Tracker** is the
//! view over both.
//!
//! Editor is a role on one wiki (`wiki.edit.editor`), held in the wiki's
//! config, and this is also where it is granted and read.

use crate::config::ProposerGate;
use crate::error::WikiError;

/// One page in a change.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct PageChange {
    /// Wiki-relative path, `Concepts/Ionian.md`.
    pub path: String,
    /// The sha-256 of the page as the proposer saw it. Empty for a page
    /// they are creating. This is the "named version" the request is
    /// against, and what makes staleness detectable (`wiki.edit.rebase`).
    #[serde(default)]
    pub base_sha256: String,
    /// The page as the proposer saw it, so a stale request can be
    /// merged three ways rather than refused two ways. Empty for a new
    /// page.
    #[serde(default)]
    pub base_markdown: String,
    /// The proposed content. Ignored when `delete` is set.
    #[serde(default)]
    pub markdown: String,
    /// Propose removing the page instead.
    #[serde(default)]
    pub delete: bool,
}

/// Where a request is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum EditStatus {
    /// Waiting for an Editor. May be claimed.
    Open,
    /// Sent back to the proposer with a reason; they may revise and it
    /// returns to `Open`.
    Returned,
    /// Landed in the wiki's history.
    Accepted,
    /// An Editor accepted it and the repository has not yet taken it.
    /// Only a repo-sourced wiki reports this (`wiki.source.editable`):
    /// the wiki does not show a change as landed until the repository
    /// has it.
    Landing,
    /// Declined. The wiki is byte-identical to before it was opened
    /// and the request keeps its text.
    Rejected,
    /// The issue was closed from the tracker without an Editor
    /// accepting. Nothing landed; the text is kept.
    Closed,
}

impl EditStatus {
    /// Whether the request is still waiting on somebody.
    #[must_use]
    pub const fn is_open(self) -> bool {
        matches!(self, Self::Open | Self::Returned)
    }

    /// The word a person reads.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Returned => "returned",
            Self::Accepted => "accepted",
            Self::Landing => "landing",
            Self::Rejected => "rejected",
            Self::Closed => "closed",
        }
    }
}

/// One Edit Request.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct EditRequest {
    /// The same id as the tracker row: one thing, two views.
    pub id: uuid::Uuid,
    /// The wiki it is against.
    pub wiki: String,
    pub title: String,
    /// What the proposer said about it. Free text.
    #[serde(default)]
    pub summary: String,
    /// The account that opened it, as the gate resolved it.
    pub proposer: String,
    /// RFC 3339.
    pub opened_at: String,
    pub status: EditStatus,
    /// Who resolved it, when, and what they said. Empty while open.
    #[serde(default)]
    pub resolved_by: String,
    #[serde(default)]
    pub resolved_at: String,
    #[serde(default)]
    pub resolution: String,
    /// The Editor reviewing it, and until when (`wiki.edit.claim`).
    /// Empty when unclaimed or the claim has expired.
    #[serde(default)]
    pub claimed_by: String,
    #[serde(default)]
    pub claimed_until: String,
    /// True when an Editor's own change went through the lane and was
    /// approved within it (`wiki.edit.auto-approve`).
    #[serde(default)]
    pub auto_approved: bool,
    /// Set when the proposer is not someone the wiki vouches for
    /// (`wiki.edit.gate`). The request exists and Editors can see it;
    /// it is never published on their behalf.
    #[serde(default)]
    pub held: bool,
    /// Where the change went on acceptance in a repo-sourced wiki — a
    /// pull request URL or commit. Empty otherwise.
    #[serde(default)]
    pub landing: String,
    pub changes: Vec<PageChange>,
}

/// What a proposer sends.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct NewEditRequest {
    pub title: String,
    #[serde(default)]
    pub summary: String,
    pub changes: Vec<PageChange>,
    /// An Editor asking for review rather than the automatic approval
    /// they would otherwise get. Ignored for anyone else.
    #[serde(default)]
    pub request_review: bool,
}

/// One page of a request, as a reviewer sees it (`wiki.edit.reviewable`).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct PageDiff {
    pub path: String,
    /// The page now. Empty when it does not exist.
    pub current: String,
    /// What the proposer wants it to say. Empty for a deletion.
    pub proposed: String,
    /// Whether the page has changed since the proposer's base, and
    /// how it would go: `applies` means the request still lands
    /// cleanly, three-way merged if need be; otherwise the two
    /// touched the same lines and a person decides
    /// (`wiki.edit.rebase`).
    pub stale: bool,
    pub applies: bool,
    /// The content that would land: the proposal, or the three-way
    /// merge of it over the current page. Empty when it does not
    /// apply.
    #[serde(default)]
    pub merged: String,
}

/// Who reviews, and who may propose.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "vox", derive(facet::Facet))]
#[repr(C)]
pub struct Editors {
    /// Visible to a contributor before they open a request
    /// (`wiki.edit.editor`).
    pub editors: Vec<String>,
    pub gate: ProposerGate,
}

#[architect::rpc]
pub trait Edits {
    /// Propose a change. Any account that may read the wiki may call
    /// this; an Editor's own call is approved within the lane unless
    /// they ask for review.
    fn open_edit_request(
        &self,
        wiki_id: &str,
        request: NewEditRequest,
    ) -> Result<EditRequest, WikiError>;

    /// Requests against one wiki. Open ones by default; everything
    /// with `include_resolved`, which is the "every change and who
    /// made it" query.
    fn list_edit_requests(
        &self,
        wiki_id: &str,
        include_resolved: bool,
    ) -> Result<Vec<EditRequest>, WikiError>;

    fn get_edit_request(&self, wiki_id: &str, id: uuid::Uuid) -> Result<EditRequest, WikiError>;

    /// The request as a diff against the current pages.
    fn diff_edit_request(&self, wiki_id: &str, id: uuid::Uuid) -> Result<Vec<PageDiff>, WikiError>;

    /// The proposer replaces the change — after a return, or because
    /// they changed their mind. Reopens a returned request.
    fn revise_edit_request(
        &self,
        wiki_id: &str,
        id: uuid::Uuid,
        changes: Vec<PageChange>,
    ) -> Result<EditRequest, WikiError>;

    /// An Editor takes the request to review it. Refused while another
    /// Editor's claim stands; a claim expires on its own
    /// (`wiki.edit.claim`).
    fn claim_edit_request(&self, wiki_id: &str, id: uuid::Uuid) -> Result<EditRequest, WikiError>;

    /// Give a claim back early.
    fn release_edit_request(&self, wiki_id: &str, id: uuid::Uuid)
    -> Result<EditRequest, WikiError>;

    /// Land it. Editor only. A stale request that still applies is
    /// merged and landed; one that conflicts is refused with the
    /// conflicting pages named, and stays open.
    fn accept_edit_request(&self, wiki_id: &str, id: uuid::Uuid) -> Result<EditRequest, WikiError>;

    /// Decline it. Editor only. The wiki is untouched.
    fn reject_edit_request(
        &self,
        wiki_id: &str,
        id: uuid::Uuid,
        reason: &str,
    ) -> Result<EditRequest, WikiError>;

    /// Send it back for changes. Editor only.
    fn return_edit_request(
        &self,
        wiki_id: &str,
        id: uuid::Uuid,
        reason: &str,
    ) -> Result<EditRequest, WikiError>;

    /// Who will review a request, and who may open one.
    fn editors(&self, wiki_id: &str) -> Result<Editors, WikiError>;

    /// Grant Editor. An existing Editor or an org admin may call this.
    fn grant_editor(&self, wiki_id: &str, principal: &str) -> Result<(), WikiError>;

    /// Revoke Editor. The last Editor cannot be revoked — a wiki that
    /// adopted the lane cannot be left with nobody able to land.
    fn revoke_editor(&self, wiki_id: &str, principal: &str) -> Result<(), WikiError>;

    /// Declare who may propose (`wiki.edit.gate`). Editor only.
    fn set_proposer_gate(&self, wiki_id: &str, gate: ProposerGate) -> Result<(), WikiError>;
}
