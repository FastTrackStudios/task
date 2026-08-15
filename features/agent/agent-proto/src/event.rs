//! Streaming event union. Backends publish these onto the
//! `Subscriptions` `events` stream, wrapped in an
//! [`AgentEventEnvelope`] that names the session. Mirrors Hermes's
//! SSE event types verbatim where it matters; `CodexMonitor`'s
//! `AppServerEvent` payloads translate into the same union.
//!
//! The variants are intentionally fine-grained — granular
//! events make it easy for UIs to render partial state
//! without re-pulling the whole session.

use chrono::{DateTime, Utc};
use facet::Facet;

use crate::{
    approval::Approval, message::Message, question::QuestionRequest, tasks::AgentTask,
    tool::ToolCall,
};

#[derive(Debug, Clone, PartialEq, Facet)]
#[repr(C)]
pub enum AgentEvent {
    /// Backend accepted a new turn and started a worker.
    /// Carries the stream id clients can use to subscribe.
    TurnStarted {
        session_id: String,
        stream_id: String,
        at: DateTime<Utc>,
    },
    /// One whole message landed (typically the user
    /// message at turn start).
    MessageWritten { message: Message },
    /// Streaming partial token — text body grew.
    /// `content_delta` is appended to the message's
    /// current text-content block.
    MessageDelta {
        session_id: String,
        message_id: String,
        content_delta: String,
    },
    /// Reasoning content streamed in.
    ReasoningDelta {
        session_id: String,
        message_id: String,
        delta: String,
    },
    /// A tool call was initiated.
    ToolStarted { tool_call: ToolCall },
    /// Tool produced an intermediate update (output line,
    /// progress percent).
    ToolProgress {
        tool_call_id: String,
        preview: String,
        /// 0.0–1.0; `-1.0` for indeterminate.
        progress: f32,
    },
    /// Tool finished (with success or error).
    ToolFinished { tool_call: ToolCall },
    /// Backend wants user approval for something.
    ApprovalRequested { approval: Approval },
    /// User answered an approval. Mirrored back on the
    /// stream for any other subscribers.
    ApprovalResolved { approval: Approval },
    /// Backend wants the user to answer a structured
    /// question.
    QuestionAsked { request: QuestionRequest },
    /// Question was answered.
    QuestionResolved { request: QuestionRequest },
    /// Context compression started — older messages are
    /// being summarized into the anchor.
    CompressionStarted { session_id: String, engine: String },
    /// Compression done; new anchor visible at
    /// `anchor_idx`.
    CompressionFinished { session_id: String, anchor_idx: u32 },
    /// Usage / metering tick. Emitted periodically while
    /// a turn runs.
    Metering {
        session_id: String,
        input_tokens: u64,
        output_tokens: u64,
        estimated_cost_usd: f32,
    },
    /// Backend status hint (`"falling back to model X"`,
    /// `"queued behind 3 other turns"`).
    Warning {
        session_id: String,
        kind: String,
        message: String,
    },
    /// Turn finished cleanly.
    TurnFinished {
        session_id: String,
        message_id: String,
        at: DateTime<Utc>,
    },
    /// Turn errored.
    TurnErrored {
        session_id: String,
        kind: String,
        message: String,
        at: DateTime<Utc>,
    },
    /// User cancelled mid-flight.
    TurnCancelled {
        session_id: String,
        at: DateTime<Utc>,
    },
    /// An agent task changed (created, moved, claimed,
    /// archived). Emitted on queue-scoped subscriptions.
    AgentTaskChanged { task: AgentTask },
    /// Generic resync signal — subscribers re-pull state.
    Resync,
}

/// One [`AgentEvent`] plus the session it belongs to — the payload
/// of [`crate::service::subscriptions::Subscriptions`]'s `events`
/// stream.
///
/// ## Why the wrapper
///
/// `#[subscribe]` streams take no filter params, so the scope has to
/// travel with the event: subscribers see every session the backend
/// runs and keep the one they're rendering. Most `AgentEvent`
/// variants already name their session, but not all
/// (`ToolProgress`, `ApprovalRequested`, …) — the envelope makes the
/// routing key uniform instead of per-variant.
///
/// ## Subscriber contract (changes only, no snapshot variant)
///
/// The stream carries *changes only*. A chat view fetches the
/// transcript once (`Threads::list_messages`, after subscribing so
/// nothing is missed in between) and then folds:
///
/// - message / tool / turn variants — apply to the local transcript
///   keyed by their ids. [`AgentEvent::MessageWritten`] carries the
///   full post-write [`Message`], so re-applying one already in the
///   fetched transcript is an idempotent replacement.
/// - [`AgentEvent::Resync`] — re-pull the transcript; events were
///   skipped.
///
/// [`AgentEvent::MessageDelta`] / [`AgentEvent::ReasoningDelta`] are
/// the exception: they are *diffs*, appended to a live buffer that
/// the terminating `MessageWritten` / `TurnFinished` replaces with
/// the authoritative message. A dropped delta therefore only
/// degrades the typewriter animation, never the settled transcript.
#[derive(Debug, Clone, PartialEq, Facet)]
pub struct AgentEventEnvelope {
    /// Session the event belongs to. Empty for cross-session news
    /// (the sidebar firehose).
    pub session_id: String,
    /// What happened.
    pub event: AgentEvent,
}

