//! The concrete **coding** workflow.
//!
//! One real implementation, no trait. It drives a `WorkSession`
//! through the `task code` loop — start → commit → push → review →
//! merge — recording every move as a [`Transition`] and every event
//! (commit, comment, tool call) as an [`Activity`], and assembling a
//! full [`ResumeContext`] on resume.
//!
//! Methods are plain inherent `async`-free fns: the only IO is the
//! file store, which is synchronous, so there's nothing to await.
//! When a second workflow appears we'll extract the shared trait
//! from the two concrete shapes — see `workflows-proto::resume`.

use uuid::Uuid;
use workflows_proto::{
    Activity, ActivityKind, AgentRef, Handoff, HandoffReason, HandoffStatus, ResumeContext,
    SessionStatus, SubjectRef, Transition, TransitionState, WorkSession, WorkflowError,
    WorkflowKind,
};

use crate::store::WorkflowStore;

/// The coding workflow's state machine.
///
/// `Branched` is the entry state (`task code start` cuts the
/// branch). The rest mirror the loop verbs. Terminal close is the
/// session's [`SessionStatus`], not a state here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodingState {
    /// Branch cut, claim held, nothing committed yet.
    Branched,
    /// At least one commit on the branch.
    Committed,
    /// Branch pushed; a PR exists.
    Pushed,
    /// PR is under review.
    Reviewing,
    /// PR merged — work landed.
    Merged,
}

impl CodingState {
    /// Stable lowercase name stored in [`Transition`] rows.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Branched => "branched",
            Self::Committed => "committed",
            Self::Pushed => "pushed",
            Self::Reviewing => "reviewing",
            Self::Merged => "merged",
        }
    }

    /// Parse back from the stored name.
    #[must_use]
    pub fn from_name(s: &str) -> Option<Self> {
        Some(match s {
            "branched" => Self::Branched,
            "committed" => Self::Committed,
            "pushed" => Self::Pushed,
            "reviewing" => Self::Reviewing,
            "merged" => Self::Merged,
            _ => return None,
        })
    }

    /// Legal forward (and self-loop) edges. Models a real loop:
    /// you can keep committing, push more commits, and review can
    /// bounce back to more commits.
    #[must_use]
    pub fn can_transition_to(self, to: Self) -> bool {
        use CodingState::{Branched, Committed, Merged, Pushed, Reviewing};
        matches!(
            (self, to),
            // (re)commit — from the branch, after a push, or to
            // address review feedback.
            (Branched | Committed | Pushed | Reviewing, Committed)
                | (Committed, Pushed)
                | (Pushed, Reviewing)
                // merge, with or without an explicit review state.
                | (Pushed | Reviewing, Merged)
        )
    }
}

impl From<CodingState> for TransitionState {
    fn from(s: CodingState) -> Self {
        TransitionState::new(s.name())
    }
}

/// How many recent activities [`resume`](CodingWorkflow::resume)
/// surfaces in the [`ResumeContext`].
const RESUME_ACTIVITY_LIMIT: usize = 10;

/// The coding workflow, bound to one org's store.
#[derive(Debug, Clone)]
pub struct CodingWorkflow {
    store: WorkflowStore,
}

impl CodingWorkflow {
    #[must_use]
    pub fn new(store: WorkflowStore) -> Self {
        Self { store }
    }

    #[must_use]
    pub fn kind(&self) -> WorkflowKind {
        WorkflowKind::Coding
    }

    #[must_use]
    pub fn store(&self) -> &WorkflowStore {
        &self.store
    }

    /// Open a session on `task_id` for `agent`, entering
    /// [`CodingState::Branched`]. Fails with
    /// [`WorkflowError::AlreadyClaimed`] if the task already has an
    /// active session under a different actor.
    pub fn start(&self, task_id: Uuid, agent: AgentRef) -> Result<WorkSession, WorkflowError> {
        if let Some(existing) = self.active_session_for_task(task_id)? {
            if existing.current_actor != agent {
                return Err(WorkflowError::AlreadyClaimed {
                    actor: existing.current_actor.short_label(),
                });
            }
            return Ok(existing); // idempotent re-start by the same actor
        }
        let session = WorkSession::start(
            WorkflowKind::Coding,
            SubjectRef::Task { id: task_id },
            agent.clone(),
        );
        self.store.put_session(&session)?;
        // Seed the state machine: <birth> → branched.
        let t = Transition::record(session.id, "", CodingState::Branched.name(), agent);
        self.store.push_transition(&t)?;
        Ok(session)
    }

