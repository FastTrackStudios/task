//! `RepoConnections` — the repos an org has *connected* (bound to a
//! project), as opposed to every repo the forge token can address
//! (that's [`crate::repo::RepoCatalog`]).
//!
//! Backed server-side by the `git-config` binding store
//! (`issue-links.json`), so the `/repos` view can show only the repos
//! deliberately wired up for sync.

use crate::{GitError, RepoId};

#[architect::rpc]
pub trait RepoConnections {
    /// Distinct repos bound to a project in this org.
    async fn list_connected_repos(&self) -> Result<Vec<RepoId>, GitError>;

    /// Repos bound to a specific project. `project_id` is the project's
    /// stable id as a string (the binding store keys on it opaquely),
    /// so this crate needn't depend on `uuid`.
    async fn repos_for_project(&self, project_id: String) -> Result<Vec<RepoId>, GitError>;
}
