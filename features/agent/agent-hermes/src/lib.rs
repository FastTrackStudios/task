//! Hermes agent backend — an in-process HTTP client for a
//! [hermes-agent](https://github.com/NousResearch/hermes-agent)
//! gateway.
//!
//! Where [`agent-codex`] spawns `codex app-server` processes over
//! stdio, Hermes is a long-running **gateway** we talk to over its
//! OpenAI-compatible API. One gateway serves every session, and the
//! agent's self-improving state (memory, skills) follows the
//! conversation rather than the connection: the [`responses`]
//! transport chains each turn onto the previous response id, and the
//! [`stream`] fallback scopes with the `X-Hermes-Session-Key` header.
//!
//! The backend keeps the Task-side conversation history itself
//! (`Vec<Message>` per session) and replays it as the `messages`
//! array each turn — the gateway's session memory is additive
//! (skills, learned context), not a substitute for the visible
//! transcript.
//!
//! Config comes from `TASK_HERMES_*` env vars (see
//! [`HermesConfig::from_env`]); the backend is only constructed
//! when `TASK_HERMES_URL` is set.

#![cfg(not(target_arch = "wasm32"))]

mod discovery;
mod responses;
mod routines;
mod service;
mod stream;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use agent_proto::event::{AgentEventEnvelope, SessionEvents};
use agent_proto::message::Message;
use agent_proto::session::Session;
use tokio::sync::Mutex;

pub const BACKEND_ID: &str = "hermes";

/// Connection settings for one hermes-agent gateway.
#[derive(Debug, Clone)]
pub struct HermesConfig {
    /// API base including the version segment, e.g.
    /// `http://127.0.0.1:8642/v1`.
    pub base_url: String,
    /// `API_SERVER_KEY` bearer token; empty = no auth header.
    pub api_key: String,
    /// Default model id sent when the turn carries no override.
    /// The gateway resolves it against its configured provider —
    /// `"hermes"` selects the gateway's default profile/model.
    pub model: String,
}

impl HermesConfig {
    /// Read the gateway settings from the environment:
    ///
    /// - `TASK_HERMES_URL` — API base (required; absence = backend
    ///   disabled). With or without the trailing `/v1`.
    /// - `TASK_HERMES_API_KEY` — bearer token (optional).
    /// - `TASK_HERMES_MODEL` — default model (default `hermes`).
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let raw = std::env::var("TASK_HERMES_URL").ok()?;
        let raw = raw.trim().trim_end_matches('/');
        if raw.is_empty() {
            return None;
        }
        let base_url = if raw.ends_with("/v1") {
            raw.to_string()
        } else {
            format!("{raw}/v1")
        };
        Some(Self {
            base_url,
            api_key: std::env::var("TASK_HERMES_API_KEY").unwrap_or_default(),
            model: std::env::var("TASK_HERMES_MODEL").unwrap_or_else(|_| "hermes".to_string()),
        })
    }
}

/// A turn's stop signal.
///
/// A bare `AtomicBool` was only observed *between* SSE chunks, so
/// Stop did nothing while the agent sat in a long tool call — exactly
/// when you want to stop it. Pairing the flag with a `Notify` lets
/// the stream pump `select!` on cancellation and return at once;
/// dropping the response closes the connection, which the gateway
/// treats as a client disconnect and answers by interrupting the
/// agent (`_write_sse_responses`).
#[derive(Clone, Default)]
pub(crate) struct Cancel {
    flag: Arc<AtomicBool>,
    notify: Arc<tokio::sync::Notify>,
}

impl Cancel {
    pub(crate) fn cancel(&self) {
        self.flag.store(true, std::sync::atomic::Ordering::Relaxed);
        self.notify.notify_waiters();
    }

