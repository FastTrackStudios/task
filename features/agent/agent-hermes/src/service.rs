//! Hermes's per-capability impls.
//!
//! Same capability set as Codex — [`Sessions`], [`TurnDispatch`],
//! [`Threads`], [`Subscriptions`] — but the turn worker calls the
//! gateway's HTTP API instead of spawning a process, and the
//! transcript is first-class: user AND assistant messages are
//! stored, so `list_messages` returns a real conversation (Codex
//! only accumulates assistant deltas today).

use agent_proto::error::AgentError;
use agent_proto::event::{AgentEvent, AgentEventEnvelope, SessionEvents};
use agent_proto::message::{ContentBlock, Message, Role};
use agent_proto::service::sessions::{CreateSession, SessionFilter, SessionPage, Sessions};
use agent_proto::service::subscriptions::SubscriptionsStreamSource;
use agent_proto::service::threads::Threads;
use agent_proto::service::turn_dispatch::{DispatchAck, DispatchTurn, TurnDispatch};
use agent_proto::session::{Session, SessionStatus, SourceTag, UsageStats};
use chrono::Utc;
use uuid::Uuid;

use crate::stream::{run_turn, wire_messages};
use crate::{BACKEND_ID, HermesBackend, SessionRow};

// ────────────────────── Sessions ──────────────────────
impl Sessions for HermesBackend {
    fn create_session(&self, args: CreateSession) -> Result<Session, AgentError> {
        let id = format!("sess-{}", Uuid::new_v4().simple());
        let now = Utc::now();
        let session = Session {
            id: id.clone(),
            title: if args.title.is_empty() {
                "Hermes chat".to_string()
            } else {
                args.title
            },
            project_id: args.project_id,
            profile_id: args.profile_id,
            backend_id: BACKEND_ID.to_string(),
            workspace_path: args.workspace_path,
            status: SessionStatus::Idle,
            source: SourceTag::Api,
            subagent_nickname: args.subagent_nickname,
            pinned: false,
            archived: false,
            created_at: now,
            updated_at: now,
            last_message_at: None,
            pending: None,
            usage: UsageStats {
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                estimated_cost_usd: 0.0,
            },
            compression: None,
            worktree: None,
            composer_draft: agent_proto::session::ComposerDraft {
                text: String::new(),
                attachments: Vec::new(),
                updated_at: None,
            },
        };
        let events_tx = SessionEvents::new(self.inner.events.clone(), id.clone());
        let mut sessions = self.inner.sessions.blocking_lock();
        sessions.insert(
            id,
            SessionRow {
                session: session.clone(),
                events_tx,
                messages: Vec::new(),
                cancel: crate::Cancel::default(),
                last_response_id: String::new(),
            },
        );
        Ok(session)
    }

    fn read_session(&self, session_id: &str) -> Result<Session, AgentError> {
        let sessions = self.inner.sessions.blocking_lock();
        sessions
            .get(session_id)
            .map(|row| row.session.clone())
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))
    }

    fn list_sessions(&self, filter: SessionFilter) -> Result<SessionPage, AgentError> {
        let sessions = self.inner.sessions.blocking_lock();
        let mut out: Vec<Session> = sessions
            .values()
            .filter(|row| {
                if !filter.project_id.is_empty() && row.session.project_id != filter.project_id {
                    return false;
                }
                if !filter.profile_id.is_empty() && row.session.profile_id != filter.profile_id {
                    return false;
                }
                if !filter.backend_id.is_empty() && row.session.backend_id != filter.backend_id {
                    return false;
                }
                if !filter.include_archived && row.session.archived {
                    return false;
                }
                if filter.only_pinned && !row.session.pinned {
                    return false;
                }
                true
            })
            .map(|row| row.session.clone())
            .collect();
        out.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        if filter.limit > 0 {
            out.truncate(filter.limit as usize);
        }
        Ok(SessionPage {
            sessions: out,
            next_cursor: String::new(),
            has_more: false,
        })
    }

    fn rename_session(&self, session_id: &str, title: &str) -> Result<Session, AgentError> {
        self.with_row(session_id, |row| {
            row.session.title = title.to_string();
        })
    }

    fn pin_session(&self, session_id: &str, pinned: bool) -> Result<Session, AgentError> {
        self.with_row(session_id, |row| {
            row.session.pinned = pinned;
        })
    }

    fn archive_session(&self, session_id: &str, archived: bool) -> Result<Session, AgentError> {
        self.with_row(session_id, |row| {
            row.session.archived = archived;
        })
    }

    fn delete_session(&self, session_id: &str) -> Result<(), AgentError> {
        let mut sessions = self.inner.sessions.blocking_lock();
        if sessions.remove(session_id).is_none() {
            return Err(AgentError::SessionNotFound(session_id.to_string()));
        }
        Ok(())
    }

    fn save_composer_draft(
        &self,
        session_id: &str,
        text: &str,
        attachments: Vec<agent_proto::attachment::AttachmentRef>,
    ) -> Result<Session, AgentError> {
        self.with_row(session_id, |row| {
            row.session.composer_draft.text = text.to_string();
            row.session.composer_draft.attachments = attachments;
            row.session.composer_draft.updated_at = Some(Utc::now());
        })
    }
}