    /// Move `session` to state `to`, validating the edge against the
    /// current state and recording a [`Transition`].
    pub fn transition(
        &self,
        session: Uuid,
        to: CodingState,
        actor: AgentRef,
    ) -> Result<Transition, WorkflowError> {
        let mut s = self.store.session(session)?;
        let from = self.current_state(session)?;
        if let Some(from) = from {
            if !from.can_transition_to(to) {
                return Err(WorkflowError::InvalidTransition {
                    from: from.name().to_owned(),
                    to: to.name().to_owned(),
                });
            }
        }
        let from_name = from.map_or("", CodingState::name);
        let t = Transition::record(session, from_name, to.name(), actor);
        self.store.push_transition(&t)?;
        s.updated_at = chrono::Utc::now();
        self.store.put_session(&s)?;
        Ok(t)
    }

    /// Append an [`Activity`] without changing state.
    pub fn record<P: serde::Serialize>(
        &self,
        session: Uuid,
        kind: ActivityKind,
        actor: AgentRef,
        payload: &P,
    ) -> Result<Activity, WorkflowError> {
        // Touch the session so listings sort by recency, and to
        // surface "session not found" before writing an orphan row.
        let mut s = self.store.session(session)?;
        let a = Activity::record(session, kind, actor, payload);
        self.store.push_activity(&a)?;
        s.updated_at = chrono::Utc::now();
        self.store.put_session(&s)?;
        Ok(a)
    }

    /// Park `session`: mark it [`SessionStatus::Parked`], cancel any
    /// prior open handoff, and post a fresh one. The TaskInfo claim
    /// is released by the caller (CLI) — this layer owns sessions,
    /// not the task record.
    pub fn park(
        &self,
        session: Uuid,
        from: AgentRef,
        reason: HandoffReason,
        summary: impl Into<String>,
        open_questions: impl Into<String>,
        recommended_next: impl Into<String>,
    ) -> Result<Handoff, WorkflowError> {
        let mut s = self.store.session(session)?;
        let mut handoffs = self.store.handoffs_for(session)?;
        for h in handoffs
            .iter_mut()
            .filter(|h| h.status == HandoffStatus::Open)
        {
            h.status = HandoffStatus::Cancelled;
            h.resolved_at = Some(chrono::Utc::now());
        }
        let mut handoff = Handoff::post(session, from.clone(), reason, summary);
        handoff.open_questions = open_questions.into();
        handoff.recommended_next = recommended_next.into();
        handoffs.push(handoff.clone());
        self.store.save_handoffs_for(session, &handoffs)?;

        // Mirror to the activity log so the trail stays dense.
        let a = Activity::record(
            session,
            ActivityKind::Handoff,
            from,
            &serde_json::json!({ "handoff": handoff.id }),
        );
        self.store.push_activity(&a)?;

        s.status = SessionStatus::Parked;
        s.updated_at = chrono::Utc::now();
        self.store.put_session(&s)?;
        Ok(handoff)
    }

