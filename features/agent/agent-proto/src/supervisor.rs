//! What to do about a run that has stopped making progress.
//!
//! The supervisor's **only** recovery power is restart. It does not
//! answer the agent's questions, and it does not decide the work is
//! done — an earlier design let it answer anything it could cite a
//! source for, and that was reverted: a human-in-the-loop question
//! resolves only through the human, and "can I cite this?" is a
//! judgement an agent talks itself past at three in the morning.
//!
//! So the whole policy is: restart a stuck run, a bounded number of
//! times, then hand it to a person.

use std::time::Duration;

use chrono::{DateTime, Utc};

use crate::run::{Run, RunStatus};

/// How long a run may make no progress before it is restarted.
pub const NO_PROGRESS_AFTER: Duration = Duration::from_secs(900);

/// How many times a ticket may be restarted before a human is asked.
pub const MAX_RESTARTS: u32 = 3;

/// What the supervisor should do about one run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Recovery {
    /// Leave it alone.
    Leave,
    /// Kill it and let the ticket be taken again.
    Restart,
    /// Out of restarts. Block the ticket on a human.
    Escalate,
}

/// Decide what to do about `run`, given how many attempts this ticket
/// has already had.
///
/// `attempts` counts every run for the ticket, this one included, so
/// the first attempt is `1`.
///
/// Only runs that are actually working are candidates: a finished run
/// is not stuck, and a run whose heartbeat is fresh is not stuck
/// either. Progress means a heartbeat — the runner says so while it
/// works, and a long build is exactly the case that must not be
/// mistaken for death.
#[must_use]
pub fn decide(
    run: &Run,
    now: DateTime<Utc>,
    no_progress_after: Duration,
    attempts: u32,
    max_restarts: u32,
) -> Recovery {
    if run.status.is_terminal() {
        return Recovery::Leave;
    }
    if !is_stuck(run, now, no_progress_after) {
        return Recovery::Leave;
    }
    // `attempts` includes the current one, so the number of restarts
    // already spent is one less.
    if attempts.saturating_sub(1) >= max_restarts {
        Recovery::Escalate
    } else {
        Recovery::Restart
    }
}

/// Has this run gone quiet for longer than the window?
///
/// A run that has never heartbeated is judged from when it started,
/// so a runner that dies immediately after claiming is still caught.
#[must_use]
pub fn is_stuck(run: &Run, now: DateTime<Utc>, no_progress_after: Duration) -> bool {
    let last = run.heartbeat_at.unwrap_or(run.started_at);
    match now.signed_duration_since(last).to_std() {
        Ok(quiet) => quiet > no_progress_after,
        // A timestamp in the future (clock skew) is not silence.
        Err(_) => false,
    }
}

/// The message a human sees when a ticket runs out of restarts.
#[must_use]
pub fn escalation_text(attempts: u32) -> String {
    format!(
        "This ticket has been restarted {attempts} times without finishing. \
         Something about it is not working — the agent cannot tell what, \
         so it is over to you."
    )
}

/// Is this run one the supervisor should even look at?
#[must_use]
pub fn is_supervisable(status: RunStatus) -> bool {
    matches!(status, RunStatus::InProgress | RunStatus::Stale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;
    use uuid::Uuid;

    fn run(status: RunStatus, quiet_secs: i64) -> Run {
        let now = Utc::now();
        Run {
            id: Uuid::new_v4(),
            ticket: Uuid::new_v4(),
            runner: "r".into(),
            parent: None,
            branch: "agent/x".into(),
            worktree_path: "/tmp/wt/x".into(),
            session_path: String::new(),
            status,
            exit_code: None,
            started_at: now - ChronoDuration::seconds(quiet_secs),
            heartbeat_at: Some(now - ChronoDuration::seconds(quiet_secs)),
            finished_at: None,
        }
    }

    const WINDOW: Duration = Duration::from_secs(900);

    #[test]
    fn a_run_that_is_still_beating_is_left_alone() {
        let r = run(RunStatus::InProgress, 10);
        assert_eq!(decide(&r, Utc::now(), WINDOW, 1, 3), Recovery::Leave);
    }

    #[test]
    fn a_long_build_is_not_mistaken_for_death() {
        // Right up to the window, a quiet-but-beating run is fine.
        let r = run(RunStatus::InProgress, 899);
        assert!(!is_stuck(&r, Utc::now(), WINDOW));
        assert_eq!(decide(&r, Utc::now(), WINDOW, 1, 3), Recovery::Leave);
    }

    #[test]
    fn a_silent_run_past_the_window_is_restarted() {
        let r = run(RunStatus::InProgress, 1000);
        assert!(is_stuck(&r, Utc::now(), WINDOW));
        assert_eq!(decide(&r, Utc::now(), WINDOW, 1, 3), Recovery::Restart);
    }

    #[test]
    fn a_stale_run_is_supervisable_and_restartable() {
        // Stale is not terminal — it is exactly the state the
        // supervisor exists to act on.
        assert!(is_supervisable(RunStatus::Stale));
        let r = run(RunStatus::Stale, 1000);
        assert_eq!(decide(&r, Utc::now(), WINDOW, 2, 3), Recovery::Restart);
    }

    #[test]
    fn restarts_are_bounded_then_a_human_is_asked() {
        let r = run(RunStatus::InProgress, 1000);
        // attempts includes the current one: 1..=3 still restart,
        // the fourth escalates.
        for attempts in 1..=3 {
            assert_eq!(
                decide(&r, Utc::now(), WINDOW, attempts, 3),
                Recovery::Restart,
                "attempt {attempts} should still restart"
            );
        }
        assert_eq!(decide(&r, Utc::now(), WINDOW, 4, 3), Recovery::Escalate);
        assert_eq!(decide(&r, Utc::now(), WINDOW, 9, 3), Recovery::Escalate);
    }

    #[test]
    fn zero_restarts_escalates_immediately() {
        let r = run(RunStatus::InProgress, 1000);
        assert_eq!(decide(&r, Utc::now(), WINDOW, 1, 0), Recovery::Escalate);
    }

    #[test]
    fn a_finished_run_is_never_touched_however_old() {
        for s in [
            RunStatus::Passed,
            RunStatus::Failed,
            RunStatus::Dead,
            RunStatus::NeedsCleanup,
            RunStatus::Archived,
        ] {
            let r = run(s, 100_000);
            assert_eq!(
                decide(&r, Utc::now(), WINDOW, 9, 3),
                Recovery::Leave,
                "{s:?} must be left alone"
            );
            assert!(!is_supervisable(s));
        }
    }

    #[test]
    fn a_run_that_never_beat_is_judged_from_when_it_started() {
        // A runner that dies right after claiming still gets caught.
        let mut r = run(RunStatus::InProgress, 1000);
        r.heartbeat_at = None;
        assert!(is_stuck(&r, Utc::now(), WINDOW));
    }

    #[test]
    fn clock_skew_into_the_future_is_not_silence() {
        let mut r = run(RunStatus::InProgress, 0);
        r.heartbeat_at = Some(Utc::now() + ChronoDuration::hours(1));
        assert!(!is_stuck(&r, Utc::now(), WINDOW));
    }

    #[test]
    fn the_escalation_says_why_it_is_being_handed_over() {
        let text = escalation_text(4);
        assert!(text.contains('4'));
        assert!(text.contains("restarted"));
    }
}
