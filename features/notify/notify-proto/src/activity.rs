//! The activity feed — one record of *everything that happened* in an
//! org, newest first, scrollable back to the beginning.
//!
//! Where a [`crate::Notification`] is a queue item ("this needs you",
//! read/unread, deletable), an [`Activity`] is history: append-only,
//! never read-flipped, never deleted by a client. The server's notifier
//! writes both from the same event hubs (`apps/server/src/notifier.rs`)
//! — the bell shows the queue, `/activity` shows the history, a task's
//! page shows its own slice of it (`ActivityFilter::entity_id`), a
//! project's page its slice (`ActivityFilter::project_id`, which takes
//! in the project's tasks too).
//!
//! Spec: `docs/spec/activity.md`.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::NotifyError;

/// What happened. One variant per sentence the feed can say; the UI
/// keys its verb and icon off this, the filter sheet its groups
/// ([`ActivityKind::group`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum ActivityKind {
    /// A thing came into being (a task, a milestone).
    Created = 0,
    /// A task's status moved between two non-terminal / two terminal
    /// states (`from` → `to`).
    StatusChanged = 1,
    /// Someone was given the task (`to` = their name).
    Assigned = 2,
    /// The task lost its owner (`from` = who had it).
    Unassigned = 3,
    PriorityChanged = 4,
    DueChanged = 5,
    /// The title changed (`from` → `to`).
    Renamed = 6,
    /// The body/description was edited. Coalesced: one entry per
    /// editing burst, not per keystroke.
    Edited = 7,
    /// Status moved into a terminal state.
    Completed = 8,
    /// Status moved out of a terminal state.
    Reopened = 9,
    Deleted = 10,
    /// A comment was posted (`body` = the comment).
    Commented = 11,
    /// A thought landed in the inbox.
    Captured = 12,
    /// A capture was decided: `to` names what it became, `href` opens
    /// that.
    Processed = 13,
    TimerStarted = 14,
    TimerStopped = 15,
    /// A timer session naming a steering kind ("Maintenance hour")
    /// started — a *mode*.
    ModeStarted = 16,
    ModeEnded = 17,
    ProjectCreated = 18,
    /// A project's title or status changed (`field` says which).
    ProjectUpdated = 19,
    MilestoneCreated = 20,
    MilestoneUpdated = 21,
    /// Written by a newer build — rendered as a plain row.
    Other = 255,
}

/// The filter sheet's groups — what a person thinks in ("comments",
/// "time") rather than the verb list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum ActivityGroup {
    Tasks = 0,
    Comments = 1,
    Inbox = 2,
    Time = 3,
    Projects = 4,
}

impl ActivityGroup {
    pub const ALL: [Self; 5] = [
        Self::Tasks,
        Self::Comments,
        Self::Inbox,
        Self::Time,
        Self::Projects,
    ];

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tasks => "Tasks",
            Self::Comments => "Comments",
            Self::Inbox => "Inbox",
            Self::Time => "Time & modes",
            Self::Projects => "Projects",
        }
    }

    /// Every kind in this group — what the client sends as
    /// [`ActivityFilter::kinds`] when the group is ticked.
    #[must_use]
    pub fn kinds(self) -> Vec<ActivityKind> {
        ActivityKind::ALL
            .iter()
            .copied()
            .filter(|k| k.group() == self)
            .collect()
    }
}

impl ActivityKind {
    pub const ALL: [Self; 22] = [
        Self::Created,
        Self::StatusChanged,
        Self::Assigned,
        Self::Unassigned,
        Self::PriorityChanged,
        Self::DueChanged,
        Self::Renamed,
        Self::Edited,
        Self::Completed,
        Self::Reopened,
        Self::Deleted,
        Self::Commented,
        Self::Captured,
        Self::Processed,
        Self::TimerStarted,
        Self::TimerStopped,
        Self::ModeStarted,
        Self::ModeEnded,
        Self::ProjectCreated,
        Self::ProjectUpdated,
        Self::MilestoneCreated,
        Self::MilestoneUpdated,
    ];