    /// Claim `session` for `agent` and assemble the
    /// [`ResumeContext`]: the session (now owned by `agent`), its
    /// last state, recent activity, the open handoff (marked
    /// `Claimed`), and the carried-over scratchpad.
    pub fn resume(&self, session: Uuid, agent: AgentRef) -> Result<ResumeContext, WorkflowError> {
        let mut s = self.store.session(session)?;
        s.current_actor = agent;
        s.status = SessionStatus::Active;
        s.updated_at = chrono::Utc::now();
        self.store.put_session(&s)?;

        let last_state = self
            .current_state(session)?
            .map(|st| TransitionState::new(st.name()));

        let mut recent_activity = self.store.activities_for(session)?;
        recent_activity.truncate(RESUME_ACTIVITY_LIMIT);

        // Latest open handoff → mark Claimed, hand it to the resumer.
        let mut handoffs = self.store.handoffs_for(session)?;
        let open_handoff = {
            let idx = handoffs
                .iter()
                .enumerate()
                .filter(|(_, h)| h.status == HandoffStatus::Open)
                .max_by_key(|(_, h)| h.created_at)
                .map(|(i, _)| i);
            match idx {
                Some(i) => {
                    handoffs[i].status = HandoffStatus::Claimed;
                    let claimed = handoffs[i].clone();
                    self.store.save_handoffs_for(session, &handoffs)?;
                    Some(claimed)
                }
                None => None,
            }
        };

        Ok(ResumeContext {
            scratchpad: s.scratchpad.clone(),
            session: s,
            last_state,
            recent_activity,
            open_handoff,
            // Neighbour assembly (blockers / related pages / symbols)
            // needs the task + wiki clients; deferred until the CLI
            // wiring passes them in. Concrete-first.
            related: Vec::new(),
        })
    }

    /// Close `session` normally.
    pub fn finish(&self, session: Uuid, actor: AgentRef) -> Result<(), WorkflowError> {
        let mut s = self.store.session(session)?;
        s.status = SessionStatus::Finished;
        s.ended_at = Some(chrono::Utc::now());
        s.updated_at = chrono::Utc::now();
        self.store.put_session(&s)?;
        let a = Activity::record(
            session,
            ActivityKind::Note,
            actor,
            &serde_json::json!({ "event": "finished" }),
        );
        self.store.push_activity(&a)?;
        Ok(())
    }

    /// Cancel `session` — an explicit drop (distinct from
    /// [`finish`](Self::finish), which marks success). Used by
    /// `task agent goal clear` to abandon a standing goal without
    /// closing it as met.
    pub fn cancel(&self, session: Uuid, actor: AgentRef) -> Result<(), WorkflowError> {
        let mut s = self.store.session(session)?;
        s.status = SessionStatus::Cancelled;
        s.ended_at = Some(chrono::Utc::now());
        s.updated_at = chrono::Utc::now();
        self.store.put_session(&s)?;
        let a = Activity::record(
            session,
            ActivityKind::Note,
            actor,
            &serde_json::json!({ "event": "cancelled" }),
        );
        self.store.push_activity(&a)?;
        Ok(())
    }

    /// The active (or parked) session for a task, if one exists.
    pub fn active_session_for_task(
        &self,
        task_id: Uuid,
    ) -> Result<Option<WorkSession>, WorkflowError> {
        let want = SubjectRef::Task { id: task_id };
        Ok(self
            .store
            .sessions()?
            .into_iter()
            .filter(|s| {
                s.subject == want
                    && matches!(s.status, SessionStatus::Active | SessionStatus::Parked)
            })
            .max_by_key(|s| s.started_at))
    }

