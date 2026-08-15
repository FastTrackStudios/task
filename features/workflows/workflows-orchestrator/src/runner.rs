//! The autonomous loop runner.
//!
//! `task run`'s engine, inspired by subsy/ralph-tui's
//! select→prompt→execute→detect→repeat cycle. This module owns the
//! *inner* loop: driving one [`WorkSession`] to completion by calling
//! an executor repeatedly and logging every pass to the audit trail.
//! The *outer* loop (pick the next ready subtask, claim it atomically,
//! open a session) is I/O against the task server and lives in the
//! CLI — this layer stays sync + unit-testable.
//!
//! The executor is a plain closure, not a trait: there is one real
//! executor today (a subprocess running the agent CLI). The trait /
//! registry is deferred until a second executor is actually wired —
//! same concrete-first rule as the rest of this crate.

use serde_json::json;
use uuid::Uuid;
use workflows_proto::{ActivityKind, AgentRef, HandoffReason, WorkflowError};

use crate::coding::CodingWorkflow;

/// What one executor pass reports back to the loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IterationOutcome {
    /// More work remains; run another iteration.
    Continue,
    /// The subtask is complete — finish the session.
    Done,
    /// Work can't proceed without intervention; park with a handoff.
    Blocked {
        /// Short tag for the [`HandoffReason::Custom`].
        reason: String,
        /// Markdown "where I left off" for the handoff summary.
        summary: String,
    },
}

/// How a [`run_session`](CodingWorkflow::run_session) ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunEnd {
    /// Executor reported `Done`; session finished.
    Completed,
    /// Executor reported `Blocked`; session parked with a handoff.
    Parked { reason: String },
    /// Hit `max_iters` without completing; session parked as
    /// [`HandoffReason::EndOfChunk`] so it can be resumed.
    MaxedOut,
}

/// Outcome of driving one session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRun {
    /// Number of executor iterations performed.
    pub iterations: u32,
    pub end: RunEnd,
}