impl HermesBackend {
    /// Mutate one session row and return the updated session.
    fn with_row(
        &self,
        session_id: &str,
        f: impl FnOnce(&mut SessionRow),
    ) -> Result<Session, AgentError> {
        let mut sessions = self.inner.sessions.blocking_lock();
        let row = sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))?;
        f(row);
        row.session.updated_at = Utc::now();
        Ok(row.session.clone())
    }
}

// ────────────────────── TurnDispatch ──────────────────────
impl TurnDispatch for HermesBackend {
    fn dispatch_turn(&self, args: DispatchTurn) -> Result<DispatchAck, AgentError> {
        let started_at = Utc::now();
        let user_msg = Message {
            id: format!("msg-{}", Uuid::new_v4().simple()),
            session_id: args.session_id.clone(),
            role: Role::User,
            content: vec![ContentBlock::Text {
                text: args.text.clone(),
            }],
            partial: false,
            errored: false,
            error_text: String::new(),
            reasoning: None,
            created_at: started_at,
        };
        let assistant_id = format!("msg-{}", Uuid::new_v4().simple());

        let (events_tx, wire, cancel, chain) = {
            let mut sessions = self.inner.sessions.blocking_lock();
            let row = sessions
                .get_mut(&args.session_id)
                .ok_or_else(|| AgentError::SessionNotFound(args.session_id.clone()))?;
            if matches!(row.session.status, SessionStatus::Running) {
                return Err(AgentError::SessionBusy(args.session_id.clone()));
            }
            row.session.status = SessionStatus::Running;
            row.session.updated_at = started_at;
            row.session.composer_draft.text = String::new();
            row.messages.push(user_msg.clone());
            row.cancel = crate::Cancel::default();
            // `/new` wipes the gateway's context, so drop the chain
            // with it — otherwise the next turn resurrects the old
            // session server-side.
            if args.text.trim_start().starts_with("/new") {
                row.last_response_id.clear();
            }
            (
                row.events_tx.clone(),
                wire_messages(&row.messages),
                row.cancel.clone(),
                row.last_response_id.clone(),
            )
        };

        let backend = self.clone();
        let session_id = args.session_id.clone();
        let stream_id = format!("stream-{}", Uuid::new_v4().simple());
        let model = if args.model_override.is_empty() {
            backend.inner.config.model.clone()
        } else {
            args.model_override.clone()
        };
        let effective_model = model.clone();

        let stream_id_for_task = stream_id.clone();
        tokio::runtime::Handle::current().spawn(async move {
            let _ = events_tx.send(AgentEvent::TurnStarted {
                session_id: session_id.clone(),
                stream_id: stream_id_for_task,
                at: started_at,
            });
            let _ = events_tx.send(AgentEvent::MessageWritten { message: user_msg });

            let result = run_turn(
                &backend.inner.http,
                &backend.inner.config,
                &session_id,
                &session_id, // gateway session key = Task session id
                &assistant_id,
                &model,
                wire,
                &chain,
                &backend.inner.legacy_transport,
                &events_tx,
                &cancel,
            )
            .await;

            let now = Utc::now();
            let mut sessions = backend.inner.sessions.lock().await;
            let Some(row) = sessions.get_mut(&session_id) else {
                return;
            };
            match result {
                Ok(outcome) => {
                    let cancelled = cancel.is_cancelled();
                    let message = Message {
                        id: assistant_id.clone(),
                        session_id: session_id.clone(),
                        role: Role::Assistant,
                        content: vec![ContentBlock::Text {
                            text: outcome.text.clone(),
                        }],
                        partial: false,
                        errored: false,
                        error_text: String::new(),
                        // Retained so a reloaded transcript can still
                        // show what the agent was thinking.
                        reasoning: (!outcome.reasoning.is_empty()).then(|| {
                            agent_proto::reasoning::ReasoningBlock {
                                summary: outcome
                                    .reasoning
                                    .lines()
                                    .find(|l| !l.trim().is_empty())
                                    .unwrap_or_default()
                                    .to_string(),
                                content: outcome.reasoning.clone(),
                                effort: String::new(),
                                structured: false,
                            }
                        }),
                        created_at: now,
                    };
                    row.messages.push(message.clone());
                    if !outcome.response_id.is_empty() {
                        row.last_response_id = outcome.response_id.clone();
                    }
                    row.session.usage.input_tokens += outcome.input_tokens;
                    row.session.usage.output_tokens += outcome.output_tokens;
                    row.session.usage.estimated_cost_usd += outcome.cost_usd;
                    row.session.status = SessionStatus::Idle;
                    row.session.updated_at = now;
                    row.session.last_message_at = Some(now);
                    let _ = events_tx.send(AgentEvent::MessageWritten { message });
                    if cancelled {
                        let _ = events_tx.send(AgentEvent::TurnCancelled {
                            session_id: session_id.clone(),
                            at: now,
                        });
                    } else {
                        let _ = events_tx.send(AgentEvent::TurnFinished {
                            session_id: session_id.clone(),
                            message_id: assistant_id,
                            at: now,
                        });
                    }
                }
                Err(e) => {
                    row.messages.push(Message {
                        id: assistant_id.clone(),
                        session_id: session_id.clone(),
                        role: Role::Assistant,
                        content: Vec::new(),
                        partial: false,
                        errored: true,
                        error_text: e.clone(),
                        reasoning: None,
                        created_at: now,
                    });
                    row.session.status = SessionStatus::Errored;
                    row.session.updated_at = now;
                    let _ = events_tx.send(AgentEvent::TurnErrored {
                        session_id: session_id.clone(),
                        kind: "gateway_error".to_string(),
                        message: e,
                        at: now,
                    });
                }
            }
        });

        Ok(DispatchAck {
            session_id: args.session_id,
            stream_id,
            turn_id: 0,
            started_at,
            effective_model,
            effective_backend_id: BACKEND_ID.to_string(),
            effective_profile_id: args.profile_override_id,
        })
    }