    /// The state a session is currently in, from its last
    /// transition. `None` before the first transition.
    pub fn current_state(&self, session: Uuid) -> Result<Option<CodingState>, WorkflowError> {
        Ok(self
            .store
            .transitions_for(session)?
            .last()
            .and_then(|t| CodingState::from_name(&t.to_state)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wf() -> (CodingWorkflow, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (
            CodingWorkflow::new(WorkflowStore::open(dir.path())),
            dir, // keep alive
        )
    }

    #[test]
    fn start_seeds_branched_state() {
        let (wf, _d) = wf();
        let task = Uuid::new_v4();
        let s = wf.start(task, AgentRef::agent("claude")).unwrap();
        assert_eq!(s.status, SessionStatus::Active);
        assert_eq!(wf.current_state(s.id).unwrap(), Some(CodingState::Branched));
    }

    #[test]
    fn start_is_idempotent_for_same_actor_but_rejects_others() {
        let (wf, _d) = wf();
        let task = Uuid::new_v4();
        let a = wf.start(task, AgentRef::agent("claude")).unwrap();
        let again = wf.start(task, AgentRef::agent("claude")).unwrap();
        assert_eq!(a.id, again.id, "same actor re-start returns same session");

        let err = wf.start(task, AgentRef::agent("codex")).unwrap_err();
        assert!(matches!(err, WorkflowError::AlreadyClaimed { .. }));
    }

    #[test]
    fn legal_transitions_advance_state() {
        let (wf, _d) = wf();
        let s = wf.start(Uuid::new_v4(), AgentRef::agent("claude")).unwrap();
        let actor = AgentRef::agent("claude");
        wf.transition(s.id, CodingState::Committed, actor.clone())
            .unwrap();
        wf.transition(s.id, CodingState::Pushed, actor.clone())
            .unwrap();
        wf.transition(s.id, CodingState::Merged, actor).unwrap();
        assert_eq!(wf.current_state(s.id).unwrap(), Some(CodingState::Merged));
    }

    #[test]
    fn illegal_transition_is_rejected() {
        let (wf, _d) = wf();
        let s = wf.start(Uuid::new_v4(), AgentRef::agent("claude")).unwrap();
        // branched → merged is not a legal edge.
        let err = wf
            .transition(s.id, CodingState::Merged, AgentRef::agent("claude"))
            .unwrap_err();
        assert!(matches!(err, WorkflowError::InvalidTransition { .. }));
    }

    #[test]
    fn park_then_resume_carries_context() {
        let (wf, _d) = wf();
        let claude = AgentRef::agent("claude");
        let s = wf.start(Uuid::new_v4(), claude.clone()).unwrap();
        wf.transition(s.id, CodingState::Committed, claude.clone())
            .unwrap();
        wf.record(
            s.id,
            ActivityKind::Commit,
            claude.clone(),
            &serde_json::json!({ "sha": "abc123" }),
        )
        .unwrap();
        wf.park(
            s.id,
            claude,
            HandoffReason::NeedsHumanInput,
            "wired the schema, blocked on header format",
            "- token or Bearer?",
            "- finish the handler once answered",
        )
        .unwrap();

        let parked = wf.store().session(s.id).unwrap();
        assert_eq!(parked.status, SessionStatus::Parked);

        let codex = AgentRef::agent("codex");
        let ctx = wf.resume(s.id, codex.clone()).unwrap();
        assert_eq!(ctx.session.current_actor, codex);
        assert_eq!(ctx.session.status, SessionStatus::Active);
        assert_eq!(ctx.last_state.as_ref().unwrap().as_str(), "committed");
        let h = ctx.open_handoff.expect("handoff surfaced");
        assert_eq!(h.status, HandoffStatus::Claimed);
        assert!(h.summary.contains("blocked on header"));
        // commit + the handoff mirror = 2 activities, newest first.
        assert_eq!(ctx.recent_activity.len(), 2);
        assert_eq!(ctx.recent_activity[0].kind, ActivityKind::Handoff);
    }

    #[test]
    fn resuming_supersedes_only_the_open_handoff() {
        let (wf, _d) = wf();
        let claude = AgentRef::agent("claude");
        let s = wf.start(Uuid::new_v4(), claude.clone()).unwrap();
        wf.park(
            s.id,
            claude.clone(),
            HandoffReason::EndOfChunk,
            "first",
            "",
            "",
        )
        .unwrap();
        // Second park cancels the first open handoff.
        wf.park(s.id, claude, HandoffReason::EndOfChunk, "second", "", "")
            .unwrap();
        let opens = wf
            .store()
            .handoffs()
            .unwrap()
            .into_iter()
            .filter(|h| h.status == HandoffStatus::Open)
            .count();
        assert_eq!(opens, 1, "only the latest handoff stays open");
    }

    #[test]
    fn finish_closes_the_session() {
        let (wf, _d) = wf();
        let claude = AgentRef::agent("claude");
        let s = wf.start(Uuid::new_v4(), claude.clone()).unwrap();
        wf.finish(s.id, claude).unwrap();
        let done = wf.store().session(s.id).unwrap();
        assert_eq!(done.status, SessionStatus::Finished);
        assert!(done.ended_at.is_some());
        // No longer the active session for the task.
        assert!(
            wf.active_session_for_task(match done.subject {
                SubjectRef::Task { id } => id,
                _ => unreachable!(),
            })
            .unwrap()
            .is_none()
        );
    }
}
