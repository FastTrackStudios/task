//! Forge (Forgejo / GitHub) backend plumbing shared by
//! `task issue`, `task code`, and `task setup`.
//!
//! Moved verbatim out of `main.rs`; behaviour unchanged.

/// Build a `RepoId` for a forge from an `owner/repo` slug.
/// `github=true` → GitHub; else Forgejo with the resolved base
/// URL.
pub(crate) fn build_repo_id(
    repo_slug: &str,
    github: bool,
    base_url: Option<String>,
) -> eyre::Result<git_proto::RepoId> {
    let (owner, repo) = parse_repo_slug(repo_slug)?;
    let forge = if github {
        git_proto::Forge::Github
    } else {
        git_proto::Forge::Forgejo {
            base_url: forgejo_base_url(base_url)?,
        }
    };
    Ok(git_proto::RepoId { forge, owner, repo })
}

/// Parse `owner/repo` into a tuple.
pub(crate) fn parse_repo_slug(s: &str) -> eyre::Result<(String, String)> {
    let (owner, repo) = s
        .split_once('/')
        .ok_or_else(|| eyre::eyre!("expected `owner/repo`, got `{s}`"))?;
    if owner.is_empty() || repo.is_empty() {
        return Err(eyre::eyre!("owner/repo: empty part in `{s}`"));
    }
    Ok((owner.to_string(), repo.to_string()))
}

/// Resolve the Forgejo base URL: flag > env `TASK_FORGEJO_BASE_URL`
/// > error. Trims trailing slash.
pub(crate) fn forgejo_base_url(flag: Option<String>) -> eyre::Result<String> {
    let raw = flag
        .or_else(|| std::env::var("TASK_FORGEJO_BASE_URL").ok())
        .ok_or_else(|| {
            eyre::eyre!("no Forgejo base URL — pass --base-url or set TASK_FORGEJO_BASE_URL")
        })?;
    Ok(raw.trim_end_matches('/').to_string())
}

/// A constructed forge backend, picked by the repo's `Forge`
/// variant. Enum dispatch rather than `Box<dyn IssueTracker>` —
/// the trait's methods take `&RepoId` and it isn't worth an
/// object-safe wrapper for two variants. Each method forwards to
/// the matching backend's sync `IssueTracker` impl.
pub(crate) enum ForgeBackend {
    Forgejo(git_forgejo::Backend),
    Github(git_github::Backend),
}

impl ForgeBackend {
    pub(crate) fn create_issue(
        &self,
        repo: &git_proto::RepoId,
        title: String,
        body: String,
    ) -> Result<git_proto::issues::Issue, git_proto::GitError> {
        use git_proto::issues::IssueTracker;
        match self {
            Self::Forgejo(b) => b.create_issue(repo, title, body),
            Self::Github(b) => b.create_issue(repo, title, body),
        }
    }

    pub(crate) fn update_issue(
        &self,
        repo: &git_proto::RepoId,
        issue: git_proto::IssueId,
        update: git_proto::issues::IssueUpdate,
    ) -> Result<git_proto::issues::Issue, git_proto::GitError> {
        use git_proto::issues::IssueTracker;
        match self {
            Self::Forgejo(b) => b.update_issue(repo, issue, update),
            Self::Github(b) => b.update_issue(repo, issue, update),
        }
    }

    pub(crate) fn list_issues(
        &self,
        repo: &git_proto::RepoId,
        filter: git_proto::issues::IssueFilter,
    ) -> Result<Vec<git_proto::issues::Issue>, git_proto::GitError> {
        use git_proto::issues::IssueTracker;
        match self {
            Self::Forgejo(b) => b.list_issues(repo, filter),
            Self::Github(b) => b.list_issues(repo, filter),
        }
    }

    pub(crate) fn get_issue(
        &self,
        repo: &git_proto::RepoId,
        issue: git_proto::IssueId,
    ) -> Result<git_proto::issues::Issue, git_proto::GitError> {
        use git_proto::issues::IssueTracker;
        match self {
            Self::Forgejo(b) => b.get_issue(repo, issue),
            Self::Github(b) => b.get_issue(repo, issue),
        }
    }

    pub(crate) fn list_pull_requests(
        &self,
        repo: &git_proto::RepoId,
    ) -> Result<Vec<git_proto::PullRequest>, git_proto::GitError> {
        use git_proto::reviews::ReviewSurface;
        match self {
            Self::Forgejo(b) => b.list_pull_requests(repo),
            Self::Github(b) => b.list_pull_requests(repo),
        }
    }

    pub(crate) fn create_pull_request(
        &self,
        repo: &git_proto::RepoId,
        new: git_proto::reviews::NewPullRequest,
    ) -> Result<git_proto::PullRequest, git_proto::GitError> {
        use git_proto::reviews::ReviewSurface;
        match self {
            Self::Forgejo(b) => b.create_pull_request(repo, new),
            Self::Github(b) => b.create_pull_request(repo, new),
        }
    }

