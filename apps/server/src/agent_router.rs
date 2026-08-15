//! Backend router for the agent services — one `Sessions` /
//! `TurnDispatch` / `Threads` / `Subscriptions` surface over
//! multiple backends (Codex, Hermes).
//!
//! Sessions are owned by exactly one backend; ownership is decided
//! at `create_session` (the new `CreateSession.backend_id` field,
//! empty = the server default: Hermes when a gateway is configured
//! — it's the primary conversational agent — else Codex) and every
//! later call routes by looking the session up in each backend's
//! registry. `list_sessions` merges both registries so the sidebar
//! shows one timeline.

use agent_codex::CodexBackend;
use agent_hermes::HermesBackend;
use agent_proto::error::AgentError;
use agent_proto::event::AgentEventEnvelope;
use agent_proto::message::Message;
use agent_proto::service::discovery::{
    BackendHealth, CapabilityFlag, Discovery, ModelInfo, SkillInfo,
};
use agent_proto::service::routines::{NewRoutine, Routine, Routines};
use agent_proto::service::sessions::{CreateSession, SessionFilter, SessionPage, Sessions};
use agent_proto::service::subscriptions::SubscriptionsStreamSource;
use agent_proto::service::threads::Threads;
use agent_proto::service::turn_dispatch::{DispatchAck, DispatchTurn, TurnDispatch};
use agent_proto::session::Session;

#[derive(Clone, architect::HasDispatcher)]
pub struct AgentRouter {
    codex: CodexBackend,
    hermes: Option<HermesBackend>,
    /// The one event hub both backends publish into — see
    /// [`AgentRouter::new`]. Serving `Subscriptions` from a single
    /// hub is what lets one client subscription cover sessions
    /// owned by either backend.
    events: architect::PubSub<AgentEventEnvelope>,
}

/// Which backend owns a session.
enum Owner {
    Codex,
    Hermes,
}

impl AgentRouter {
    /// Route over `codex` + optional `hermes`, all three sharing
    /// `events`.
    ///
    /// The hub must be the same one both backends were built with
    /// (`CodexBackend::with_events` / `HermesBackend::with_events`):
    /// a `#[subscribe]` stream is served from exactly one hub, and
    /// the router's job is to make two backends look like one
    /// service. Ownership routing still happens per call for
    /// everything else — only the event feed is merged.
    pub fn new(
        codex: CodexBackend,
        hermes: Option<HermesBackend>,
        events: architect::PubSub<AgentEventEnvelope>,
    ) -> Self {
        Self {
            codex,
            hermes,
            events,
        }
    }

    /// Resolve a session's owning backend. Registries are
    /// in-memory maps, so probing both is cheap.
    fn owner(&self, session_id: &str) -> Result<Owner, AgentError> {
        if let Some(h) = &self.hermes {
            if h.read_session(session_id).is_ok() {
                return Ok(Owner::Hermes);
            }
        }
        if self.codex.read_session(session_id).is_ok() {
            return Ok(Owner::Codex);
        }
        Err(AgentError::SessionNotFound(session_id.to_string()))
    }

    fn hermes(&self) -> Result<&HermesBackend, AgentError> {
        self.hermes
            .as_ref()
            .ok_or_else(|| AgentError::BackendNotFound(agent_hermes::BACKEND_ID.to_string()))
    }
}

/// Route a by-session-id call to its owning backend.
macro_rules! route {
    ($self:ident, $sid:expr, $method:ident ( $($arg:expr),* )) => {
        match $self.owner($sid)? {
            Owner::Hermes => $self.hermes()?.$method($($arg),*),
            Owner::Codex => $self.codex.$method($($arg),*),
        }
    };
}

impl Sessions for AgentRouter {
    fn create_session(&self, args: CreateSession) -> Result<Session, AgentError> {
        match args.backend_id.as_str() {
            "hermes" => self.hermes()?.create_session(args),
            "codex" => self.codex.create_session(args),
            // Default backend: the conversational agent when a
            // Hermes gateway is configured, else Codex.
            "" => match &self.hermes {
                Some(h) => h.create_session(args),
                None => self.codex.create_session(args),
            },
            other => Err(AgentError::BackendNotFound(other.to_string())),
        }
    }