    pub(crate) fn is_cancelled(&self) -> bool {
        self.flag.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Resolves once cancelled. Registers the waiter *before*
    /// re-checking the flag so a cancel racing the await isn't lost.
    pub(crate) async fn cancelled(&self) {
        loop {
            let waiting = self.notify.notified();
            if self.is_cancelled() {
                return;
            }
            waiting.await;
            if self.is_cancelled() {
                return;
            }
        }
    }
}

/// Per-session bookkeeping.
pub(crate) struct SessionRow {
    pub(crate) session: Session,
    /// This session's publish end of the backend-wide event hub —
    /// `dispatch_turn`'s worker sends into it, wire subscribers
    /// attach to the hub behind the `Subscriptions` stream.
    pub(crate) events_tx: SessionEvents,
    /// Full transcript (user + assistant), chronological. Replayed
    /// as the `messages` array on each turn.
    pub(crate) messages: Vec<Message>,
    /// Tripped by `cancel_turn`; the streaming worker races it
    /// against the SSE body and stops immediately.
    pub(crate) cancel: Cancel,
    /// Response id of the last completed turn — chains the next one
    /// onto the same gateway-side session (see [`responses`]).
    pub(crate) last_response_id: String,
}

/// Handle to the Hermes backend. Clone-friendly — state lives
/// behind `Arc` so workers spawned from trait methods share the
/// session map.
#[derive(Clone, architect::HasDispatcher)]
pub struct HermesBackend {
    pub(crate) inner: Arc<HermesInner>,
}

pub(crate) struct HermesInner {
    pub(crate) config: HermesConfig,
    pub(crate) http: reqwest::Client,
    /// Runtime handle captured at construction — the sync Discovery
    /// methods run on blocking dispatcher threads and drive their
    /// HTTP hops through it.
    pub(crate) runtime: tokio::runtime::Handle,
    pub(crate) sessions: Mutex<HashMap<String, SessionRow>>,
    /// Latched once a gateway 404s `/v1/responses` — pre-Responses
    /// deployments shouldn't pay a failed round-trip per turn.
    pub(crate) legacy_transport: AtomicBool,
    /// Fan-out hub behind the `Subscriptions` `#[subscribe]` stream.
    /// Every session's [`SessionEvents`] publishes here, so ONE
    /// subscription carries every session's traffic (the envelope's
    /// `session_id` is the client's filter). Sliding mailbox: a slow
    /// subscriber loses its oldest queued events — for the settled
    /// transcript that's recoverable (re-pull on reconnect); for
    /// token deltas it only clips the animation.
    ///
    /// Shared, not owned: the server hands the same hub to every
    /// agent backend so the router can serve them from one stream.
    pub(crate) events: architect::PubSub<AgentEventEnvelope>,
}

impl HermesBackend {
    #[must_use]
    pub fn new(config: HermesConfig) -> Self {
        Self::with_events(config, architect::PubSub::sliding(512))
    }

    /// Same, sharing an existing event hub — how a multi-backend
    /// router gets one `Subscriptions` stream over several backends.
    #[must_use]
    pub fn with_events(
        config: HermesConfig,
        events: architect::PubSub<AgentEventEnvelope>,
    ) -> Self {
        Self {
            inner: Arc::new(HermesInner {
                config,
                http: reqwest::Client::new(),
                runtime: tokio::runtime::Handle::current(),
                sessions: Mutex::new(HashMap::new()),
                legacy_transport: AtomicBool::new(false),
                events,
            }),
        }
    }

    /// Construct from `TASK_HERMES_*` env vars; `None` when the
    /// gateway URL isn't configured.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        HermesConfig::from_env().map(Self::new)
    }

    /// [`Self::from_env`] on a shared event hub.
    #[must_use]
    pub fn from_env_with_events(events: architect::PubSub<AgentEventEnvelope>) -> Option<Self> {
        HermesConfig::from_env().map(|c| Self::with_events(c, events))
    }

    /// The hub every session of this backend publishes into.
    #[must_use]
    pub fn events(&self) -> &architect::PubSub<AgentEventEnvelope> {
        &self.inner.events
    }

    #[must_use]
    pub fn config(&self) -> &HermesConfig {
        &self.inner.config
    }

    /// Async session-existence probe — for callers already inside
    /// the runtime (the sync trait methods use `blocking_lock` and
    /// must not be called from async context).
    pub async fn has_session(&self, session_id: &str) -> bool {
        self.inner.sessions.lock().await.contains_key(session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cancel_wakes_a_waiter_immediately() {
        let cancel = Cancel::default();
        assert!(!cancel.is_cancelled());
        let watcher = cancel.clone();
        let waited = tokio::spawn(async move {
            tokio::time::timeout(std::time::Duration::from_secs(5), watcher.cancelled()).await
        });
        // Give the task a chance to park on `notified()`.
        tokio::task::yield_now().await;
        cancel.cancel();
        assert!(
            waited.await.expect("join").is_ok(),
            "cancel didn't wake the waiter"
        );
        assert!(cancel.is_cancelled());
    }

    #[tokio::test]
    async fn cancelling_before_the_await_is_not_lost() {
        // The race the plain `Notify` would drop: cancel fires before
        // anyone is waiting, so `notify_waiters` reaches nobody. The
        // flag check inside `cancelled()` has to catch it.
        let cancel = Cancel::default();
        cancel.cancel();
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), cancel.cancelled()).await;
        assert!(
            got.is_ok(),
            "an already-cancelled token must resolve at once"
        );
    }
}