    pub(crate) fn merge_pull_request(
        &self,
        repo: &git_proto::RepoId,
        pr: git_proto::PullRequestId,
        method: git_proto::reviews::MergeMethod,
    ) -> Result<Option<String>, git_proto::GitError> {
        use git_proto::reviews::ReviewSurface;
        match self {
            Self::Forgejo(b) => b.merge_pull_request(repo, pr, method),
            Self::Github(b) => b.merge_pull_request(repo, pr, method),
        }
    }
}

/// Build the right backend for a repo, reading the matching
/// token. Forgejo → `forgejo_token()`; GitHub → `github_token()`.
pub(crate) fn forge_backend_for(repo: &git_proto::RepoId) -> eyre::Result<ForgeBackend> {
    match &repo.forge {
        git_proto::Forge::Forgejo { base_url } => {
            let tok = forgejo_token()?;
            let base = if base_url.is_empty() {
                forgejo_base_url(None)?
            } else {
                base_url.clone()
            };
            let b = git_forgejo::Backend::from_token(&base, &tok)
                .map_err(|e| eyre::eyre!("forgejo backend: {e:?}"))?;
            Ok(ForgeBackend::Forgejo(b))
        }
        git_proto::Forge::Github => {
            let tok = github_token()?;
            let b = git_github::Backend::from_token(&tok)
                .map_err(|e| eyre::eyre!("github backend: {e:?}"))?;
            Ok(ForgeBackend::Github(b))
        }
    }
}

/// Resolve a GitHub personal-access token: env `TASK_GITHUB_TOKEN`
/// then `GITHUB_TOKEN`, then `~/.config/task/github-token`, then
/// error.
pub(crate) fn github_token() -> eyre::Result<String> {
    for var in ["TASK_GITHUB_TOKEN", "GITHUB_TOKEN"] {
        if let Ok(v) = std::env::var(var) {
            if !v.is_empty() {
                return Ok(v);
            }
        }
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| eyre::eyre!("HOME not set; can't resolve fallback token path"))?;
    let p = std::path::Path::new(&home)
        .join(".config")
        .join("task")
        .join("github-token");
    if p.exists() {
        let s =
            std::fs::read_to_string(&p).map_err(|e| eyre::eyre!("read {}: {e}", p.display()))?;
        let t = s.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    Err(eyre::eyre!(
        "no GitHub token — set TASK_GITHUB_TOKEN (or GITHUB_TOKEN) or write one to ~/.config/task/github-token"
    ))
}

/// Resolve a Forgejo personal-access token: env `TASK_FORGEJO_TOKEN`
/// then `FORGEJO_TOKEN`, then `~/.config/task/forgejo-token`, then
/// error.
pub(crate) fn forgejo_token() -> eyre::Result<String> {
    if let Ok(v) = std::env::var("TASK_FORGEJO_TOKEN") {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if let Ok(v) = std::env::var("FORGEJO_TOKEN") {
        if !v.is_empty() {
            return Ok(v);
        }
    }
    let home = std::env::var_os("HOME")
        .ok_or_else(|| eyre::eyre!("HOME not set; can't resolve fallback token path"))?;
    let p = std::path::Path::new(&home)
        .join(".config")
        .join("task")
        .join("forgejo-token");
    if p.exists() {
        let s =
            std::fs::read_to_string(&p).map_err(|e| eyre::eyre!("read {}: {e}", p.display()))?;
        let t = s.trim();
        if !t.is_empty() {
            return Ok(t.to_string());
        }
    }
    Err(eyre::eyre!(
        "no Forgejo token — set TASK_FORGEJO_TOKEN (or FORGEJO_TOKEN) or write one to ~/.config/task/forgejo-token"
    ))
}

/// Open the per-org issue-link `FileStore` at
/// `~/.task/orgs/<slug>/issue-links.json`.
///
/// Vox-unification judgment: machine-local integration config —
/// the forge sync loop that consumes it (webhooks, `task setup`,
/// server forge_sync) runs co-resident with this data root. A
/// remote provisioning path would need an org-management RPC
/// (gap, tracked with the webhook secret below in `setup.rs`).
pub(crate) fn forge_link_store(org_slug: &str) -> eyre::Result<git_config::FileStore> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| eyre::eyre!("HOME not set; can't resolve issue-link store path"))?;
    let p = std::path::Path::new(&home)
        .join(".task")
        .join("orgs")
        .join(org_slug)
        .join("issue-links.json");
    git_config::FileStore::open(p).map_err(|e| eyre::eyre!("open link store: {e}"))
}