    fn read_session(&self, session_id: &str) -> Result<Session, AgentError> {
        route!(self, session_id, read_session(session_id))
    }

    fn list_sessions(&self, filter: SessionFilter) -> Result<SessionPage, AgentError> {
        let mut page = self.codex.list_sessions(filter.clone())?;
        if let Some(h) = &self.hermes {
            let hermes_page = h.list_sessions(filter.clone())?;
            page.sessions.extend(hermes_page.sessions);
        }
        page.sessions
            .sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        if filter.limit > 0 {
            page.sessions.truncate(filter.limit as usize);
        }
        Ok(page)
    }

    fn rename_session(&self, session_id: &str, title: &str) -> Result<Session, AgentError> {
        route!(self, session_id, rename_session(session_id, title))
    }

    fn pin_session(&self, session_id: &str, pinned: bool) -> Result<Session, AgentError> {
        route!(self, session_id, pin_session(session_id, pinned))
    }

    fn archive_session(&self, session_id: &str, archived: bool) -> Result<Session, AgentError> {
        route!(self, session_id, archive_session(session_id, archived))
    }

    fn delete_session(&self, session_id: &str) -> Result<(), AgentError> {
        route!(self, session_id, delete_session(session_id))
    }

    fn save_composer_draft(
        &self,
        session_id: &str,
        text: &str,
        attachments: Vec<agent_proto::attachment::AttachmentRef>,
    ) -> Result<Session, AgentError> {
        route!(
            self,
            session_id,
            save_composer_draft(session_id, text, attachments)
        )
    }
}

impl TurnDispatch for AgentRouter {
    fn dispatch_turn(&self, args: DispatchTurn) -> Result<DispatchAck, AgentError> {
        match self.owner(&args.session_id)? {
            Owner::Hermes => self.hermes()?.dispatch_turn(args),
            Owner::Codex => self.codex.dispatch_turn(args),
        }
    }

    fn cancel_turn(&self, session_id: &str) -> Result<(), AgentError> {
        route!(self, session_id, cancel_turn(session_id))
    }

    fn resume_session(&self, session_id: &str) -> Result<DispatchAck, AgentError> {
        route!(self, session_id, resume_session(session_id))
    }
}

impl Threads for AgentRouter {
    fn list_messages(
        &self,
        session_id: &str,
        limit: u32,
        before_cursor: &str,
    ) -> Result<Vec<Message>, AgentError> {
        route!(
            self,
            session_id,
            list_messages(session_id, limit, before_cursor)
        )
    }

    fn read_message(&self, message_id: &str) -> Result<Message, AgentError> {
        if let Some(h) = &self.hermes {
            if let Ok(m) = h.read_message(message_id) {
                return Ok(m);
            }
        }
        self.codex.read_message(message_id)
    }

    fn append_note(&self, session_id: &str, text: &str) -> Result<Message, AgentError> {
        route!(self, session_id, append_note(session_id, text))
    }
}

impl Discovery for AgentRouter {
    fn list_models(&self, backend_id: &str) -> Result<Vec<ModelInfo>, AgentError> {
        let mut out = Vec::new();
        if backend_id.is_empty() || backend_id == "hermes" {
            if let Some(h) = &self.hermes {
                match h.list_models(backend_id) {
                    Ok(mut m) => out.append(&mut m),
                    Err(e) => tracing::warn!("hermes list_models: {e}"),
                }
            }
        }
        if backend_id.is_empty() || backend_id == "codex" {
            // Codex has no discovery API — its usual model set, default
            // first. Free-form overrides still work via DispatchTurn.
            for (i, id) in ["gpt-5.5-codex", "gpt-5.5", "o5-mini"].iter().enumerate() {
                out.push(ModelInfo {
                    backend_id: "codex".to_string(),
                    id: (*id).to_string(),
                    label: String::new(),
                    is_default: i == 0,
                    context_length: 0,
                    provider_id: "openai".to_string(),
                    provider_name: "OpenAI".to_string(),
                    reasoning: true,
                    cost_in_per_mtok: 0.0,
                    cost_out_per_mtok: 0.0,
                });
            }
        }
        Ok(out)
    }

