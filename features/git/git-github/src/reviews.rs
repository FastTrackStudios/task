//! `ReviewSurface` impl — PR CRUD, reviews, requested
//! reviewers, merge. Mirrors the octocrab-async + `block_on`
//! shape used by the `IssueTracker` impl.

use crate::{Backend, map_err};
use git_proto::reviews::{
    MergeMethod, NewPullRequest, PullRequestUpdate, Review, ReviewState, ReviewSurface,
};
use git_proto::{
    GitError, GitEvent, Label, PullRequest, PullRequestId, PullRequestState, RepoId, Reviewer, User,
};

impl ReviewSurface for Backend {
    fn list_pull_requests(&self, repo: &RepoId) -> Result<Vec<PullRequest>, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let page = backend
                .octo()
                .pulls(&repo.owner, &repo.repo)
                .list()
                .send()
                .await
                .map_err(map_err)?;
            Ok(page
                .items
                .into_iter()
                .map(|p| translate_pr(&repo, p))
                .collect())
        })
    }

    fn get_pull_request(&self, repo: &RepoId, pr: PullRequestId) -> Result<PullRequest, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let raw = backend
                .octo()
                .pulls(&repo.owner, &repo.repo)
                .get(pr.0)
                .await
                .map_err(map_err)?;
            Ok(translate_pr(&repo, raw))
        })
    }

    fn create_pull_request(
        &self,
        repo: &RepoId,
        new: NewPullRequest,
    ) -> Result<PullRequest, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        let created: Result<PullRequest, GitError> = self.runtime().block_on(async move {
            let raw = backend
                .octo()
                .pulls(&repo.owner, &repo.repo)
                .create(new.title, new.head, new.base)
                .body(new.body)
                .draft(Some(new.draft))
                .send()
                .await
                .map_err(map_err)?;
            Ok(translate_pr(&repo, raw))
        });
        if let Ok(pr) = &created {
            self.publish_review(GitEvent::PullRequestCreated {
                repo: pr.repo.clone(),
                pr: pr.id,
            });
        }
        created
    }

    fn update_pull_request(
        &self,
        repo: &RepoId,
        pr: PullRequestId,
        update: PullRequestUpdate,
    ) -> Result<PullRequest, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        let updated: Result<PullRequest, GitError> = self.runtime().block_on(async move {
            let handle = backend.octo().pulls(&repo.owner, &repo.repo);
            let mut req = handle.update(pr.0);
            if let Some(ref t) = update.title {
                req = req.title(t.as_str());
            }
            if let Some(ref b) = update.body {
                req = req.body(b.as_str());
            }
            if let Some(s) = update.state {
                // octocrab's PR update takes its own state enum
                // (open/closed only — "merged" is a side effect
                // of merge, not a settable state).
                req = req.state(match s {
                    PullRequestState::Open => octocrab::params::pulls::State::Open,
                    PullRequestState::Closed | PullRequestState::Merged => {
                        octocrab::params::pulls::State::Closed
                    }
                });
            }
            // `draft`, `labels`, `assignees` aren't exposed on
            // octocrab's PR update builder — they go through the
            // issues endpoint (a PR is an issue on GitHub). Left
            // unhandled here; documented on the tracking task.
            let raw = req.send().await.map_err(map_err)?;
            Ok(translate_pr(&repo, raw))
        });
        if let Ok(pr) = &updated {
            self.publish_review(GitEvent::PullRequestUpdated {
                repo: pr.repo.clone(),
                pr: pr.id,
                state: pr.state,
            });
        }
        updated
    }

    fn list_reviews(&self, repo: &RepoId, pr: PullRequestId) -> Result<Vec<Review>, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let page = backend
                .octo()
                .pulls(&repo.owner, &repo.repo)
                .list_reviews(pr.0)
                .send()
                .await
                .map_err(map_err)?;
            Ok(page.items.into_iter().map(translate_review).collect())
        })
    }

    fn request_reviewers(
        &self,
        repo: &RepoId,
        pr: PullRequestId,
        reviewers: Vec<Reviewer>,
    ) -> Result<(), GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        // Split the proto's user/team distinction into the two
        // lists GitHub's endpoint expects.
        let mut users = Vec::new();
        let mut teams = Vec::new();
        for r in reviewers {
            match r {
                Reviewer::User(u) => users.push(u.login),
                Reviewer::Team { slug } => teams.push(slug),
            }
        }
        self.runtime().block_on(async move {
            backend
                .octo()
                .pulls(&repo.owner, &repo.repo)
                .request_reviews(pr.0, users, teams)
                .await
                .map_err(map_err)?;
            Ok(())
        })
    }

    fn merge_pull_request(
        &self,
        repo: &RepoId,
        pr: PullRequestId,
        method: MergeMethod,
    ) -> Result<Option<String>, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let event_repo = repo.clone();
        let repo = repo.clone();
        let merged: Result<Option<String>, GitError> = self.runtime().block_on(async move {
            let merge_method = match method {
                MergeMethod::Merge => octocrab::params::pulls::MergeMethod::Merge,
                MergeMethod::Squash => octocrab::params::pulls::MergeMethod::Squash,
                MergeMethod::Rebase => octocrab::params::pulls::MergeMethod::Rebase,
            };
            let merged = backend
                .octo()
                .pulls(&repo.owner, &repo.repo)
                .merge(pr.0)
                .method(merge_method)
                .send()
                .await
                .map_err(map_err)?;
            // octocrab's Merge model exposes the resulting sha.
            Ok(merged.sha)
        });
        if merged.is_ok() {
            self.publish_review(GitEvent::PullRequestUpdated {
                repo: event_repo,
                pr,
                state: PullRequestState::Merged,
            });
        }
        merged
    }
}

