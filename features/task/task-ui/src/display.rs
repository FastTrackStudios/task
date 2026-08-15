//! Presentation helpers over the wire model.
//!
//! The components render `task_proto::TaskInfo` directly — there
//! is no UI-side mirror of the persistence shape any more. What
//! lives here is the part that genuinely *is* presentation and so
//! has no business on the wire type:
//!
//! - duration formatting ([`duration_label`] / [`clock_label`]) —
//!   these take a plain second count and never touch a task;
//! - the human labels + board column order for
//!   [`Status`] / [`Priority`] (UI copy, not wire values — the
//!   wire values are `as_str()`);
//! - [`TaskDisplay`], a handful of derived reads the board wants
//!   (typed status / priority, the parsed due date, the state
//!   group, the flattened `workflow.parent`).
//!
//! [`TaskDisplay::state_group`] deliberately stays here rather
//! than on the model: it resolves against
//! [`project_proto::default_states`], and the *correct* registry
//! is the owning project's. These components are project-agnostic
//! (props-driven), so they take the default and callers that know
//! the project resolve upstream with
//! [`project_proto::resolve_state_group`].

use chrono::NaiveDate;
use project_proto::StateGroup;
use task_proto::{Priority, Status, TaskInfo};
use uuid::Uuid;

/// Default board column order — left to right in the kanban.
/// `Cancelled` has no column (cancelled work is hidden, not
/// parked).
pub const BOARD_ORDER: &[Status] = &[
    Status::Open,
    Status::InProgress,
    Status::Waiting,
    Status::Done,
];

/// Human label for a [`Status`] — UI copy. The *wire* value is
/// `Status::as_str()`; never persist a label.
pub trait StatusLabel {
    fn label(self) -> &'static str;
}

impl StatusLabel for Status {
    fn label(self) -> &'static str {
        match self {
            Self::Open => "Open",
            Self::InProgress => "In progress",
            Self::Waiting => "Waiting",
            Self::Done => "Done",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// Human label for a [`Priority`] — UI copy, as [`StatusLabel`].
pub trait PriorityLabel {
    fn label(self) -> &'static str;
}

impl PriorityLabel for Priority {
    fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Low => "Low",
            Self::Normal => "Normal",
            Self::High => "High",
            Self::Critical => "Critical",
        }
    }
}

/// Derived reads the board views want off a [`TaskInfo`].
pub trait TaskDisplay {
    /// Raw `status` as the canonical enum, falling back to
    /// [`Status::Open`] for custom names (the pill still shows the
    /// raw string — see `subtasks.rs`).
    fn status_enum(&self) -> Status;
    /// Raw `priority` as the canonical enum, defaulting to
    /// [`Priority::Normal`].
    fn priority_enum(&self) -> Priority;
    /// Parse `due` as a `NaiveDate`. Accepts plain `YYYY-MM-DD` or
    /// strips the time off a full ISO timestamp.
    fn due_date(&self) -> Option<NaiveDate>;
    /// Canonical state group for the raw status, resolved against
    /// the default registry — progress classification goes through
    /// this, not [`Self::status_enum`], so custom status names
    /// (`shipped`, `qa-review`) classify sensibly.
    fn state_group(&self) -> StateGroup;
    /// `true` once the status classifies into the `completed`
    /// group.
    fn is_done(&self) -> bool;
    /// Parent task (`workflow.parent`) — set when this row is a
    /// subtask.
    fn parent(&self) -> Option<Uuid>;
}

impl TaskDisplay for TaskInfo {
    fn status_enum(&self) -> Status {
        Status::from_str(&self.status).unwrap_or(Status::Open)
    }

    fn priority_enum(&self) -> Priority {
        Priority::from_str(&self.priority).unwrap_or(Priority::Normal)
    }

    fn due_date(&self) -> Option<NaiveDate> {
        let s = self.due.as_deref()?;
        s.get(..10).and_then(|d| d.parse::<NaiveDate>().ok())
    }

    fn state_group(&self) -> StateGroup {
        project_proto::resolve_state_group(None, &self.status)
    }

    fn is_done(&self) -> bool {
        self.state_group() == StateGroup::Completed
    }

    fn parent(&self) -> Option<Uuid> {
        self.workflow.as_ref().and_then(|w| w.parent)
    }
}

/// Compact duration label: `47s` under a minute, `12m` under an
/// hour, `1h05` beyond. Shared by the row chips and the Now bar.
#[must_use]
pub fn duration_label(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 60 {
        format!("{s}s")
    } else if s < 3600 {
        format!("{}m", s / 60)
    } else {
        format!("{}h{:02}", s / 3600, (s % 3600) / 60)
    }
}

/// Live-clock label with visible seconds: `4:32`, `1:04:32`. The
/// Now bar ticks once a second — the label must move with it.
#[must_use]
pub fn clock_label(seconds: i64) -> String {
    let s = seconds.max(0);
    if s < 3600 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
    }
}

#[cfg(test)]
mod duration_tests {
    use super::{clock_label, duration_label};

    #[test]
    fn duration_label_scales() {
        assert_eq!(duration_label(47), "47s");
        assert_eq!(duration_label(150), "2m");
        assert_eq!(duration_label(3900), "1h05");
    }

    #[test]
    fn clock_label_shows_seconds() {
        assert_eq!(clock_label(272), "4:32");
        assert_eq!(clock_label(3872), "1:04:32");
    }
}