    fn list_skills(&self, backend_id: &str) -> Result<Vec<SkillInfo>, AgentError> {
        match &self.hermes {
            Some(h) if backend_id.is_empty() || backend_id == "hermes" => h.list_skills(backend_id),
            _ => Ok(Vec::new()),
        }
    }

    fn list_capabilities(&self, backend_id: &str) -> Result<Vec<CapabilityFlag>, AgentError> {
        match &self.hermes {
            Some(h) if backend_id.is_empty() || backend_id == "hermes" => {
                h.list_capabilities(backend_id)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn backend_health(&self, backend_id: &str) -> Result<Vec<BackendHealth>, AgentError> {
        let mut out = Vec::new();
        if backend_id.is_empty() || backend_id == "hermes" {
            match &self.hermes {
                Some(h) => out.extend(h.backend_health(backend_id)?),
                // Configured-off is a status too: the UI says "no
                // gateway configured" instead of silently offering
                // a backend that can't answer.
                None => out.push(BackendHealth {
                    backend_id: "hermes".to_string(),
                    reachable: false,
                    last_ping_ms: 0,
                    version: String::new(),
                    status_text: "TASK_HERMES_URL is not set on the server".to_string(),
                    state: "unconfigured".to_string(),
                    active_agents: 0,
                    platforms: Vec::new(),
                    model: String::new(),
                    at: chrono::Utc::now(),
                }),
            }
        }
        if backend_id.is_empty() || backend_id == "codex" {
            // Codex runs in-process (spawned per session) — no probe
            // to make, so report it as available.
            out.push(BackendHealth {
                backend_id: "codex".to_string(),
                reachable: true,
                last_ping_ms: 0,
                version: String::new(),
                status_text: "in-process app-server".to_string(),
                state: "local".to_string(),
                active_agents: 0,
                platforms: Vec::new(),
                model: String::new(),
                at: chrono::Utc::now(),
            });
        }
        Ok(out)
    }
}

/// The `#[subscribe]` backend contract: the shared hub both agent
/// backends publish into, so one subscription carries every session
/// regardless of which backend runs it.
impl SubscriptionsStreamSource for AgentRouter {
    fn events_hub(&self) -> &architect::PubSub<AgentEventEnvelope> {
        &self.events
    }
}

impl Routines for AgentRouter {
    fn list_routines(
        &self,
        backend_id: &str,
        include_disabled: bool,
    ) -> Result<Vec<Routine>, AgentError> {
        // Only Hermes schedules today. An unconfigured gateway is an
        // empty list, not an error — the page renders its empty state.
        match &self.hermes {
            Some(h) if backend_id.is_empty() || backend_id == "hermes" => {
                h.list_routines(backend_id, include_disabled)
            }
            _ => Ok(Vec::new()),
        }
    }

    fn create_routine(&self, routine: NewRoutine) -> Result<Routine, AgentError> {
        self.hermes()?.create_routine(routine)
    }

    fn set_routine_paused(
        &self,
        backend_id: &str,
        id: &str,
        paused: bool,
    ) -> Result<Routine, AgentError> {
        self.hermes()?.set_routine_paused(backend_id, id, paused)
    }

    fn run_routine(&self, backend_id: &str, id: &str) -> Result<Routine, AgentError> {
        self.hermes()?.run_routine(backend_id, id)
    }

    fn delete_routine(&self, backend_id: &str, id: &str) -> Result<(), AgentError> {
        self.hermes()?.delete_routine(backend_id, id)
    }
}
