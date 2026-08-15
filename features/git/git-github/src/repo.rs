//! `RepoCatalog` impl. All methods stubbed until the binding +
//! config layer needs them — agent-task and project-dashboard
//! call into `IssueTracker` first.

use crate::Backend;
use git_proto::repo::RepoCatalog;
use git_proto::{GitError, Repo, RepoId};

impl RepoCatalog for Backend {
    fn list_repos(&self) -> Result<Vec<Repo>, GitError> {
        todo!(
            "list_repos: paginate /user/repos via octocrab.current().list_repos_for_authenticated_user()"
        )
    }

    fn get_repo(&self, repo: &RepoId) -> Result<Repo, GitError> {
        Backend::check_forge(repo)?;
        let _ = repo;
        todo!("get_repo: octo.repos(owner, repo).get()")
    }
}
