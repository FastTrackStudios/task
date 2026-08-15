//! `ReviewSurface` impl. List / get / create / update / merge
//! / reviews / request-reviewers all wired against the
//! `/api/v1/repos/{owner}/{repo}/pulls...` family.
//!
//! Forgejo separates pull requests from issues at the URL level
//! (`/pulls` not `/issues`) with an independent numbering
//! namespace — `PullRequestId(u64)` carries the `index` value
//! the Forgejo API returns.

use crate::{Backend, check_status, map_err};
use git_proto::reviews::{MergeMethod, NewPullRequest, PullRequestUpdate, Review, ReviewSurface};
use git_proto::{
    GitError, GitEvent, Label, PullRequest, PullRequestId, PullRequestState, RepoId, ReviewState,
    Reviewer, User,
};
use serde::{Deserialize, Serialize};

// ---- raw DTOs ---------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawPull {
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    #[serde(default)]
    merged: bool,
    #[serde(default)]
    draft: bool,
    user: RawUser,
    base: RawRef,
    head: RawRef,
    #[serde(default)]
    labels: Vec<RawLabel>,
    #[serde(default)]
    assignees: Option<Vec<RawUser>>,
    #[serde(default)]
    requested_reviewers: Option<Vec<RawUser>>,
}

#[derive(Debug, Deserialize)]
struct RawRef {
    #[serde(rename = "ref")]
    ref_name: String,
}

#[derive(Debug, Deserialize)]
struct RawUser {
    login: String,
    #[serde(default)]
    full_name: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawLabel {
    name: String,
    #[serde(default)]
    color: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawReview {
    id: u64,
    user: RawUser,
    /// One of `APPROVED`, `REQUEST_CHANGES`, `COMMENT`,
    /// `PENDING`, `DISMISSED` (Forgejo uppercases these).
    state: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMergeResult {
    #[serde(default)]
    sha: Option<String>,
}

// ---- impl -------------------------------------------------------------

impl ReviewSurface for Backend {
    fn list_pull_requests(&self, repo: &RepoId) -> Result<Vec<PullRequest>, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let url = Backend::url(&base, &format!("/repos/{}/{}/pulls", repo.owner, repo.repo));
            let resp = backend
                .auth(backend.http().get(&url))
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: Vec<RawPull> = resp.json().await.map_err(map_err)?;
            Ok(raw.into_iter().map(|p| translate_pull(&repo, p)).collect())
        })
    }

