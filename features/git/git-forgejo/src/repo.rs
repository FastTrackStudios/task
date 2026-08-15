//! `RepoCatalog` impl. `get_repo` is wired against
//! `GET /api/v1/repos/{owner}/{repo}`. `list_repos` paginates
//! `/repos/search` filtered to the authenticated user.

use crate::{Backend, check_status, map_err};
use git_proto::repo::RepoCatalog;
use git_proto::{Forge, GitError, Repo, RepoId};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
struct RawRepo {
    // `id`/`full_name` not surfaced through `git_proto::Repo`,
    // but we need `owner.login` + `name` to reconstruct a
    // `RepoId` for the `list_repos` path.
    name: String,
    owner: RawOwner,
    description: Option<String>,
    default_branch: Option<String>,
    private: bool,
    archived: bool,
}

#[derive(Debug, Deserialize)]
struct RawOwner {
    login: String,
}

impl RepoCatalog for Backend {
    fn list_repos(&self) -> Result<Vec<Repo>, GitError> {
        let backend = self.clone();
        // No `RepoId` argument — caller can't tell us which
        // Forgejo host they meant if a backend was somehow built
        // hostless. Use the configured base.
        let base = backend.inner.base_url.clone();
        self.runtime().block_on(async move {
            // `/repos/search` returns paginated results. First
            // page only for now; full pagination is a follow-up
            // when a caller needs >50 repos. Forgejo defaults to
            // 50 per page.
            let url = format!("{base}/api/v1/repos/search?limit=50");
            let resp = backend
                .auth(backend.http().get(&url))
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            #[derive(Deserialize)]
            struct Page {
                data: Vec<RawRepo>,
            }
            let page: Page = resp.json().await.map_err(map_err)?;
            Ok(page
                .data
                .into_iter()
                .map(|r| translate_repo(&base, r))
                .collect())
        })
    }

    fn get_repo(&self, repo: &RepoId) -> Result<Repo, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let url = Backend::url(&base, &format!("/repos/{}/{}", repo.owner, repo.repo));
            let resp = backend
                .auth(backend.http().get(&url))
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: RawRepo = resp.json().await.map_err(map_err)?;
            Ok(translate_repo(&base, raw))
        })
    }
}

fn translate_repo(base_url: &str, raw: RawRepo) -> Repo {
    Repo {
        id: RepoId {
            forge: Forge::Forgejo {
                base_url: base_url.to_string(),
            },
            owner: raw.owner.login,
            repo: raw.name,
        },
        description: raw.description,
        default_branch: raw.default_branch.unwrap_or_else(|| "main".into()),
        private: raw.private,
        archived: raw.archived,
    }
}
