//! `IssueTracker` impl. `list_issues` + `get_issue` are wired
//! end-to-end; the rest are `todo!()` stubs.

use crate::{Backend, map_err};
use git_proto::issues::{Comment, Issue, IssueFilter, IssueTracker, IssueUpdate};
use git_proto::{GitError, GitEvent, IssueId, IssueState, Label, RepoId, User};

impl IssueTracker for Backend {
    fn list_issues(&self, repo: &RepoId, filter: IssueFilter) -> Result<Vec<Issue>, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let handler = backend.octo().issues(&repo.owner, &repo.repo);
            let mut req = handler.list();
            if let Some(state) = filter.state {
                req = req.state(match state {
                    IssueState::Open => octocrab::params::State::Open,
                    IssueState::Closed => octocrab::params::State::Closed,
                });
            }
            if !filter.labels.is_empty() {
                req = req.labels(&filter.labels);
            }
            if let Some(ref assignee) = filter.assignee {
                req = req.assignee(assignee.as_str());
            }
            if let Some(ref creator) = filter.author {
                req = req.creator(creator.as_str());
            }
            if let Some(milestone) = filter.milestone {
                req = req.milestone(milestone);
            }
            let page = req.send().await.map_err(map_err)?;
            Ok(page
                .items
                .into_iter()
                .map(|i| translate_issue(&repo, i))
                .collect())
        })
    }

    fn get_issue(&self, repo: &RepoId, issue: IssueId) -> Result<Issue, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let raw = backend
                .octo()
                .issues(&repo.owner, &repo.repo)
                .get(issue.0)
                .await
                .map_err(map_err)?;
            Ok(translate_issue(&repo, raw))
        })
    }

    fn create_issue(&self, repo: &RepoId, title: String, body: String) -> Result<Issue, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        let created: Result<Issue, GitError> = self.runtime().block_on(async move {
            let raw = backend
                .octo()
                .issues(&repo.owner, &repo.repo)
                .create(title)
                .body(body)
                .send()
                .await
                .map_err(map_err)?;
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
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        let updated: Result<Issue, GitError> = self.runtime().block_on(async move {
            let handle = backend.octo().issues(&repo.owner, &repo.repo);
            let mut req = handle.update(issue.0);
            if let Some(ref t) = update.title {
                req = req.title(t.as_str());
            }
            if let Some(ref b) = update.body {
                req = req.body(b.as_str());
            }
            if let Some(s) = update.state {
                req = req.state(match s {
                    IssueState::Open => octocrab::models::IssueState::Open,
                    IssueState::Closed => octocrab::models::IssueState::Closed,
                });
            }
            if let Some(ref labels) = update.labels {
                // octocrab takes a slice of String references for labels.
                req = req.labels(labels.as_slice());
            }
            if let Some(ref assignees) = update.assignees {
                req = req.assignees(assignees.as_slice());
            }
            if let Some(milestone_opt) = update.milestone {
                // octocrab's builder takes `impl Into<u64>` for
                // the milestone number. `None` clears via `0`
                // (older octocrab versions; newer expose a
                // `clear_milestone()` we can switch to once
                // it's available in our pinned version).
                req = req.milestone(milestone_opt.unwrap_or(0));
            }
            let raw = req.send().await.map_err(map_err)?;
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
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let repo = repo.clone();
        self.runtime().block_on(async move {
            let page = backend
                .octo()
                .issues(&repo.owner, &repo.repo)
                .list_comments(issue.0)
                .send()
                .await
                .map_err(map_err)?;
            Ok(page
                .items
                .into_iter()
                .map(|c| Comment {
                    id: c.id.0.to_string(),
                    author: User {
                        login: c.user.login.clone(),
                        display_name: c.user.name.clone(),
                    },
                    body: c.body.unwrap_or_default(),
                })
                .collect())
        })
    }

    fn add_comment(
        &self,
        repo: &RepoId,
        issue: IssueId,
        body: String,
    ) -> Result<Comment, GitError> {
        Backend::check_forge(repo)?;
        let backend = self.clone();
        let event_repo = repo.clone();
        let repo = repo.clone();
        let posted: Result<Comment, GitError> = self.runtime().block_on(async move {
            let c = backend
                .octo()
                .issues(&repo.owner, &repo.repo)
                .create_comment(issue.0, body)
                .await
                .map_err(map_err)?;
            Ok(Comment {
                id: c.id.0.to_string(),
                author: User {
                    login: c.user.login.clone(),
                    display_name: c.user.name.clone(),
                },
                body: c.body.unwrap_or_default(),
            })
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
/// GitHub gives us no push channel here (a webhook receiver needs
/// an externally-reachable endpoint; `repos().events()` is a poll),
/// so the hub carries the changes **this process** commits — the
/// writes Task itself makes. That's what lets a UI stop refetching
/// after its own mutations; forge-side changes by other actors
/// still arrive via the server's poll loop.
impl git_proto::issues::IssueTrackerStreamSource for Backend {
    fn issue_events_hub(&self) -> &architect::PubSub<GitEvent> {
        self.issue_hub()
    }
}

/// Translate octocrab's issue model into the proto shape.
fn translate_issue(repo: &RepoId, raw: octocrab::models::issues::Issue) -> Issue {
    Issue {
        id: IssueId(raw.number),
        repo: repo.clone(),
        title: raw.title,
        body: raw.body.unwrap_or_default(),
        // `IssueState` is `#[non_exhaustive]`; the wildcard arm
        // is for unknown future variants — keep both Closed and
        // wildcard mapping to the same value.
        #[allow(clippy::match_same_arms)]
        state: match raw.state {
            octocrab::models::IssueState::Open => IssueState::Open,
            octocrab::models::IssueState::Closed => IssueState::Closed,
            _ => IssueState::Closed,
        },
        author: User {
            login: raw.user.login.clone(),
            display_name: raw.user.name.clone(),
        },
        labels: raw
            .labels
            .into_iter()
            .map(|l| Label {
                name: l.name,
                color: Some(l.color),
            })
            .collect(),
        assignees: raw
            .assignees
            .into_iter()
            .map(|a| User {
                login: a.login,
                display_name: a.name,
            })
            .collect(),
        milestone: raw.milestone.map(|m| git_proto::Milestone {
            title: m.title,
            number: m.number as u64,
        }),
        updated_at: Some(raw.updated_at.to_rfc3339()),
    }
}
