//! Server-side `RepoConnections` — the repos this org has connected
//! (project-bound), read from the `git-config` binding store
//! (`issue-links.json`). Backs the `/repos` "connected" view, distinct
//! from the raw forge catalog (`RepoCatalog::list_repos`).

use std::path::PathBuf;

use git_config::FileStore;
use git_proto::connections::RepoConnections;
use git_proto::{GitError, RepoId};

#[derive(Clone)]
pub struct ConnectionsBackend {
    issue_links_path: PathBuf,
}

impl ConnectionsBackend {
    #[must_use]
    pub fn new(issue_links_path: PathBuf) -> Self {
        Self { issue_links_path }
    }
}

impl RepoConnections for ConnectionsBackend {
    async fn list_connected_repos(&self) -> Result<Vec<RepoId>, GitError> {
        let store = FileStore::open(&self.issue_links_path)
            .map_err(|e| GitError::Forge(format!("open link store: {e}")))?;
        store
            .binding_repos()
            .map_err(|e| GitError::Forge(format!("binding repos: {e}")))
    }

    async fn repos_for_project(&self, project_id: String) -> Result<Vec<RepoId>, GitError> {
        use git_config::BindingStore as _;
        let store = FileStore::open(&self.issue_links_path)
            .map_err(|e| GitError::Forge(format!("open link store: {e}")))?;
        store
            .repos_for_project(&project_id)
            .map_err(|e| GitError::Forge(format!("repos for project: {e}")))
    }
}