    /// Stable snake_case name — the persisted DB value.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::StatusChanged => "status_changed",
            Self::Assigned => "assigned",
            Self::Unassigned => "unassigned",
            Self::PriorityChanged => "priority_changed",
            Self::DueChanged => "due_changed",
            Self::Renamed => "renamed",
            Self::Edited => "edited",
            Self::Completed => "completed",
            Self::Reopened => "reopened",
            Self::Deleted => "deleted",
            Self::Commented => "commented",
            Self::Captured => "captured",
            Self::Processed => "processed",
            Self::TimerStarted => "timer_started",
            Self::TimerStopped => "timer_stopped",
            Self::ModeStarted => "mode_started",
            Self::ModeEnded => "mode_ended",
            Self::ProjectCreated => "project_created",
            Self::ProjectUpdated => "project_updated",
            Self::MilestoneCreated => "milestone_created",
            Self::MilestoneUpdated => "milestone_updated",
            Self::Other => "other",
        }
    }

    /// Inverse of [`Self::as_str`]; unknown names land in
    /// [`Self::Other`].
    #[must_use]
    pub fn parse(s: &str) -> Self {
        Self::ALL
            .iter()
            .copied()
            .find(|k| k.as_str() == s)
            .unwrap_or(Self::Other)
    }

    #[must_use]
    pub const fn group(self) -> ActivityGroup {
        match self {
            Self::Commented => ActivityGroup::Comments,
            Self::Captured | Self::Processed => ActivityGroup::Inbox,
            Self::TimerStarted | Self::TimerStopped | Self::ModeStarted | Self::ModeEnded => {
                ActivityGroup::Time
            }
            Self::ProjectCreated
            | Self::ProjectUpdated
            | Self::MilestoneCreated
            | Self::MilestoneUpdated => ActivityGroup::Projects,
            _ => ActivityGroup::Tasks,
        }
    }
}

/// One thing that happened.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct Activity {
    /// Server-minted v4 (seeded rows carry fixed ids so a replant is
    /// idempotent).
    pub id: Uuid,
    pub at: DateTime<Utc>,
    /// Who did it: an auth user id when the server knew the caller, an
    /// agent label (`triage-bot`), or empty when nobody could be named.
    pub actor_id: String,
    /// What to call them — a display name, an agent label, or
    /// `"Someone"`. Never empty.
    pub actor_name: String,
    pub kind: ActivityKind,
    /// `"task"`, `"project"`, `"inbox"`, `"timer"`, `"milestone"`,
    /// `"note"`.
    pub entity_type: String,
    /// The entity's id in its own service's terms (UUID for tasks,
    /// projects and timer sessions; the capture id for inbox items).
    pub entity_id: String,
    /// Its title at the time — the feed reads "Alice completed *Comp
    /// lead vocals*" even after the task is renamed or gone.
    pub entity_title: String,
    /// App route to open on tap (`/tasks/<id>`, `/projects/<id>`,
    /// `/timer`). Empty = nowhere to go.
    pub href: String,
    /// The project the entity belongs to (a task's project; a
    /// project's own id), so a project's feed takes in its tasks.
    pub project_id: Option<Uuid>,
    /// Which attribute changed, for change kinds (`"status"`,
    /// `"priority"`, `"due"`, `"title"`, `"assignee"`). Empty otherwise.
    pub field: String,
    /// Before / after, for change kinds. Empty when not applicable.
    pub from: String,
    pub to: String,
    /// Free text: the comment, a session's duration, what a capture
    /// became. Empty = none.
    pub body: String,
}

/// Client-side optimistic cache identity (`architect::Store`).
#[cfg(feature = "atom")]
impl architect::StoreEntity for Activity {
    type Key = Uuid;
    fn key(&self) -> Uuid {
        self.id
    }
}

