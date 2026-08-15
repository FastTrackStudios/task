//! `ReviewSurface` — pull-request CRUD, review threads,
//! requested reviewers, merge.

use crate::{
    GitError, GitEvent, PullRequest, PullRequestId, PullRequestState, RepoId, Reviewer, User,
};
use facet::Facet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum ReviewState {
    Pending,
    Approved,
    ChangesRequested,
    Commented,
    Dismissed,
}

#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct Review {
    pub id: String,
    pub author: User,
    pub state: ReviewState,
    pub body: String,
}

/// Merge strategy. Backends translate or reject `Unsupported`
/// when the forge doesn't offer the variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum MergeMethod {
    Merge,
    Squash,
    Rebase,
}

/// Args for `create_pull_request`. Bundled into a struct so
/// the trait method stays under Facet's tuple arity limit and
/// future fields (e.g. `maintainer_can_modify`) don't break the
/// signature.
#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct NewPullRequest {
    pub title: String,
    pub body: String,
    pub base: String,
    pub head: String,
    pub draft: bool,
}

#[derive(Debug, Clone, Default, Facet, Serialize, Deserialize)]
pub struct PullRequestUpdate {
    pub title: Option<String>,
    pub body: Option<String>,
    pub state: Option<PullRequestState>,
    pub draft: Option<bool>,
    pub labels: Option<Vec<String>>,
    pub assignees: Option<Vec<String>>,
}

#[architect::rpc]
pub trait ReviewSurface {
    fn list_pull_requests(&self, repo: &RepoId) -> Result<Vec<PullRequest>, GitError>;

    fn get_pull_request(&self, repo: &RepoId, pr: PullRequestId) -> Result<PullRequest, GitError>;

    fn create_pull_request(
        &self,
        repo: &RepoId,
        new: NewPullRequest,
    ) -> Result<PullRequest, GitError>;

    fn update_pull_request(
        &self,
        repo: &RepoId,
        pr: PullRequestId,
        update: PullRequestUpdate,
    ) -> Result<PullRequest, GitError>;

    fn list_reviews(&self, repo: &RepoId, pr: PullRequestId) -> Result<Vec<Review>, GitError>;

    fn request_reviewers(
        &self,
        repo: &RepoId,
        pr: PullRequestId,
        reviewers: Vec<Reviewer>,
    ) -> Result<(), GitError>;

    /// Merge the PR. Returns the resulting merge commit sha
    /// (or rebased head sha) where the forge supplies one.
    fn merge_pull_request(
        &self,
        repo: &RepoId,
        pr: PullRequestId,
        method: MergeMethod,
    ) -> Result<Option<String>, GitError>;

    /// Every pull-request change this backend commits, as it
    /// happens — open / update / review / merge. Unfiltered across
    /// repos; each [`GitEvent`] names its `repo`. Same
    /// changes-only, re-read-what-it-touches contract as
    /// `IssueTracker`'s stream (issue and PR feeds are separate
    /// hubs, so a reviews subscriber doesn't pay for issue churn).
    #[subscribe]
    fn review_events(&self) -> GitEvent;
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{MergeMethod, NewPullRequest, PullRequestUpdate, Review, ReviewState};
    unsafe impl vox_types::Reborrow for NewPullRequest {
        type Ref<'a> = NewPullRequest;
    }
    unsafe impl vox_types::Reborrow for ReviewState {
        type Ref<'a> = ReviewState;
    }
    unsafe impl vox_types::Reborrow for Review {
        type Ref<'a> = Review;
    }
    unsafe impl vox_types::Reborrow for MergeMethod {
        type Ref<'a> = MergeMethod;
    }
    unsafe impl vox_types::Reborrow for PullRequestUpdate {
        type Ref<'a> = PullRequestUpdate;
    }
}

#[cfg(feature = "vox")]
pub use self::ReviewSurfaceClient as Client;
