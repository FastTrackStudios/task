//! The [`Notification`] record + its typed pieces.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What kind of news a notification carries. One variant per notifier
/// rule (see `apps/task/server/src/notifier.rs` — the design goal is
/// that adding a rule is one variant + a few lines). The UI keys its
/// icon/color off this; the webhook channel serializes the kebab-case
/// name via [`NotifyKind::as_str`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum NotifyKind {
    /// A task's status transitioned into a terminal state.
    TaskCompleted = 0,
    /// A task was claimed / assigned (the actor is the new assignee).
    TaskAssigned = 1,
    /// An agent turn finished cleanly.
    AgentTurnFinished = 2,
    /// An agent turn errored.
    AgentTurnFailed = 3,
    /// A booking landed on the calendar.
    BookingCreated = 4,
    /// A booking was cancelled.
    BookingCancelled = 5,
    /// Forge issue activity (created / closed).
    ForgeIssue = 6,
    /// Forge pull-request activity (opened / reviewed).
    ForgePullRequest = 7,
    /// New mail landed in a watched mailbox.
    EmailReceived = 9,
    /// Anything else — kept so old clients render unknown future
    /// rules as a generic row instead of failing to decode… which
    /// they cannot do across a facet enum, so this is really the
    /// "rule without a dedicated variant yet" bucket.
    Other = 8,
}

impl NotifyKind {
    /// Stable kebab-case name — the persisted DB value and the
    /// webhook payload's `kind` field.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TaskCompleted => "task-completed",
            Self::TaskAssigned => "task-assigned",
            Self::AgentTurnFinished => "agent-turn-finished",
            Self::AgentTurnFailed => "agent-turn-failed",
            Self::BookingCreated => "booking-created",
            Self::BookingCancelled => "booking-cancelled",
            Self::ForgeIssue => "forge-issue",
            Self::ForgePullRequest => "forge-pull-request",
            Self::EmailReceived => "email-received",
            Self::Other => "other",
        }
    }

    /// Inverse of [`Self::as_str`]; unknown strings (a row written by
    /// a newer build) land in [`Self::Other`] instead of erroring.
    #[must_use]
    pub fn parse(s: &str) -> Self {
        match s {
            "task-completed" => Self::TaskCompleted,
            "task-assigned" => Self::TaskAssigned,
            "agent-turn-finished" => Self::AgentTurnFinished,
            "agent-turn-failed" => Self::AgentTurnFailed,
            "booking-created" => Self::BookingCreated,
            "booking-cancelled" => Self::BookingCancelled,
            "forge-issue" => Self::ForgeIssue,
            "forge-pull-request" => Self::ForgePullRequest,
            "email-received" => Self::EmailReceived,
            _ => Self::Other,
        }
    }
}

/// Where a notification came from — enough for the UI to navigate to
/// the thing that changed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct NotifySource {
    /// Producing service, e.g. `"task"`, `"agent"`, `"scheduling"`,
    /// `"forge"`. Diagnostic + grouping; not an enum so new producers
    /// don't need a proto rev.
    pub service: String,
    /// The changed entity's id in its own service's terms (task UUID,
    /// agent session id, booking id, `repo#123`).
    pub entity: String,
    /// App route to open on click (`/tasks`, `/vault?path=…`,
    /// `/agents?session=…`, `/bookings`, `/repos`). Empty = nowhere
    /// to go.
    pub href: String,
}

/// One notification row.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct Notification {
    /// Stable identity (server-minted v4).
    pub id: Uuid,
    pub kind: NotifyKind,
    /// One-line headline ("Task done: ship the notifier").
    pub title: String,
    /// Optional detail line. Empty = headline only.
    pub body: String,
    pub source: NotifySource,
    /// Who caused it, when the event carried an actor (agent label,
    /// booking attendee). Empty = unknown. The notifier uses it to
    /// avoid notifying an actor about their own action; the UI shows
    /// it as the byline.
    pub actor: String,
    pub created_at: DateTime<Utc>,
    /// `Some` once seen — the unread badge counts `None`s.
    pub read_at: Option<DateTime<Utc>>,
}

/// Client-side optimistic cache identity (`architect::Store`).
#[cfg(feature = "atom")]
impl architect::StoreEntity for Notification {
    type Key = Uuid;
    fn key(&self) -> Uuid {
        self.id
    }
}

/// Args for [`crate::Notify::list`]. Newest first, windowed.
#[derive(Debug, Clone, PartialEq, Eq, Default, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct NotifyListFilter {
    /// Only rows with `read_at = None`.
    pub unread_only: bool,
    /// Page size; `None` = the server default (100).
    pub limit: Option<u32>,
    /// Rows to skip (newest-first order).
    pub offset: Option<u32>,
}

impl NotifyListFilter {
    /// The bell's fetch: unread + recent, one default page.
    #[must_use]
    pub fn recent() -> Self {
        Self::default()
    }
}
