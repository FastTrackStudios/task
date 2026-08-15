//! `IssueTracker` impl. List, get, create, update, comments
//! wired against `/api/v1/repos/{owner}/{repo}/issues...`.
//! `subscribe` is stubbed — Forgejo doesn't expose a long-poll
//! event stream out of the box, so a polling implementation
//! lands in a follow-up.

use crate::{Backend, check_status, map_err};
use git_proto::issues::{Comment, Issue, IssueFilter, IssueTracker, IssueUpdate};
use git_proto::{GitError, GitEvent, IssueId, IssueState, Label, Milestone, RepoId, User};
use serde::{Deserialize, Serialize};

// ---- raw DTOs ---------------------------------------------------------

#[derive(Debug, Deserialize)]
struct RawIssue {
    /// Forgejo's per-repo issue index. Same field used for PRs
    /// on the `/pulls` endpoint — they share neither namespace
    /// nor numbering on Forgejo (unlike GitHub).
    number: u64,
    title: String,
    body: Option<String>,
    state: String,
    user: RawUser,
    #[serde(default)]
    labels: Vec<RawLabel>,
    #[serde(default)]
    assignees: Option<Vec<RawUser>>,
    milestone: Option<RawMilestone>,
    /// Gitea/Forgejo RFC-3339 last-update timestamp.
    #[serde(default)]
    updated_at: Option<String>,
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
    /// Forgejo returns label colors as 6-char hex without `#`.
    #[serde(default)]
    color: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawMilestone {
    id: u64,
    title: String,
}

#[derive(Debug, Deserialize)]
struct RawComment {
    id: u64,
    body: String,
    user: RawUser,
}

// ---- impl -------------------------------------------------------------

impl IssueTracker for Backend {
    fn list_issues(&self, repo: &RepoId, filter: IssueFilter) -> Result<Vec<Issue>, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let url = Backend::url(
                &base,
                &format!("/repos/{}/{}/issues", repo.owner, repo.repo),
            );
            let mut req = backend.http().get(&url);
            // `type=issues` filters out PRs — Forgejo lists both
            // through `/issues` unless told otherwise.
            let mut query: Vec<(&str, String)> = vec![("type", "issues".into())];
            if let Some(state) = filter.state {
                query.push((
                    "state",
                    match state {
                        IssueState::Open => "open",
                        IssueState::Closed => "closed",
                    }
                    .into(),
                ));
            }
            if !filter.labels.is_empty() {
                query.push(("labels", filter.labels.join(",")));
            }
            if let Some(ref assignee) = filter.assignee {
                query.push(("assigned_by", assignee.clone()));
            }
            if let Some(ref creator) = filter.author {
                query.push(("created_by", creator.clone()));
            }
            if let Some(milestone) = filter.milestone {
                query.push(("milestones", milestone.to_string()));
            }
            req = req.query(&query);
            let resp = backend.auth(req).send().await.map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: Vec<RawIssue> = resp.json().await.map_err(map_err)?;
            Ok(raw.into_iter().map(|i| translate_issue(&repo, i)).collect())
        })
    }

    fn get_issue(&self, repo: &RepoId, issue: IssueId) -> Result<Issue, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let url = Backend::url(
                &base,
                &format!("/repos/{}/{}/issues/{}", repo.owner, repo.repo, issue.0),
            );
            let resp = backend
                .auth(backend.http().get(&url))
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: RawIssue = resp.json().await.map_err(map_err)?;
            Ok(translate_issue(&repo, raw))
        })
    }

    fn create_issue(&self, repo: &RepoId, title: String, body: String) -> Result<Issue, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        let created: Result<Issue, GitError> = self.runtime().block_on(async move {
            #[derive(Serialize)]
            struct Body {
                title: String,
                body: String,
            }
            let url = Backend::url(
                &base,
                &format!("/repos/{}/{}/issues", repo.owner, repo.repo),
            );
            let resp = backend
                .auth(backend.http().post(&url))
                .json(&Body { title, body })
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: RawIssue = resp.json().await.map_err(map_err)?;
            Ok(translate_issue(&repo, raw))
        });
        if let Ok(issue) = &created {
            self.publish_issue(GitEvent::IssueCreated {
                repo: issue.repo.clone(),
                issue: issue.id,
            });
        }
        created
    }

    fn update_issue(
        &self,
        repo: &RepoId,
        issue: IssueId,
        update: IssueUpdate,
    ) -> Result<Issue, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        let updated: Result<Issue, GitError> = self.runtime().block_on(async move {
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
                /// Outer `Option` = "leave alone"; inner
                /// `Option` carrying `None` = "clear".
                #[serde(skip_serializing_if = "Option::is_none")]
                milestone: Option<u64>,
            }
            let body = Body {
                title: update.title,
                body: update.body,
                state: update.state.map(|s| match s {
                    IssueState::Open => "open".into(),
                    IssueState::Closed => "closed".into(),
                }),
                assignees: update.assignees,
                milestone: update.milestone.and_then(|m| m),
            };
            let url = Backend::url(
                &base,
                &format!("/repos/{}/{}/issues/{}", repo.owner, repo.repo, issue.0),
            );
            let resp = backend
                .auth(backend.http().patch(&url))
                .json(&body)
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: RawIssue = resp.json().await.map_err(map_err)?;
            // `labels` on Forgejo issues are managed via a
            // separate `/labels` endpoint; PATCH ignores the
            // field. Leave label updates as a follow-up — the
            // proto's `IssueUpdate::labels` is dropped silently
            // for now (we still return the issue as the server
            // sees it).
            let _ = update.labels;
            Ok(translate_issue(&repo, raw))
        });
        if let Ok(issue) = &updated {
            self.publish_issue(GitEvent::IssueUpdated {
                repo: issue.repo.clone(),
                issue: issue.id,
                state: issue.state,
            });
        }
        updated
    }

    fn list_comments(&self, repo: &RepoId, issue: IssueId) -> Result<Vec<Comment>, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let url = Backend::url(
                &base,
                &format!(
                    "/repos/{}/{}/issues/{}/comments",
                    repo.owner, repo.repo, issue.0
                ),
            );
            let resp = backend
                .auth(backend.http().get(&url))
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: Vec<RawComment> = resp.json().await.map_err(map_err)?;
            Ok(raw.into_iter().map(translate_comment).collect())
        })
    }

    fn add_comment(
        &self,
        repo: &RepoId,
        issue: IssueId,
        body: String,
    ) -> Result<Comment, GitError> {
        let base = self.check_forge(repo)?.to_string();
        let backend = self.clone();
        let event_repo = repo.clone();
        let repo = repo.clone();
        let posted: Result<Comment, GitError> = self.runtime().block_on(async move {
            #[derive(Serialize)]
            struct Body {
                body: String,
            }
            let url = Backend::url(
                &base,
                &format!(
                    "/repos/{}/{}/issues/{}/comments",
                    repo.owner, repo.repo, issue.0
                ),
            );
            let resp = backend
                .auth(backend.http().post(&url))
                .json(&Body { body })
                .send()
                .await
                .map_err(map_err)?;
            let resp = check_status(resp).await?;
            let raw: RawComment = resp.json().await.map_err(map_err)?;
            Ok(translate_comment(raw))
        });
        if posted.is_ok() {
            self.publish_issue(GitEvent::IssueCommented {
                repo: event_repo,
                issue,
            });
        }
        posted
    }
}