impl CodingWorkflow {
    /// Drive a single `session` to a terminal outcome, invoking
    /// `exec` once per iteration (the agent does its work inside
    /// `exec`). Each pass is recorded as an [`ActivityKind::ToolCall`]
    /// so a crashed run is fully reconstructable from the audit log.
    ///
    /// `max_iters` is a hard ceiling; `0` means "no iterations" and
    /// parks immediately as [`RunEnd::MaxedOut`]. On `Done` the
    /// session is [`finish`](CodingWorkflow::finish)ed; on `Blocked`
    /// or ceiling it is [`park`](CodingWorkflow::park)ed so another
    /// pass (or agent) can resume it.
    pub fn run_session<F>(
        &self,
        session: Uuid,
        agent: AgentRef,
        max_iters: u32,
        mut exec: F,
    ) -> Result<SessionRun, WorkflowError>
    where
        F: FnMut(u32) -> Result<IterationOutcome, WorkflowError>,
    {
        for i in 0..max_iters {
            // Record the attempt *before* running it, so a crash mid-
            // iteration still leaves a breadcrumb.
            self.record(
                session,
                ActivityKind::ToolCall,
                agent.clone(),
                &json!({ "event": "iteration", "n": i }),
            )?;

            match exec(i)? {
                IterationOutcome::Continue => {}
                IterationOutcome::Done => {
                    self.finish(session, agent)?;
                    return Ok(SessionRun {
                        iterations: i + 1,
                        end: RunEnd::Completed,
                    });
                }
                IterationOutcome::Blocked { reason, summary } => {
                    self.park(
                        session,
                        agent,
                        HandoffReason::Custom {
                            tag: reason.clone(),
                        },
                        summary,
                        String::new(),
                        String::new(),
                    )?;
                    return Ok(SessionRun {
                        iterations: i + 1,
                        end: RunEnd::Parked { reason },
                    });
                }
            }
        }

        // Ran out of budget without finishing — park so it's resumable.
        self.park(
            session,
            agent,
            HandoffReason::EndOfChunk,
            format!("hit the {max_iters}-iteration ceiling without completing"),
            String::new(),
            "- resume and continue; consider raising --max-iters".to_owned(),
        )?;
        Ok(SessionRun {
            iterations: max_iters,
            end: RunEnd::MaxedOut,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::WorkflowStore;
    use std::cell::Cell;
    use workflows_proto::SessionStatus;

    fn wf() -> (CodingWorkflow, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (CodingWorkflow::new(WorkflowStore::open(dir.path())), dir)
    }

    #[test]
    fn completes_after_n_continues() {
        let (wf, _d) = wf();
        let agent = AgentRef::agent("claude");
        let s = wf.start(Uuid::new_v4(), agent.clone()).unwrap();
        // Continue twice, then Done.
        let run = wf
            .run_session(s.id, agent, 10, |i| {
                Ok(if i < 2 {
                    IterationOutcome::Continue
                } else {
                    IterationOutcome::Done
                })
            })
            .unwrap();
        assert_eq!(run.end, RunEnd::Completed);
        assert_eq!(run.iterations, 3);
        assert_eq!(
            wf.store().session(s.id).unwrap().status,
            SessionStatus::Finished
        );
        // 3 iteration breadcrumbs + the finish note.
        let acts = wf.store().activities_for(s.id).unwrap();
        let iters = acts
            .iter()
            .filter(|a| a.kind == ActivityKind::ToolCall)
            .count();
        assert_eq!(iters, 3);
    }

    #[test]
    fn blocked_parks_with_reason() {
        let (wf, _d) = wf();
        let agent = AgentRef::agent("claude");
        let s = wf.start(Uuid::new_v4(), agent.clone()).unwrap();
        let run = wf
            .run_session(s.id, agent, 10, |_| {
                Ok(IterationOutcome::Blocked {
                    reason: "needs-creds".into(),
                    summary: "stuck: missing API token".into(),
                })
            })
            .unwrap();
        assert_eq!(
            run.end,
            RunEnd::Parked {
                reason: "needs-creds".into()
            }
        );
        let parked = wf.store().session(s.id).unwrap();
        assert_eq!(parked.status, SessionStatus::Parked);
        let h = wf
            .store()
            .handoffs()
            .unwrap()
            .into_iter()
            .find(|h| h.session_id == s.id)
            .unwrap();
        assert!(h.summary.contains("missing API token"));
    }

    #[test]
    fn ceiling_parks_as_end_of_chunk() {
        let (wf, _d) = wf();
        let agent = AgentRef::agent("claude");
        let s = wf.start(Uuid::new_v4(), agent.clone()).unwrap();
        let calls = Cell::new(0u32);
        let run = wf
            .run_session(s.id, agent, 3, |_| {
                calls.set(calls.get() + 1);
                Ok(IterationOutcome::Continue)
            })
            .unwrap();
        assert_eq!(run.end, RunEnd::MaxedOut);
        assert_eq!(run.iterations, 3);
        assert_eq!(calls.get(), 3, "executor called exactly max_iters times");
        assert_eq!(
            wf.store().session(s.id).unwrap().status,
            SessionStatus::Parked
        );
    }

    #[test]
    fn zero_iters_parks_immediately() {
        let (wf, _d) = wf();
        let agent = AgentRef::agent("claude");
        let s = wf.start(Uuid::new_v4(), agent.clone()).unwrap();
        let run = wf
            .run_session(s.id, agent, 0, |_| Ok(IterationOutcome::Done))
            .unwrap();
        assert_eq!(run.end, RunEnd::MaxedOut);
        assert_eq!(run.iterations, 0);
    }

    #[test]
    fn executor_error_propagates() {
        let (wf, _d) = wf();
        let agent = AgentRef::agent("claude");
        let s = wf.start(Uuid::new_v4(), agent.clone()).unwrap();
        let err = wf
            .run_session(s.id, agent, 5, |_| {
                Err(WorkflowError::Backend("boom".into()))
            })
            .unwrap_err();
        assert!(matches!(err, WorkflowError::Backend(_)));
    }
}
