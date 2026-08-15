//! `agent-codex` — Codex backend for the agent feature.
//!
//! Vendors `CodexMonitor`'s tokio Rust control plane for
//! `codex app-server` under [`vendor`] (~3100 LOC, mostly
//! verbatim) and exposes a wrapper that implements
//! `agent_proto::Agents` on top of it.
//!
//! ## Surface
//!
//! - `CodexBackend` — top-level handle, cheaply clonable.
//!   Owns a `BroadcastSink` (vendor `EventSink`), a session
//!   registry keyed by `session_id`, and per-session
//!   broadcast channels with translated
//!   `agent_proto::AgentEvent`s.
//! - `CodexBackend::subscribe_raw()` — raw event firehose
//!   for debug / advanced consumers.
//! - `CodexBackend::chat()` — single-turn convenience used
//!   by the CLI demo and by `agent-wiki`'s ingest bridge.
//! - `impl Agents for CodexBackend` — proto-shaped surface.
//!   Codex-relevant methods (sessions, `dispatch_turn`,
//!   the event stream) are real; non-Codex methods
//!   (profiles, kanban, projects, ...) return
//!   `AgentError::Unsupported`.

#[path = "../vendor/mod.rs"]
mod vendor;

mod chat;
mod service;
mod translate;

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, broadcast};

use agent_proto::event::{AgentEventEnvelope, SessionEvents};
use agent_proto::session::Session;

pub use chat::{ChatHandle, ChatOpts};
pub use vendor::events::AppServerEvent;

/// Sink that fans `AppServerEvent`s out to a broadcast
/// channel. Implements the vendored `EventSink` trait;
/// `CodexBackend` translates each event into an
/// `agent_proto::AgentEvent` for the trait's subscription
/// channels.
#[derive(Clone)]
pub struct BroadcastSink {
    tx: broadcast::Sender<AppServerEvent>,
}

impl BroadcastSink {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        let (tx, _rx) = broadcast::channel(capacity);
        Self { tx }
    }

    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<AppServerEvent> {
        self.tx.subscribe()
    }
}

impl vendor::events::EventSink for BroadcastSink {
    fn emit_app_server_event(&self, event: AppServerEvent) {
        let _ = self.tx.send(event);
    }
}

/// Per-session bookkeeping carried by [`CodexBackend`].
pub(crate) struct SessionRow {
    pub(crate) session: Session,
    /// This session's publish end of the backend-wide event
    /// hub. `dispatch_turn` sends into it; wire subscribers
    /// (UIs, CLIs, agent-wiki bridges) attach to the hub
    /// behind the `Subscriptions` stream.
    pub(crate) events_tx: SessionEvents,
    /// Accumulated assistant text keyed by message id —
    /// `list_messages` rebuilds `Message`s from this.
    pub(crate) accumulated: HashMap<String, String>,
}

/// Top-level handle to the Codex backend. Clone-friendly:
/// the inner state lives behind `Arc`, so workers spawned
/// from trait methods share the same session map.
#[derive(Clone, architect::HasDispatcher)]
pub struct CodexBackend {
    inner: Arc<CodexInner>,
}

pub(crate) struct CodexInner {
    pub(crate) sink: BroadcastSink,
    pub(crate) sessions: Mutex<HashMap<String, SessionRow>>,
    /// Fan-out hub behind the `Subscriptions` `#[subscribe]`
    /// stream — every session's [`SessionEvents`] publishes
    /// here, so ONE subscription carries every session (the
    /// envelope's `session_id` is the client's filter).
    /// Shared, not owned: the server hands the same hub to
    /// every agent backend so the router serves them as one
    /// stream.
    pub(crate) events: architect::PubSub<AgentEventEnvelope>,
}

impl CodexBackend {
    #[must_use]
    pub fn new() -> Self {
        Self::with_events(architect::PubSub::sliding(512))
    }

    /// Same, sharing an existing event hub — how a
    /// multi-backend router gets one `Subscriptions` stream
    /// over several backends.
    #[must_use]
    pub fn with_events(events: architect::PubSub<AgentEventEnvelope>) -> Self {
        Self {
            inner: Arc::new(CodexInner {
                sink: BroadcastSink::new(1024),
                sessions: Mutex::new(HashMap::new()),
                events,
            }),
        }
    }

    /// The hub every session of this backend publishes into.
    #[must_use]
    pub fn events(&self) -> &architect::PubSub<AgentEventEnvelope> {
        &self.inner.events
    }

    /// Subscribe to the raw Codex event stream — every
    /// `AppServerEvent` regardless of workspace. UIs prefer
    /// the `Subscriptions` `events` stream (translated,
    /// session-stamped) instead.
    /// Async session-existence probe — for callers already inside
    /// the runtime (the sync trait methods use `blocking_lock` and
    /// must not be called from async context).
    pub async fn has_session(&self, session_id: &str) -> bool {
        self.inner.sessions.lock().await.contains_key(session_id)
    }

    #[must_use]
    pub fn subscribe_raw(&self) -> broadcast::Receiver<AppServerEvent> {
        self.inner.sink.subscribe()
    }

    /// Clone of the sink so callers can spawn workspace
    /// sessions with it.
    #[must_use]
    pub fn sink(&self) -> BroadcastSink {
        self.inner.sink.clone()
    }
}

impl Default for CodexBackend {
    fn default() -> Self {
        Self::new()
    }
}