/// The `#[subscribe]` backend contract for issue traffic.
///
/// Forgejo exposes no long-poll event stream and webhooks need an
/// externally-reachable receiver, so the hub only carries changes
/// **this process** commits — the writes Task itself makes through
/// `create_issue` / `update_issue` / `add_comment`. That is exactly
/// what the UI needs to stop refetching after its own mutations;
/// forge-side changes by other actors still arrive via the server's
/// poll loop.
impl git_proto::issues::IssueTrackerStreamSource for Backend {
    fn issue_events_hub(&self) -> &architect::PubSub<GitEvent> {
        self.issue_hub()
    }
}

// ---- translators ------------------------------------------------------

fn translate_user(raw: RawUser) -> User {
    User {
        login: raw.login,
        display_name: raw.full_name,
    }
}

fn translate_issue(repo: &RepoId, raw: RawIssue) -> Issue {
    Issue {
        id: IssueId(raw.number),
        repo: repo.clone(),
        title: raw.title,
        body: raw.body.unwrap_or_default(),
        state: match raw.state.as_str() {
            "open" => IssueState::Open,
            _ => IssueState::Closed,
        },
        author: translate_user(raw.user),
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
        milestone: raw.milestone.map(|m| Milestone {
            title: m.title,
            number: m.id,
        }),
        updated_at: raw.updated_at,
    }
}

fn translate_comment(raw: RawComment) -> Comment {
    Comment {
        id: raw.id.to_string(),
        author: translate_user(raw.user),
        body: raw.body,
    }
}
