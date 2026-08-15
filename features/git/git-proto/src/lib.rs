//! `git-proto` — wire contract for the git feature.
//!
//! Three sibling `#[architect::rpc]` traits live in their own
//! modules:
//!
//! - [`repo::RepoCatalog`] — list/get repos. Repo-level metadata
//!   that bindings depend on.
//! - [`issues::IssueTracker`] — issue CRUD, comments, labels,
//!   assignees, milestones, state transitions.
//! - [`reviews::ReviewSurface`] — pull-request CRUD, review
//!   threads, requested reviewers, merge.
//!
//! Splitting the surface (vs one monolithic trait like
//! `EmailSync`) lets a backend implement partial support — e.g.
//! a future read-only mirror provides reads on `IssueTracker`
//! and skips `ReviewSurface` entirely.
//!
//! Each trait module re-exports its own `Client`, `Service`,
//! `serve`, `descriptor` so consumers reach them as
//! `git_proto::repo::serve`, `git_proto::issues::Client`, etc.
//! No symbol collisions across the three traits.
//!
//! Backends live in sibling crates (`git-github` ships first,
//! `git-forgejo` next) and impl whichever traits the forge
//! supports.
//!
//! Mirrors the shape of `email-proto` / `vault-proto`.

pub mod connections;
pub mod error;
pub mod event;
pub mod issues;
pub mod payload;
pub mod repo;
pub mod reviews;

pub use error::GitError;
pub use event::GitEvent;
pub use payload::{
    Forge, IssueId, IssueState, Label, Milestone, PullRequest, PullRequestId, PullRequestState,
    Repo, RepoId, Reviewer, User,
};

// Issue + Comment live with `IssueTracker` because they're its
// primary payload.
pub use issues::{Comment, Issue, IssueFilter, IssueUpdate};
pub use reviews::{MergeMethod, NewPullRequest, PullRequestUpdate, Review, ReviewState};
