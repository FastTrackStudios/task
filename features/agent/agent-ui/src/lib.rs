//! Agents — sessions, the fleet surface, and everything they call.
//!
//! Two screens over one domain: the **conversation** UI (sessions,
//! messages, dispatch, the inspector) and the **fleet surface** —
//! everything blocking a human across every project.
//!
//! Kept as a plugin rather than folded into the platform, deliberately.
//! An agent is the one part of Task that acts on its own, and "off"
//! should be a state an org can be in without arguing for it. Nothing
//! else here depends on agents existing.
//!
//! Mounted by `task-plugin-agent`.

use task_ui_core::feeds;
use task_ui_core::feeds::collect;

/// This app's id in Task's catalog, and the first segment of every
/// link it writes to itself.
pub const APP_ID: &str = "agent";

// ── Agents ────────────────────────────────────────────────────────

/// Agent sessions across the selected orgs (concurrent fan-out).
///
/// Each session carries its owning org slug so the `/agents` page can
/// show provenance in multi-org "All" mode. Archived sessions are
/// included so the listing is a faithful mirror of the backend.
pub async fn fetch_agent_sessions(
    slugs: &[String],
) -> Result<Vec<(String, agent_proto::session::Session)>, String> {
    let futs = slugs.iter().map(|slug| async move {
        let client = task_ui_core::vox_clients::establish_for::<
            agent_proto::service::sessions::SessionsClient,
        >(slug)
        .await?;
        let filter = agent_proto::service::sessions::SessionFilter {
            project_id: String::new(),
            backend_id: String::new(),
            profile_id: String::new(),
            include_archived: true,
            only_pinned: false,
            limit: 0,
            cursor: String::new(),
        };
        let page = client
            .list_sessions(filter)
            .await
            .map_err(|e| format!("{slug}: list agent sessions: {e:?}"))?;
        Ok::<_, String>(
            page.sessions
                .into_iter()
                .map(|s| (slug.clone(), s))
                .collect::<Vec<_>>(),
        )
    });
    collect(futures_util::future::join_all(futs).await)
}

/// Agent sessions attached to one project, filtered server-side
/// (`SessionFilter.project_id` is an exact match in the backend).
/// Archived sessions are excluded — this powers "active now" style
/// views, and an archived session can't be active.
pub async fn fetch_project_agent_sessions(
    slug: &str,
    project_id: &str,
) -> Result<Vec<agent_proto::session::Session>, String> {
    let client = task_ui_core::vox_clients::establish_for::<
        agent_proto::service::sessions::SessionsClient,
    >(slug)
    .await?;
    let filter = agent_proto::service::sessions::SessionFilter {
        project_id: project_id.to_owned(),
        backend_id: String::new(),
        profile_id: String::new(),
        include_archived: false,
        only_pinned: false,
        limit: 0,
        cursor: String::new(),
    };
    let page = client
        .list_sessions(filter)
        .await
        .map_err(|e| format!("{slug}: list agent sessions: {e:?}"))?;
    Ok(page.sessions)
}

feeds! {
    agent_proto::service::sessions::SessionsClient {
        /// Create a new agent chat session. `backend_id` picks the backend
        /// (`"hermes"`, `"codex"`); empty = the server default (Hermes when
        /// a gateway is configured).
        create_agent_session(backend_id: &str, title: &str) -> agent_proto::session::Session
            = create_session(agent_proto::service::sessions::CreateSession { project_id: String::new(), profile_id: String::new(), backend_id: backend_id.to_owned(), title: title.to_owned(), workspace_path: String::new(), subagent_nickname: String::new(), }) as "create agent session";

        /// Session mutations for the rail/inspector: rename, pin, archive,
        /// delete. Each returns the updated session (delete returns unit).
        rename_agent_session(session_id: &str, title: &str) -> agent_proto::session::Session
            = rename_session(session_id.to_owned(), title.to_owned()) as "rename session";

        pin_agent_session(session_id: &str, pinned: bool) -> agent_proto::session::Session
            = pin_session(session_id.to_owned(), pinned) as "pin session";

        archive_agent_session(session_id: &str, archived: bool) -> agent_proto::session::Session
            = archive_session(session_id.to_owned(), archived) as "archive session";

        delete_agent_session(session_id: &str) -> ()
            = delete_session(session_id.to_owned()) as "delete session";
    }

    agent_proto::service::turn_dispatch::TurnDispatchClient {
        /// Kick off one turn — the user message goes to the session's
        /// backend; the reply arrives on the `Subscriptions` events
        /// stream the chat view holds open.
        dispatch_agent_turn(session_id: &str, text: &str, model_override: &str) -> agent_proto::service::turn_dispatch::DispatchAck
            = dispatch_turn(agent_proto::service::turn_dispatch::DispatchTurn { session_id: session_id.to_owned(), text: text.to_owned(), attachments: Vec::new(), profile_override_id: String::new(), personality_override_id: String::new(), model_override: model_override.to_owned(), }) as "dispatch turn";

        /// Cancel the in-flight turn on a session.
        cancel_agent_turn(session_id: &str) -> ()
            = cancel_turn(session_id.to_owned()) as "cancel turn";
    }

    agent_proto::service::threads::ThreadsClient {
        /// Full transcript for a session (backend returns newest-first;
        /// callers reverse for display).
        fetch_agent_messages(session_id: &str) -> Vec<agent_proto::message::Message>
            = list_messages(session_id.to_owned(), 0, String::new()) as "list messages";
    }

    agent_proto::service::discovery::DiscoveryClient {
        /// Live model list across the org's agent backends (Hermes gateway
        /// models + Codex's static set) — feeds the composer's model chip.
        fetch_agent_models() -> Vec<agent_proto::service::discovery::ModelInfo>
            = list_models(String::new()) as "agent models";

        /// Agent skills (Hermes's self-improving skill library).
        fetch_agent_skills() -> Vec<agent_proto::service::discovery::SkillInfo>
            = list_skills(String::new()) as "agent skills";

        /// Backend capability flags, for the inspector panel.
        fetch_agent_capabilities() -> Vec<agent_proto::service::discovery::CapabilityFlag>
            = list_capabilities(String::new()) as "agent capabilities";

        /// Live per-backend health — gateway state, connected platforms,
        /// in-flight agents, probe latency. Polled by the chat header's
        /// status chip so an unreachable gateway says so instead of
        /// silently swallowing turns.
        fetch_agent_health() -> Vec<agent_proto::backend::BackendHealth>
            = backend_health(String::new()) as "agent health";
    }

    agent_proto::service::routines::RoutinesClient {
        /// Scheduled agent routines (the Hermes gateway's cron jobs).
        /// Includes paused ones — the panel shows them greyed rather than
        /// hiding them, so a paused routine isn't mistaken for a deleted one.
        fetch_agent_routines() -> Vec<agent_proto::service::routines::Routine>
            = list_routines(String::new(), true) as "agent routines";

        create_agent_routine(routine: agent_proto::service::routines::NewRoutine) -> agent_proto::service::routines::Routine
            = create_routine(routine) as "create routine";

        set_agent_routine_paused(id: &str, paused: bool) -> agent_proto::service::routines::Routine
            = set_routine_paused(String::new(), id.to_owned(), paused) as "pause routine";

        run_agent_routine(id: &str) -> agent_proto::service::routines::Routine
            = run_routine(String::new(), id.to_owned()) as "run routine";

        delete_agent_routine(id: &str) -> ()
            = delete_routine(String::new(), id.to_owned()) as "delete routine";
    }
}

