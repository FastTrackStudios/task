//! Forgejo (Gitea-compatible) backend for `git-proto`.
//!
//! Implements all three traits (`RepoCatalog`, `IssueTracker`,
//! `ReviewSurface`) on a single [`Backend`] type backed by a
//! [`reqwest::Client`] talking to a Forgejo REST API at the
//! base URL captured in [`git_proto::Forge::Forgejo`].
//!
//! Same `architect::HasDispatcher` + `block_on` shape as
//! `git-github`. The trait methods are sync; we hop into the
//! captured tokio runtime via `block_on` to drive `reqwest`'s
//! async calls.
//!
//! Unlike GitHub, Forgejo separates issues and pull requests
//! into different REST resources (`/issues` vs `/pulls`) and
//! has independent numbering namespaces for each — the
//! `IssueId` / `PullRequestId` split in `git_proto::payload`
//! already encodes that distinction.

#![cfg(not(target_arch = "wasm32"))]

mod issues;
mod repo;
mod reviews;

use std::sync::Arc;

use git_proto::{Forge, GitError, RepoId};

/// Forgejo backend. Cheap to `Clone` — internals are `Arc`'d.
#[derive(Clone, architect::HasDispatcher)]
pub struct Backend {
    inner: Arc<Inner>,
}

struct Inner {
    http: reqwest::Client,
    base_url: String,
    token: String,
    runtime: tokio::runtime::Handle,
    /// Fan-out hubs behind the `#[subscribe]` streams — issue
    /// traffic and pull-request traffic kept apart so a reviews
    /// subscriber doesn't pay for issue churn. Every committed
    /// mutation publishes into one of them (see `publish_issue` /
    /// `publish_review`). Sliding mailboxes: a slow subscriber
    /// loses its oldest queued events and re-reads on reconnect —
    /// correct here, since events are "that row is stale" hints,
    /// not state.
    issue_events: architect::PubSub<git_proto::GitEvent>,
    review_events: architect::PubSub<git_proto::GitEvent>,
}

impl Backend {
    /// Build a backend from a personal access token and a base
    /// URL (the Forgejo origin, e.g. `https://codeberg.org` —
    /// the `/api/v1` prefix is added internally). Must be
    /// called from a tokio runtime — the handle is captured and
    /// reused for every `block_on`.
    pub fn from_token(
        base_url: impl Into<String>,
        token: impl Into<String>,
    ) -> Result<Self, BuildError> {
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| BuildError::NoRuntime)?;
        let http = reqwest::Client::builder()
            .user_agent("task/git-forgejo")
            .build()
            .map_err(|e| BuildError::Http(e.to_string()))?;
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Ok(Self {
            inner: Arc::new(Inner {
                http,
                base_url,
                token: token.into(),
                runtime,
                issue_events: architect::PubSub::sliding(256),
                review_events: architect::PubSub::sliding(256),
            }),
        })
    }


    /// Announce a committed issue change. Call only after the
    /// forge accepted the write — subscribers re-read on the
    /// event, so a speculative one shows state that never was.
    pub(crate) fn publish_issue(&self, event: git_proto::GitEvent) {
        self.inner.issue_events.publish(event);
    }

    /// Announce a committed pull-request change.
    pub(crate) fn publish_review(&self, event: git_proto::GitEvent) {
        self.inner.review_events.publish(event);
    }

    /// The hub the `IssueTracker` stream is served from.
    pub(crate) fn issue_hub(&self) -> &architect::PubSub<git_proto::GitEvent> {
        &self.inner.issue_events
    }

    /// The hub the `ReviewSurface` stream is served from.
    pub(crate) fn review_hub(&self) -> &architect::PubSub<git_proto::GitEvent> {
        &self.inner.review_events
    }

    /// Reject non-Forgejo repos at the boundary so backend
    /// methods can assume the forge matches. The dispatcher
    /// should route correctly already, but defensive checks are
    /// cheap and surface routing bugs early.
    ///
    /// Also returns the per-repo base URL, since the
    /// `Forgejo { base_url }` variant carries it and a single
    /// `Backend` instance might in theory serve repos on the
    /// same host with different paths.
    pub(crate) fn check_forge<'a>(&'a self, repo: &'a RepoId) -> Result<&'a str, GitError> {
        match &repo.forge {
            Forge::Forgejo { base_url } => {
                // Empty `base_url` => fall back to the backend's
                // configured base. Non-empty => caller pinned
                // the host, use that.
                if base_url.is_empty() {
                    Ok(self.inner.base_url.as_str())
                } else {
                    Ok(base_url.trim_end_matches('/'))
                }
            }
            Forge::Github => Err(GitError::Invalid(
                "git-forgejo backend received a non-forgejo repo".into(),
            )),
        }
    }

    pub(crate) fn http(&self) -> &reqwest::Client {
        &self.inner.http
    }

    pub(crate) fn token(&self) -> &str {
        &self.inner.token
    }

    pub(crate) fn runtime(&self) -> &tokio::runtime::Handle {
        &self.inner.runtime
    }

    /// Build `{base}/api/v1{path}`. `path` must start with `/`.
    pub(crate) fn url(base: &str, path: &str) -> String {
        format!("{base}/api/v1{path}")
    }

    /// Attach the bearer token in Forgejo's idiomatic form:
    /// `Authorization: token <PAT>` (mirrors Gitea's docs).
    pub(crate) fn auth(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("Authorization", format!("token {}", self.token()))
            .header("Accept", "application/json")
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("Backend::from_token must be called from a tokio runtime")]
    NoRuntime,
    #[error("reqwest client build failed: {0}")]
    Http(String),
}

/// Map a `reqwest::Error` into the proto's `GitError`. Status
/// codes get dedicated variants; everything else falls into
/// `Forge`.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn map_err(err: reqwest::Error) -> GitError {
    let msg = err.to_string();
    if let Some(status) = err.status() {
        return match status.as_u16() {
            401 | 403 => GitError::Unauthorized(msg),
            404 => GitError::NotFound(msg),
            422 => GitError::Invalid(msg),
            429 => GitError::RateLimited(msg),
            _ => GitError::Forge(msg),
        };
    }
    GitError::Forge(msg)
}

/// Inspect a `reqwest::Response` and translate non-2xx into
/// `GitError`. Consumes the response on the error path.
pub(crate) async fn check_status(resp: reqwest::Response) -> Result<reqwest::Response, GitError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let code = status.as_u16();
    let body = resp.text().await.unwrap_or_default();
    let msg = format!("HTTP {code}: {body}");
    Err(match code {
        401 | 403 => GitError::Unauthorized(msg),
        404 => GitError::NotFound(msg),
        422 => GitError::Invalid(msg),
        429 => GitError::RateLimited(msg),
        _ => GitError::Forge(msg),
    })
}