    fn cancel_turn(&self, session_id: &str) -> Result<(), AgentError> {
        let mut sessions = self.inner.sessions.blocking_lock();
        let row = sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))?;
        row.cancel.cancel();
        Ok(())
    }

    fn resume_session(&self, session_id: &str) -> Result<DispatchAck, AgentError> {
        Err(AgentError::Unsupported {
            backend: BACKEND_ID.to_string(),
            operation: format!("resume_session({session_id})"),
        })
    }
}

// ────────────────────── Threads ──────────────────────
impl Threads for HermesBackend {
    fn list_messages(
        &self,
        session_id: &str,
        limit: u32,
        _before_cursor: &str,
    ) -> Result<Vec<Message>, AgentError> {
        let sessions = self.inner.sessions.blocking_lock();
        let row = sessions
            .get(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))?;
        // Newest-first per the trait contract.
        let mut out: Vec<Message> = row.messages.iter().rev().cloned().collect();
        if limit > 0 {
            out.truncate(limit as usize);
        }
        Ok(out)
    }

    fn read_message(&self, message_id: &str) -> Result<Message, AgentError> {
        let sessions = self.inner.sessions.blocking_lock();
        for row in sessions.values() {
            if let Some(m) = row.messages.iter().find(|m| m.id == message_id) {
                return Ok(m.clone());
            }
        }
        Err(AgentError::MessageNotFound(message_id.to_string()))
    }

    fn append_note(&self, session_id: &str, text: &str) -> Result<Message, AgentError> {
        let note = Message {
            id: format!("msg-{}", Uuid::new_v4().simple()),
            session_id: session_id.to_string(),
            role: Role::System,
            content: vec![ContentBlock::Text {
                text: text.to_string(),
            }],
            partial: false,
            errored: false,
            error_text: String::new(),
            reasoning: None,
            created_at: Utc::now(),
        };
        let mut sessions = self.inner.sessions.blocking_lock();
        let row = sessions
            .get_mut(session_id)
            .ok_or_else(|| AgentError::SessionNotFound(session_id.to_string()))?;
        row.messages.push(note.clone());
        row.session.updated_at = Utc::now();
        Ok(note)
    }
}

// ────────────────────── Subscriptions ──────────────────────
/// The `#[subscribe]` backend contract. Publishing happens through
/// each session's [`SessionEvents`] as its turn runs; the stream
/// host attaches every subscriber sink to this one hub.
impl SubscriptionsStreamSource for HermesBackend {
    fn events_hub(&self) -> &architect::PubSub<AgentEventEnvelope> {
        &self.inner.events
    }
}