/// Everything blocking a human in the agent lane, for one project or
/// the whole fleet.
///
/// One fetch rather than three so the panels always describe the same
/// instant — a surface where "running" and "awaiting review" disagree
/// about a ticket is worse than one that is slightly stale.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AgentSurface {
    /// Unresolved questions, paired with the ticket each blocks.
    pub questions: Vec<(
        agent_proto::question::QuestionRequest,
        Option<task_proto::TaskInfo>,
    )>,
    /// Runs executing right now, paired with their ticket.
    pub running: Vec<(agent_proto::run::Run, Option<task_proto::TaskInfo>)>,
    /// Tickets whose branch is green and waiting.
    pub review: Vec<task_proto::TaskInfo>,
}

/// Fetch the agent surface. `project` scopes it; `None` is the fleet.
///
/// # Errors
///
/// The first transport failure. The surface is all-or-nothing on
/// purpose: a panel that renders empty because its call failed reads
/// as "nothing is blocking you", which is the opposite of the truth.
pub async fn fetch_agent_surface(
    slug: &str,
    project: Option<uuid::Uuid>,
) -> Result<AgentSurface, String> {
    let tasks =
        task_ui_core::vox_clients::establish_for::<task_proto::TaskServiceClient>(slug).await?;
    let all = tasks.list().await.map_err(|e| format!("{e:?}"))?;
    let in_scope = |t: &task_proto::TaskInfo| project.is_none_or(|p| t.project_id == Some(p));
    let find = |id: uuid::Uuid| all.iter().find(|t| t.id == id).cloned();

    let questions_client = task_ui_core::vox_clients::establish_for::<
        agent_proto::service::questions::QuestionsClient,
    >(slug)
    .await?;
    let mut questions = Vec::new();
    for req in questions_client
        .unresolved_questions()
        .await
        .map_err(|e| format!("{e:?}"))?
    {
        let ticket = questions_client
            .question_ticket(req.id.clone())
            .await
            .ok()
            .flatten()
            .and_then(find);
        // A question whose ticket is out of scope belongs to another
        // project's surface. One with no ticket at all is still shown:
        // it is blocked on a human either way, and hiding it loses it.
        if ticket.as_ref().is_none_or(in_scope) {
            questions.push((req, ticket));
        }
    }

    let runs_client =
        task_ui_core::vox_clients::establish_for::<agent_proto::service::runs::RunsClient>(slug)
            .await?;
    let running: Vec<_> = runs_client
        .list_runs(agent_proto::run::RunFilter {
            status: Some(agent_proto::run::RunStatus::InProgress),
            ..Default::default()
        })
        .await
        .map_err(|e| format!("{e:?}"))?
        .into_iter()
        .map(|r| {
            let t = find(r.ticket);
            (r, t)
        })
        .filter(|(_, t)| t.as_ref().is_none_or(in_scope))
        .collect();

    let review: Vec<task_proto::TaskInfo> = all
        .iter()
        .filter(|t| task_proto::has_triage_label(t, task_proto::TriageLabel::NeedsReview))
        .filter(|t| in_scope(t))
        .cloned()
        .collect();

    Ok(AgentSurface {
        questions,
        running,
        review,
    })
}

pub mod panel;
pub mod routines;
pub mod sessions;
pub mod surface;

pub use panel::{AgentPanel, AgentPanelSelected};
pub use sessions::AgentsView;
pub use surface::AgentSurfaceView;

/// Install this app's contexts at the app root.
///
/// One thing: which conversation the docked panel is showing. It has
/// to outlive the panel being toggled shut and the route changing
/// under it, and the app root is the only place that does.
pub fn provide_stores() {
    use dioxus::prelude::*;
    let _ = use_context_provider(|| Signal::new(AgentPanelSelected(String::new())));
}
