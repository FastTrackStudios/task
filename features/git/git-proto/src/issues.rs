//! `IssueTracker` — issue CRUD, comments, labels, assignees,
//! milestones, state transitions, live subscription.

use crate::{GitError, GitEvent, IssueId, IssueState, Label, Milestone, RepoId, User};
use facet::Facet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct Issue {
    pub id: IssueId,
    pub repo: RepoId,
    pub title: String,
    pub body: String,
    pub state: IssueState,
    pub author: User,
    pub labels: Vec<Label>,
    pub assignees: Vec<User>,
    pub milestone: Option<Milestone>,
    /// Forge-reported last-update time, RFC-3339. `None` when the
    /// backend doesn't surface it. Carried as a string (not a
    /// `chrono` type) to keep the DTO `Facet`-encodable; consumers
    /// that need ordering parse it. Drives the sync fast-path — an
    /// issue whose `updated_at` hasn't advanced since the last
    /// reconcile couldn't have changed on the forge.
    #[serde(default)]
    pub updated_at: Option<String>,
}

#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct Comment {
    /// Forge-assigned comment id, opaque string so backends
    /// don't have to agree on numeric vs uuid shape.
    pub id: String,
    pub author: User,
    pub body: String,
}

/// Filter for `list_issues`. Each field is "all" when `None` /
/// empty. Backends best-effort: a forge that can't filter by
/// label server-side filters client-side.
#[derive(Debug, Clone, Default, Facet, Serialize, Deserialize)]
pub struct IssueFilter {
    pub state: Option<IssueState>,
    pub labels: Vec<String>,
    pub assignee: Option<String>,
    pub milestone: Option<u64>,
    pub author: Option<String>,
}

/// Partial update — `None` fields are left untouched. Backends
/// translate to a single PATCH where possible, multiple calls
/// where not.
#[derive(Debug, Clone, Default, Facet, Serialize, Deserialize)]
pub struct IssueUpdate {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<IssueState>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
    pub milestone: Option<Option<u64>>,
}

#[architect::rpc]
pub trait IssueTracker {
    fn list_issues(&self, repo: &RepoId, filter: IssueFilter) -> Result<Vec<Issue>, GitError>;

    fn get_issue(&self, repo: &RepoId, issue: IssueId) -> Result<Issue, GitError>;

    fn create_issue(&self, repo: &RepoId, title: String, body: String) -> Result<Issue, GitError>;

    fn update_issue(
        &self,
        repo: &RepoId,
        issue: IssueId,
        update: IssueUpdate,
    ) -> Result<Issue, GitError>;

    fn list_comments(&self, repo: &RepoId, issue: IssueId) -> Result<Vec<Comment>, GitError>;

    fn add_comment(&self, repo: &RepoId, issue: IssueId, body: String)
    -> Result<Comment, GitError>;

    /// Every issue change this backend commits, as it happens —
    /// create / update / comment. Unfiltered across repos: streams
    /// take no params, and every [`GitEvent`] already names its
    /// `repo`, so a page watching one repo filters client-side.
    ///
    /// ## Subscriber contract (changes only, no snapshot variant)
    ///
    /// Events name *what* changed, not the new value — an
    /// [`Issue`] is a forge read, not something the event can
    /// carry authoritatively. A subscriber lists once
    /// ([`Self::list_issues`], after subscribing so nothing is
    /// missed in between) and re-reads what an event touches:
    /// `IssueCreated` / `IssueUpdated` / `IssueCommented` for its
    /// repo mean "that row is stale", [`GitEvent::Resync`] means
    /// "all of them are".
    #[subscribe]
    fn issue_events(&self) -> GitEvent;
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Comment, Issue, IssueFilter, IssueUpdate};
    unsafe impl vox_types::Reborrow for Issue {
        type Ref<'a> = Issue;
    }
    unsafe impl vox_types::Reborrow for Comment {
        type Ref<'a> = Comment;
    }
    unsafe impl vox_types::Reborrow for IssueFilter {
        type Ref<'a> = IssueFilter;
    }
    unsafe impl vox_types::Reborrow for IssueUpdate {
        type Ref<'a> = IssueUpdate;
    }
}

#[cfg(feature = "vox")]
pub use self::IssueTrackerClient as Client;
