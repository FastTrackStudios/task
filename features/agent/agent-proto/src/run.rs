//! `Run` — one attempt at one ticket by one runner.
//!
//! Runs **nest**: a workstream manager is a run whose children are
//! its tickets' attempts. That is what makes the supervisor as
//! observable and as killable as anything else it spawns.
//!
//! A ticket attempted three times has three runs, not one row that
//! forgets. "This has died three times on the same verify command"
//! is the single most useful thing this system can tell you, and it
//! is unanswerable without keeping every attempt.
//!
//! # Paths are observations, not configuration
//!
//! `worktree_path` and `session_path` are recorded *after the fact*,
//! by the runner that made them. The server never hands a runner a
//! path — doing so would make runners non-portable and put the
//! server back in the business of reaching into machines. It stores
//! them so that "which worktrees can I reclaim?" has an answer.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Where a run is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub enum RunStatus {
    /// Claimed and working.
    InProgress,
    /// Finished, verify command exited zero.
    Passed,
    /// Finished, verify command exited non-zero.
    Failed,
    /// No heartbeat within the window. Might still be alive.
    Stale,
    /// The runner is gone, or the process exited abnormally.
    Dead,
    /// Terminal, but a worktree is still on disk.
    NeedsCleanup,
    /// Worktree removed. The session file is kept.
    Archived,
}

impl RunStatus {
    /// Has this run stopped doing work?
    ///
    /// `Stale` is deliberately *not* terminal: a runner mid-build can
    /// miss a heartbeat and come back, and declaring it finished
    /// would strand a live attempt.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Passed | Self::Failed | Self::Dead | Self::NeedsCleanup | Self::Archived
        )
    }

    /// Lowercase slug, for storage and display.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InProgress => "in-progress",
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Stale => "stale",
            Self::Dead => "dead",
            Self::NeedsCleanup => "needs-cleanup",
            Self::Archived => "archived",
        }
    }

    /// Parse a slug.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::InProgress,
            Self::Passed,
            Self::Failed,
            Self::Stale,
            Self::Dead,
            Self::NeedsCleanup,
            Self::Archived,
        ]
        .into_iter()
        .find(|v| v.as_str() == s.trim().to_ascii_lowercase())
    }
}

/// One attempt at one ticket.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct Run {
    pub id: Uuid,
    /// The ticket being attempted.
    pub ticket: Uuid,
    /// Which runner holds it.
    pub runner: String,
    /// Parent run, when this attempt was spawned by a workstream
    /// manager. `None` for a top-level run.
    pub parent: Option<Uuid>,
    /// Branch the work lands on.
    pub branch: String,
    /// Where the worktree is, as reported by the runner.
    pub worktree_path: String,
    /// Where the agent session file is, as reported by the runner.
    /// Empty when there is none to resume.
    pub session_path: String,
    pub status: RunStatus,
    /// Verify command exit code, once there is one.
    pub exit_code: Option<i32>,
    pub started_at: DateTime<Utc>,
    /// Last time the runner said this attempt was still going.
    pub heartbeat_at: Option<DateTime<Utc>>,
    pub finished_at: Option<DateTime<Utc>>,
}

/// What a runner reports when it starts an attempt.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct StartRun {
    pub ticket: Uuid,
    pub runner: String,
    pub parent: Option<Uuid>,
    pub branch: String,
    pub worktree_path: String,
    pub session_path: String,
}

/// What a runner reports when an attempt ends.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct FinishRun {
    pub run: Uuid,
    /// `true` when the verify command exited zero.
    pub passed: bool,
    pub exit_code: Option<i32>,
    /// `true` when the worktree is still on disk — which is what
    /// `needs-cleanup` means.
    pub worktree_kept: bool,
}

/// Filter for listing runs.
#[derive(Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct RunFilter {
    /// Only runs for this ticket.
    pub ticket: Option<Uuid>,
    /// Only runs on this runner.
    pub runner: String,
    /// Only children of this run.
    pub parent: Option<Uuid>,
    /// Only runs in this state.
    pub status: Option<RunStatus>,
    /// Cap. `0` = no cap.
    pub limit: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statuses_round_trip_through_their_slugs() {
        for s in [
            RunStatus::InProgress,
            RunStatus::Passed,
            RunStatus::Failed,
            RunStatus::Stale,
            RunStatus::Dead,
            RunStatus::NeedsCleanup,
            RunStatus::Archived,
        ] {
            assert_eq!(RunStatus::parse(s.as_str()), Some(s));
        }
        assert_eq!(RunStatus::parse("nonsense"), None);
    }

    #[test]
    fn stale_is_not_terminal() {
        // A runner mid-build can miss a beat and come back. Calling
        // that finished would strand a live attempt.
        assert!(!RunStatus::Stale.is_terminal());
        assert!(!RunStatus::InProgress.is_terminal());
        for s in [
            RunStatus::Passed,
            RunStatus::Failed,
            RunStatus::Dead,
            RunStatus::NeedsCleanup,
            RunStatus::Archived,
        ] {
            assert!(s.is_terminal(), "{s:?} should be terminal");
        }
    }
}
