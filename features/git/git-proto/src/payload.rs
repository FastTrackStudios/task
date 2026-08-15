//! Cross-trait payload types — `Repo`, `User`, `Label`,
//! `Milestone`, `PullRequest` summary, and the typed `Forge`
//! enum that tells the dispatcher which backend serves a given
//! lookup.
//!
//! Issue / Comment live in [`crate::issues`] because they're
//! the primary payload of `IssueTracker`; `Review` lives in
//! [`crate::reviews`] for the same reason.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Which forge a binding points at. The dispatcher inspects
/// this to pick a backend; the backend uses `base_url` (for
/// self-hosted Forgejo) to build the API client.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum Forge {
    /// github.com.
    Github,
    /// Self-hosted Forgejo (or compatible Gitea). `base_url`
    /// is the API root, e.g. `https://git.example.org`.
    Forgejo { base_url: String },
}

/// `(forge, owner, repo)` triple. Stable address used by every
/// trait method and by `git-config` bindings.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
pub struct RepoId {
    pub forge: Forge,
    pub owner: String,
    pub repo: String,
}

/// Repo metadata the catalog returns. Intentionally a small
/// common subset — backend-specific fields (GitHub topics,
/// Forgejo mirror flags) are deferred until a caller actually
/// needs them.
#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct Repo {
    pub id: RepoId,
    pub description: Option<String>,
    pub default_branch: String,
    pub private: bool,
    pub archived: bool,
}

/// Opaque issue number on a given repo. Forge-assigned;
/// `IssueTracker` callers pass these around verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
pub struct IssueId(pub u64);

/// Issue state subset shared by every forge. Forge-specific
/// reasons (e.g. GitHub's `closed_as: completed | not_planned`)
/// are deferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum IssueState {
    Open,
    Closed,
}

/// Same as `IssueId` but typed differently so the compiler
/// stops you from passing an issue number where a PR number
/// is expected. On GitHub the numbers share a namespace; on
/// Forgejo they don't — typing the distinction up front saves
/// a class of bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
pub struct PullRequestId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum PullRequestState {
    Open,
    Closed,
    Merged,
}

/// Forge user. `login` is the stable identifier; `display_name`
/// is the rendered form (may equal `login`).
#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct User {
    pub login: String,
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct Label {
    pub name: String,
    /// 6-char hex without leading `#`. `None` means the forge
    /// didn't supply one.
    pub color: Option<String>,
}

#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct Milestone {
    pub title: String,
    pub number: u64,
}

#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
pub struct PullRequest {
    pub id: PullRequestId,
    pub repo: RepoId,
    pub title: String,
    pub body: String,
    pub state: PullRequestState,
    pub author: User,
    pub base: String,
    pub head: String,
    pub draft: bool,
    pub labels: Vec<Label>,
    pub assignees: Vec<User>,
    pub requested_reviewers: Vec<Reviewer>,
}

/// Reviewer request target. Forges distinguish user vs team
/// reviewers; we carry the distinction across the wire.
#[derive(Debug, Clone, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum Reviewer {
    User(User),
    /// `slug` is the forge-native team identifier.
    Team {
        slug: String,
    },
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{
        Forge, IssueId, IssueState, Label, Milestone, PullRequest, PullRequestId, PullRequestState,
        Repo, RepoId, Reviewer, User,
    };
    macro_rules! reborrow_owned {
        ($($t:ty),* $(,)?) => {
            $(
                unsafe impl vox_types::Reborrow for $t {
                    type Ref<'a> = $t;
                }
            )*
        };
    }
    reborrow_owned!(
        Forge,
        RepoId,
        Repo,
        IssueId,
        IssueState,
        PullRequestId,
        PullRequestState,
        User,
        Label,
        Milestone,
        PullRequest,
        Reviewer,
    );
}