    fn get_pull_request(&self, repo: &RepoId, pr: PullRequestId) -> Result<PullRequest, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let url = Backend::url(
                &base,
                &format!("/repos/{}/{}/pulls/{}", repo.owner, repo.repo, pr.0),
            );
            let resp = backend
                .auth(backend.http().get(&url))
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: RawPull = resp.json().await.map_err(map_err)?;
            Ok(translate_pull(&repo, raw))
        })
    }

    fn create_pull_request(
        &self,
        repo: &RepoId,
        new: NewPullRequest,
    ) -> Result<PullRequest, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        let created: Result<PullRequest, GitError> = self.runtime().block_on(async move {
            #[allow(clippy::struct_field_names)]
            #[derive(Serialize)]
            struct Body {
                title: String,
                body: String,
                base: String,
                head: String,
                #[serde(skip_serializing_if = "std::ops::Not::not")]
                draft: bool,
            }
            let url = Backend::url(&base, &format!("/repos/{}/{}/pulls", repo.owner, repo.repo));
            let resp = backend
                .auth(backend.http().post(&url))
                .json(&Body {
                    title: new.title,
                    body: new.body,
                    base: new.base,
                    head: new.head,
                    draft: new.draft,
                })
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: RawPull = resp.json().await.map_err(map_err)?;
            Ok(translate_pull(&repo, raw))
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
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        let updated: Result<PullRequest, GitError> = self.runtime().block_on(async move {
            #[allow(clippy::struct_field_names)]
            #[derive(Serialize, Default)]
            struct Body {
                #[serde(skip_serializing_if = "Option::is_none")]
                title: Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                body: Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                state: Option<String>,
                #[serde(skip_serializing_if = "Option::is_none")]
                assignees: Option<Vec<String>>,
            }
            let body = Body {
                title: update.title,
                body: update.body,
                state: update.state.map(|s| match s {
                    PullRequestState::Open => "open".into(),
                    // Forgejo only accepts `open` / `closed` on
                    // PATCH; `merged` is the result of a merge,
                    // not a settable state. Collapse to closed.
                    PullRequestState::Closed | PullRequestState::Merged => "closed".into(),
                }),
                assignees: update.assignees,
            };
            // `draft` toggling has its own endpoint
            // (`/pulls/{n}/{ready_for_review,convert_to_draft}`)
            // — defer until a caller needs it.
            let _ = update.draft;
            let _ = update.labels;
            let url = Backend::url(
                &base,
                &format!("/repos/{}/{}/pulls/{}", repo.owner, repo.repo, pr.0),
            );
            let resp = backend
                .auth(backend.http().patch(&url))
                .json(&body)
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: RawPull = resp.json().await.map_err(map_err)?;
            Ok(translate_pull(&repo, raw))
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
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let url = Backend::url(
                &base,
                &format!("/repos/{}/{}/pulls/{}/reviews", repo.owner, repo.repo, pr.0),
            );
            let resp = backend
                .auth(backend.http().get(&url))
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: Vec<RawReview> = resp.json().await.map_err(map_err)?;
            Ok(raw.into_iter().map(translate_review).collect())
        })
    }

    fn request_reviewers(
        &self,
        repo: &RepoId,
        pr: PullRequestId,
        reviewers: Vec<Reviewer>,
    ) -> Result<(), GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            #[allow(clippy::struct_field_names)]
            #[derive(Serialize)]
            struct Body {
                reviewers: Vec<String>,
                team_reviewers: Vec<String>,
            }
            let mut body = Body {
                reviewers: Vec::new(),
                team_reviewers: Vec::new(),
            };
            for r in reviewers {
                match r {
                    Reviewer::User(u) => body.reviewers.push(u.login),
                    Reviewer::Team { slug } => body.team_reviewers.push(slug),
                }
            }
            let url = Backend::url(
                &base,
                &format!(
                    "/repos/{}/{}/pulls/{}/requested_reviewers",
                    repo.owner, repo.repo, pr.0
                ),
            );
            let resp = backend
                .auth(backend.http().post(&url))
                .json(&body)
                .send()
                .await
                .map_err(map_err)?;
            let _ = check_status(resp).await?;
            Ok(())
        })
    }

    fn merge_pull_request(
        &self,
        repo: &RepoId,
        pr: PullRequestId,
        method: MergeMethod,
    ) -> Result<Option<String>, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let event_repo = repo.clone();
        let repo = repo.clone();
        let merged: Result<Option<String>, GitError> = self.runtime().block_on(async move {
            #[allow(clippy::struct_field_names)]
            #[derive(Serialize)]
            struct Body {
                #[serde(rename = "Do")]
                do_: &'static str,
            }
            let do_ = match method {
                MergeMethod::Merge => "merge",
                MergeMethod::Squash => "squash",
                MergeMethod::Rebase => "rebase",
            };
            let url = Backend::url(
                &base,
                &format!("/repos/{}/{}/pulls/{}/merge", repo.owner, repo.repo, pr.0),
            );
            let resp = backend
                .auth(backend.http().post(&url))
                .json(&Body { do_ })
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            // Forgejo's merge endpoint returns 200 with a body
            // on success and 204 with no body in some
            // versions. Try to parse, tolerate empty.
            let bytes = resp.bytes().await.map_err(map_err)?;
            if bytes.is_empty() {
                return Ok(None);
            }
            let parsed: Result<RawMergeResult, _> = serde_json::from_slice(&bytes);
            Ok(parsed.ok().and_then(|m| m.sha))
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

// ---- translators ------------------------------------------------------

fn translate_user(raw: RawUser) -> User {
    User {
        login: raw.login,
        display_name: raw.full_name,
    }
}

fn translate_pull(repo: &RepoId, raw: RawPull) -> PullRequest {
    let state = if raw.merged {
        PullRequestState::Merged
    } else {
        match raw.state.as_str() {
            "open" => PullRequestState::Open,
            _ => PullRequestState::Closed,
        }
    };
    PullRequest {
        id: PullRequestId(raw.number),
        repo: repo.clone(),
        title: raw.title,
        body: raw.body.unwrap_or_default(),
        state,
        author: translate_user(raw.user),
        base: raw.base.ref_name,
        head: raw.head.ref_name,
        draft: raw.draft,
        labels: raw
            .labels
            .into_iter()
            .map(|l| Label {
                name: l.name,
                color: l.color,
            })
            .collect(),
        assignees: raw
            .assignees
            .unwrap_or_default()
            .into_iter()
            .map(translate_user)
            .collect(),
        requested_reviewers: raw
            .requested_reviewers
            .unwrap_or_default()
            .into_iter()
            .map(|u| Reviewer::User(translate_user(u)))
            .collect(),
    }
}

fn translate_review(raw: RawReview) -> Review {
    let state = match raw.state.as_str() {
        "APPROVED" | "approved" => ReviewState::Approved,
        "REQUEST_CHANGES" | "request_changes" | "CHANGES_REQUESTED" => {
            ReviewState::ChangesRequested
        }
        "COMMENT" | "comment" | "COMMENTED" => ReviewState::Commented,
        "DISMISSED" | "dismissed" => ReviewState::Dismissed,
        _ => ReviewState::Pending,
    };
    Review {
        id: raw.id.to_string(),
        author: translate_user(raw.user),
        state,
        body: raw.body.unwrap_or_default(),
    }
}