/// The `#[subscribe]` backend contract for pull-request traffic.
/// Same scope as the issue stream: changes this process commits.
impl git_proto::reviews::ReviewSurfaceStreamSource for Backend {
    fn review_events_hub(&self) -> &architect::PubSub<GitEvent> {
        self.review_hub()
    }
}

/// Translate octocrab's PR model into the proto shape.
fn translate_pr(repo: &RepoId, raw: octocrab::models::pulls::PullRequest) -> PullRequest {
    // PR state: merged trumps the open/closed flag.
    let state = if raw.merged_at.is_some() {
        PullRequestState::Merged
    } else {
        match raw.state {
            Some(octocrab::models::IssueState::Open) => PullRequestState::Open,
            Some(octocrab::models::IssueState::Closed) => PullRequestState::Closed,
            _ => PullRequestState::Closed,
        }
    };
    let author = raw.user.map_or_else(
        || User {
            login: String::new(),
            display_name: None,
        },
        |u| User {
            login: u.login.clone(),
            display_name: None,
        },
    );
    PullRequest {
        id: PullRequestId(raw.number),
        repo: repo.clone(),
        title: raw.title.unwrap_or_default(),
        body: raw.body.unwrap_or_default(),
        state,
        author,
        base: raw.base.ref_field,
        head: raw.head.ref_field,
        draft: raw.draft.unwrap_or(false),
        labels: raw
            .labels
            .unwrap_or_default()
            .into_iter()
            .map(|l| Label {
                name: l.name,
                color: Some(l.color),
            })
            .collect(),
        assignees: raw
            .assignees
            .unwrap_or_default()
            .into_iter()
            .map(|a| User {
                login: a.login,
                display_name: None,
            })
            .collect(),
        requested_reviewers: raw
            .requested_reviewers
            .unwrap_or_default()
            .into_iter()
            .map(|a| {
                Reviewer::User(User {
                    login: a.login,
                    display_name: None,
                })
            })
            .collect(),
    }
}

/// Translate octocrab's review model into the proto shape.
fn translate_review(raw: octocrab::models::pulls::Review) -> Review {
    use octocrab::models::pulls::ReviewState as OctoState;
    let state = match raw.state {
        Some(OctoState::Approved) => ReviewState::Approved,
        Some(OctoState::ChangesRequested) => ReviewState::ChangesRequested,
        Some(OctoState::Commented) => ReviewState::Commented,
        Some(OctoState::Dismissed) => ReviewState::Dismissed,
        Some(OctoState::Pending) => ReviewState::Pending,
        _ => ReviewState::Commented,
    };
    let author = raw.user.map_or_else(
        || User {
            login: String::new(),
            display_name: None,
        },
        |u| User {
            login: u.login.clone(),
            display_name: None,
        },
    );
    Review {
        id: raw.id.0.to_string(),
        author,
        state,
        body: raw.body.unwrap_or_default(),
    }
}