// SelfRef compatibility (vox channel `.get()`): neither type has
// lifetime parameters, so Ref<'a> = Self. Required to consume the
// `Subscriptions` stream through a
// `vox::channel::<AgentEventEnvelope>()` on the client.
#[cfg(feature = "vox")]
#[allow(unsafe_code)]
unsafe impl vox_types::Reborrow for AgentEvent {
    type Ref<'a> = AgentEvent;
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
unsafe impl vox_types::Reborrow for AgentEventEnvelope {
    type Ref<'a> = AgentEventEnvelope;
}

/// A session's publish end of the shared event hub.
///
/// Backends thread one of these through their turn workers instead
/// of a per-session `broadcast::Sender`: it remembers the session id
/// so every `send` stamps the right envelope, and it fans out to the
/// one process-wide [`architect::PubSub`] the `Subscriptions` stream
/// is served from. Cheap to clone — the hub is `Arc` inside, so
/// every clone reaches the same subscribers.
#[cfg(feature = "vox")]
#[derive(Clone)]
pub struct SessionEvents {
    hub: architect::PubSub<AgentEventEnvelope>,
    session_id: String,
    tap: Option<EventTap>,
}

#[cfg(feature = "vox")]
impl SessionEvents {
    /// Bind `hub` to `session_id`.
    #[must_use]
    pub fn new(hub: architect::PubSub<AgentEventEnvelope>, session_id: impl Into<String>) -> Self {
        Self {
            hub,
            session_id: session_id.into(),
            tap: None,
        }
    }

    /// A sink with an [`EventTap`] attached — see the tap's docs for
    /// why an in-process observer can't just attach to the hub.
    #[must_use]
    pub fn tapped(session_id: impl Into<String>) -> (Self, EventTap) {
        let tap = EventTap::default();
        let mut me = Self::new(architect::PubSub::sliding(64), session_id);
        me.tap = Some(tap.clone());
        (me, tap)
    }

    /// Publish one event for this session. Returns how many
    /// subscribers it reached — zero is normal (nobody is watching).
    /// Call only once the state it reports is committed.
    pub fn send(&self, event: AgentEvent) -> usize {
        if let Some(tap) = &self.tap {
            tap.push(event.clone());
        }
        self.hub.publish(AgentEventEnvelope {
            session_id: self.session_id.clone(),
            event,
        })
    }

    /// The session this sink stamps onto its events.
    #[must_use]
    pub fn session_id(&self) -> &str {
        &self.session_id
    }
}

/// Synchronous in-process tap on a [`SessionEvents`]: every event it
/// publishes is also queued here, in order.
///
/// [`architect::PubSub`] only fans out to `vox::Tx` sinks, and an
/// *unbound* vox channel pair can't be delivered into — sinks resolve
/// when a transport binds them. So code that wants to watch a turn's
/// events without standing up a server (the SSE-translation tests,
/// in-process bridges) taps the sink instead of subscribing to the
/// hub. Cheap to clone; all clones share one queue.
#[cfg(feature = "vox")]
#[derive(Clone, Default)]
pub struct EventTap(std::sync::Arc<std::sync::Mutex<std::collections::VecDeque<AgentEvent>>>);

#[cfg(feature = "vox")]
impl EventTap {
    fn push(&self, event: AgentEvent) {
        self.0.lock().expect("event tap poisoned").push_back(event);
    }

    /// Pop the oldest queued event, if any.
    #[must_use]
    pub fn try_next(&self) -> Option<AgentEvent> {
        self.0.lock().expect("event tap poisoned").pop_front()
    }

    /// How many events are queued.
    #[must_use]
    pub fn len(&self) -> usize {
        self.0.lock().expect("event tap poisoned").len()
    }

    /// Whether nothing is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(feature = "vox")]
impl std::fmt::Debug for EventTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("EventTap").field(&self.len()).finish()
    }
}

#[cfg(feature = "vox")]
impl std::fmt::Debug for SessionEvents {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionEvents")
            .field("session_id", &self.session_id)
            .finish_non_exhaustive()
    }
}