/// Where a page of the feed starts: strictly older than this entry.
/// Pass the oldest entry of the page you hold to fetch the next.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct ActivityCursor {
    pub at: DateTime<Utc>,
    pub id: Uuid,
}

impl ActivityCursor {
    /// The cursor that continues after `a` (exclusive).
    #[must_use]
    pub fn after(a: &Activity) -> Self {
        Self { at: a.at, id: a.id }
    }
}

/// Args for [`ActivityFeed::list`]. Every set field narrows (AND);
/// the default is the whole org's feed, newest first, one page.
#[derive(Debug, Clone, PartialEq, Default, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct ActivityFilter {
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    /// Entries whose `project_id` is this — the project itself and
    /// everything in it.
    pub project_id: Option<Uuid>,
    /// Only these kinds. Empty = every kind.
    pub kinds: Vec<ActivityKind>,
    /// Only this actor (`Activity::actor_id`) — "only mine".
    pub actor_id: Option<String>,
    /// Scroll-back: only entries strictly older than this.
    pub before: Option<ActivityCursor>,
    /// Page size; `None` = the server default (50), capped at 500.
    pub limit: Option<u32>,
}

impl ActivityFilter {
    /// One entity's history.
    #[must_use]
    pub fn entity(entity_type: &str, entity_id: impl ToString) -> Self {
        Self {
            entity_type: Some(entity_type.to_owned()),
            entity_id: Some(entity_id.to_string()),
            limit: Some(200),
            ..Self::default()
        }
    }

    /// One project's history, its tasks' included.
    #[must_use]
    pub fn project(project_id: Uuid) -> Self {
        Self {
            project_id: Some(project_id),
            limit: Some(200),
            ..Self::default()
        }
    }

    /// Does `a` pass this filter? The server's query and the client's
    /// live fold agree through this one predicate (the cursor and the
    /// page size aside — those are about windows, not membership).
    #[must_use]
    pub fn matches(&self, a: &Activity) -> bool {
        self.entity_type
            .as_ref()
            .is_none_or(|t| *t == a.entity_type)
            && self.entity_id.as_ref().is_none_or(|i| *i == a.entity_id)
            && self.project_id.is_none_or(|p| a.project_id == Some(p))
            && (self.kinds.is_empty() || self.kinds.contains(&a.kind))
            && self.actor_id.as_ref().is_none_or(|x| *x == a.actor_id)
    }
}

/// One activity change, broadcast to every subscriber.
///
/// Fetch a page via [`ActivityFeed::list`] (after subscribing), then
/// fold: [`ActivityEvent::Recorded`] inserts (idempotent by `id`).
/// Entries are never edited, so there is no update variant;
/// [`ActivityEvent::Removed`] exists for operator pruning.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
#[allow(clippy::large_enum_variant)]
pub enum ActivityEvent {
    Recorded(Activity),
    Removed(Uuid),
}

#[architect::rpc]
pub trait ActivityFeed {
    /// Entries matching `filter`, newest first (`at` desc, `id` desc
    /// as the tie-break), at most `filter.limit`.
    async fn list(&self, filter: ActivityFilter) -> Result<Vec<Activity>, NotifyError>;

    /// Every new entry as it is recorded. See [`ActivityEvent`].
    #[subscribe]
    fn events(&self) -> ActivityEvent;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_names_round_trip_and_unknown_is_other() {
        for k in ActivityKind::ALL {
            assert_eq!(ActivityKind::parse(k.as_str()), k);
        }
        assert_eq!(ActivityKind::parse("from_the_future"), ActivityKind::Other);
    }

    #[test]
    fn every_kind_has_one_group_and_groups_cover_them() {
        let covered: usize = ActivityGroup::ALL.iter().map(|g| g.kinds().len()).sum();
        assert_eq!(covered, ActivityKind::ALL.len());
        assert_eq!(ActivityKind::Commented.group(), ActivityGroup::Comments);
        assert_eq!(ActivityKind::ModeStarted.group(), ActivityGroup::Time);
    }
}
