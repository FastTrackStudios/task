//! GitHub backend for `git-proto`.
//!
//! Implements all three traits (`RepoCatalog`, `IssueTracker`,
//! `ReviewSurface`) on a single [`Backend`] type backed by
//! [`octocrab`]. First-pass scope: `list_issues` and
//! `get_issue` are wired end-to-end; every other method is a
//! `todo!()` stub so the type checks and the bridge can route
//! calls. Methods get filled in pass by pass.
//!
//! Same `architect::HasDispatcher` + `block_on` shape as
//! `email-imap::Backend`. The trait methods are sync; we hop
//! into the captured tokio runtime via `block_on` to drive
//! `octocrab`'s async calls.

#![cfg(not(target_arch = "wasm32"))]

mod issues;
mod repo;
mod reviews;

use std::sync::Arc;

use git_proto::{Forge, RepoId};

/// GitHub backend. Cheap to `Clone` — internals are `Arc`'d.
#[derive(Clone, architect::HasDispatcher)]
pub struct Backend {
    inner: Arc<Inner>,
}

struct Inner {
    octo: octocrab::Octocrab,
    runtime: tokio::runtime::Handle,
    /// Fan-out hubs behind the `#[subscribe]` streams — issue
    /// traffic and pull-request traffic kept apart so a reviews
    /// subscriber doesn't pay for issue churn. Every committed
    /// mutation publishes into one (see `publish_issue` /
    /// `publish_review`). Sliding mailboxes: a slow subscriber
    /// loses its oldest queued events and re-reads on reconnect —
    /// correct here, since events are "that row is stale" hints,
    /// not state.
    issue_events: architect::PubSub<git_proto::GitEvent>,
    review_events: architect::PubSub<git_proto::GitEvent>,
}

impl Backend {
    /// Build a backend from a personal access token. Must be
    /// called from a tokio runtime — the handle is captured and
    /// reused for every `block_on`.
    pub fn from_token(token: impl Into<String>) -> Result<Self, BuildError> {
        // octocrab's rustls backend needs a process-level
        // CryptoProvider. reqwest installs one as a side effect;
        // octocrab does not, so install ring once here. Ignore
        // the Err when a provider is already installed.
        static CRYPTO: std::sync::Once = std::sync::Once::new();
        CRYPTO.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
        let runtime = tokio::runtime::Handle::try_current().map_err(|_| BuildError::NoRuntime)?;
        let octo = octocrab::OctocrabBuilder::new()
            .personal_token(token.into())
            .build()
            .map_err(|e| BuildError::Octocrab(e.to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner {
                octo,
                runtime,
                issue_events: architect::PubSub::sliding(256),
                review_events: architect::PubSub::sliding(256),
            }),
        })
    }


    /// Announce a committed issue change. Call only after GitHub
    /// accepted the write — subscribers re-read on the event, so a
    /// speculative one shows state that never was.
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

    /// Reject non-GitHub repos at the boundary so backend
    /// methods can assume the forge matches. The dispatcher
    /// should route correctly already, but defensive checks are
    /// cheap and surface routing bugs early.
    pub(crate) fn check_forge(repo: &RepoId) -> Result<(), git_proto::GitError> {
        match &repo.forge {
            Forge::Github => Ok(()),
            Forge::Forgejo { .. } => Err(git_proto::GitError::Invalid(
                "git-github backend received a non-github repo".into(),
            )),
        }
    }

    pub(crate) fn octo(&self) -> &octocrab::Octocrab {
        &self.inner.octo
    }

    pub(crate) fn runtime(&self) -> &tokio::runtime::Handle {
        &self.inner.runtime
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("Backend::from_token must be called from a tokio runtime")]
    NoRuntime,
    #[error("octocrab build failed: {0}")]
    Octocrab(String),
}

/// Map an octocrab error into the proto's `GitError`. Best
/// effort — status codes get a dedicated variant; everything
/// else falls into `Forge`.
#[allow(clippy::needless_pass_by_value)]
pub(crate) fn map_err(err: octocrab::Error) -> git_proto::GitError {
    use git_proto::GitError;
    let msg = err.to_string();
    if let octocrab::Error::GitHub { source, .. } = &err {
        let code = source.status_code.as_u16();
        return match code {
            401 | 403 => GitError::Unauthorized(msg),
            404 => GitError::NotFound(msg),
            422 => GitError::Invalid(msg),
            429 => GitError::RateLimited(msg),
            _ => GitError::Forge(msg),
        };
    }
    GitError::Forge(msg)
}
