//! The one thing most worth doing now — the Suggested slot on the Tasks
//! page, and the watch's glance.
//!
//! One suggestion, never a list: a second card is a dashboard. [`suggest`]
//! walks a fixed order and stops at the first that applies:
//!
//! 1. **The calendar planned a task for now** (a block with a task
//!    assigned): start it.
//! 2. **A steering block is running** (getting ready, a maintenance hour,
//!    wind-down): start its mode — a timer session named for the kind,
//!    which is what makes its work show (`docs/spec/capture-and-process.md`
//!    § the hour suggests).
//! 3. **Work in progress**: pick up where you left off.
//! 4. **Overdue**: the oldest late task.
//! 5. **Captures to process**: drain the inbox.
//! 6. **Due today**.
//!
//! "Not now" sets a suggestion aside (by [`Suggestion::key`]) and the
//! next one shows. Pure, on wire types, with the plan passed in as a
//! [`NowHint`] — so every client (web, phone, the watch bridge on the
//! server) suggests the same thing at the same minute.

use uuid::Uuid;

use crate::model::TaskInfo;
use crate::relevance::{RelevanceContext, matches_focus};
use crate::status_is_open;

/// What the day plan says about now, already resolved by the caller
/// (`scheduling_proto::{steering_block_at, planned_task_at}`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NowHint {
    /// A block running now with a task assigned: `(block label, task id)`.
    pub planned: Option<(String, Uuid)>,
    /// The steering block running now, if one is.
    pub mode: Option<ModeHint>,
    /// When the block in question ends, as the client shows a clock
    /// ("5:00 PM").
    pub until: String,
}

/// What a mode is called while it runs ("Maintenance mode").
#[must_use]
pub const fn mode_name(category: scheduling_proto::BlockCategory) -> &'static str {
    use scheduling_proto::BlockCategory as C;
    match category {
        C::Maintenance => "Maintenance mode",
        C::Meal => "Meal time",
        C::Exercise => "Exercise",
        C::Spiritual => "Quiet time",
        C::Reset => "Morning reset",
        C::Hygiene => "Getting ready",
        C::WindDown => "Wind-down",
        C::Allocatable | C::Sleep | C::Other => "Focus",
    }
}

/// The steering kind's mode, as the suggestion needs it.
#[must_use]
pub fn mode_hint(label: &str, category: scheduling_proto::BlockCategory) -> Option<ModeHint> {
    Some(ModeHint {
        label: label.to_owned(),
        title: category.focus_label()?.to_owned(),
        name: mode_name(category).to_owned(),
        focus: category
            .surfaces()
            .iter()
            .map(|s| (*s).to_owned())
            .collect(),
    })
}

/// Read a day's blocks at `minute` (since local midnight) into the hint:
/// the planned task (`scheduling_proto::planned_task_at`), the steering
/// block's mode (`scheduling_proto::steering_block_at`), and when the
/// block the suggestion is about ends. `None` when nothing is planned now.
#[must_use]
pub fn now_hint(blocks: &[scheduling_proto::PlannedBlock], minute: u16) -> Option<NowHint> {
    let planned = scheduling_proto::planned_task_at(blocks, minute)
        .and_then(|(b, id)| Some((b, Uuid::parse_str(id).ok()?)));
    let steering = scheduling_proto::steering_block_at(blocks, minute);
    let ends = planned.map(|(b, _)| b).or(steering)?.end;
    let until =
        chrono::NaiveTime::from_hms_opt(u32::from(ends.hours()), u32::from(ends.minutes()), 0)
            .map(|t| t.format("%-I:%M %p").to_string())
            .unwrap_or_default();
    Some(NowHint {
        planned: planned.map(|(b, id)| (b.label.clone(), id)),
        mode: steering.and_then(|b| mode_hint(&b.label, b.category)),
        until,
    })
}

/// A steering block's mode, as the suggestion needs it.
#[derive(Clone, Debug, PartialEq)]
pub struct ModeHint {
    /// The block's own label ("Errands & chores") — what "Not now" keys on.
    pub label: String,
    /// The kind's focus label ("Maintenance hour") — the session's
    /// description when started.
    pub title: String,
    /// What it is called while it runs ("Maintenance mode").
    pub name: String,
    /// Context / tag names it brings forward.
    pub focus: Vec<String>,
}

/// What the slot suggests.
#[derive(Clone, Debug, PartialEq)]
pub enum Suggestion {
    /// Start the task the calendar planned for now.
    StartPlanned {
        task: TaskInfo,
        block: String,
        until: String,
    },
    /// Start the running block's mode; `waiting` of yours match it.
    StartMode {
        mode: ModeHint,
        until: String,
        waiting: usize,
    },
    /// Carry on with a task already moving.
    Continue(TaskInfo),
    /// The oldest late task.
    Overdue(TaskInfo),
    /// Decide what the inbox's captures are.
    Process(usize),
    /// Something due today.
    DueToday(TaskInfo),
}

/// The words every client shows for a suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Card {
    /// Why now, small, above the title ("Maintenance hour · until 5:00 PM").
    pub eyebrow: String,
    pub title: String,
    /// A quiet line under the title ("3 tasks waiting"); may be empty.
    pub detail: String,
    /// Eyebrow and detail in one line, for one-line surfaces (the watch).
    pub why: String,
    /// The primary action's label: Start, Done, Process.
    pub action: &'static str,
}

impl Suggestion {
    /// What "Not now" remembers: the kind and what it is about.
    #[must_use]
    pub fn key(&self) -> String {
        match self {
            Self::StartPlanned { task, .. } => format!("planned:{}", task.id),
            Self::StartMode { mode, .. } => format!("mode:{}", mode.label),
            Self::Continue(t) => format!("continue:{}", t.id),
            Self::Overdue(t) => format!("overdue:{}", t.id),
            Self::Process(_) => "process".to_owned(),
            Self::DueToday(t) => format!("today:{}", t.id),
        }
    }

    /// The task it is about, when it is about one.
    #[must_use]
    pub fn task(&self) -> Option<&TaskInfo> {
        match self {
            Self::StartPlanned { task, .. } => Some(task),
            Self::Continue(t) | Self::Overdue(t) | Self::DueToday(t) => Some(t),
            Self::StartMode { .. } | Self::Process(_) => None,
        }
    }

    /// The words, worded once for every client: an eyebrow saying why
    /// now, the title, a quiet detail, and the action.
    #[must_use]
    pub fn card(&self) -> Card {
        let (eyebrow, title, detail, action) = match self {
            Self::StartPlanned { task, block, until } => (
                format!("{block} · until {until}"),
                task.title.clone(),
                "Planned for now".to_owned(),
                "Start",
            ),
            Self::StartMode {
                mode,
                until,
                waiting,
            } => (
                format!("{} · until {until}", mode.title),
                format!("Start {}", mode.name.to_lowercase()),
                match waiting {
                    0 => "Nothing tagged for it yet".to_owned(),
                    1 => "1 task waiting".to_owned(),
                    n => format!("{n} tasks waiting"),
                },
                "Start",
            ),
            Self::Continue(t) => (
                "In progress".to_owned(),
                t.title.clone(),
                "Pick up where you left off".to_owned(),
                "Done",
            ),
            Self::Overdue(t) => (
                format!(
                    "Overdue · since {}",
                    when(t).map(short_date).unwrap_or_default()
                ),
                t.title.clone(),
                "The oldest late task".to_owned(),
                "Done",
            ),
            Self::Process(n) => (
                "Inbox".to_owned(),
                if *n == 1 {
                    "Process 1 capture".to_owned()
                } else {
                    format!("Process {n} captures")
                },
                "Decide what each one is".to_owned(),
                "Process",
            ),
            Self::DueToday(t) => (
                "Due today".to_owned(),
                t.title.clone(),
                String::new(),
                "Done",
            ),
        };
        let why = if detail.is_empty() {
            eyebrow.clone()
        } else {
            format!("{eyebrow} · {detail}")
        };
        Card {
            eyebrow,
            title,
            detail,
            why,
            action,
        }
    }
}

fn day(s: &str) -> &str {
    s.get(..10).unwrap_or(s)
}

/// The task's date that matters — hard due first, else scheduled.
fn when(t: &TaskInfo) -> Option<&str> {
    t.due
        .as_deref()
        .or(t.scheduled.as_deref())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(day)
}

/// "Sep 24" from `YYYY-MM-DD`.
fn short_date(d: &str) -> String {
    chrono::NaiveDate::parse_from_str(day(d), "%Y-%m-%d")
        .map_or_else(|_| d.to_owned(), |d| d.format("%b %-d").to_string())
}

/// Everything the pick reads.
pub struct SuggestInput<'a> {
    /// What the plan says about now.
    pub now: Option<&'a NowHint>,
    /// The person's own tasks (the Tasks list's scope, `crate::is_mine`);
    /// finished ones are ignored.
    pub mine: &'a [&'a TaskInfo],
    /// The task a running timer is on, if any — already started, so not
    /// suggested again.
    pub timing: Option<Uuid>,
    /// Captures due for processing (`inbox_proto::InboxItem::is_due`).
    pub to_process: usize,
    /// Today, `YYYY-MM-DD`, local to the person.
    pub today: &'a str,
    /// [`Suggestion::key`]s set aside with "Not now".
    pub declined: &'a [String],
}

/// Every suggestion that applies, in order — the first not declined is
/// the one to show ([`suggest`]).
#[must_use]
pub fn candidates(input: &SuggestInput<'_>) -> Vec<Suggestion> {
    let open: Vec<&TaskInfo> = input
        .mine
        .iter()
        .copied()
        .filter(|t| status_is_open(&t.status))
        .collect();
    let mut out: Vec<Suggestion> = Vec::new();
    if let Some(now) = input.now {
        if let Some((block, id)) = &now.planned {
            // The planned task, wherever it is filed — the calendar
            // assigned it to the person's time, so it's theirs to start.
            if let Some(t) = open.iter().find(|t| t.id == *id)
                && input.timing != Some(*id)
            {
                out.push(Suggestion::StartPlanned {
                    task: (*t).clone(),
                    block: block.clone(),
                    until: now.until.clone(),
                });
            }
        }
        if let Some(mode) = &now.mode {
            let ctx = RelevanceContext {
                focus: mode.focus.clone(),
                ..Default::default()
            };
            out.push(Suggestion::StartMode {
                mode: mode.clone(),
                until: now.until.clone(),
                waiting: open.iter().filter(|t| matches_focus(t, &ctx)).count(),
            });
        }
    }
    out.extend(
        open.iter()
            .filter(|t| t.status == "in-progress" && input.timing != Some(t.id))
            .map(|t| Suggestion::Continue((*t).clone())),
    );
    let mut late: Vec<&TaskInfo> = open
        .iter()
        .copied()
        .filter(|t| when(t).is_some_and(|d| d < input.today))
        .collect();
    late.sort_by(|a, b| when(a).cmp(&when(b)));
    out.extend(late.into_iter().map(|t| Suggestion::Overdue(t.clone())));
    if input.to_process > 0 {
        out.push(Suggestion::Process(input.to_process));
    }
    out.extend(
        open.iter()
            .filter(|t| t.status != "in-progress" && when(t) == Some(input.today))
            .map(|t| Suggestion::DueToday((*t).clone())),
    );
    out
}

/// The one suggestion: the first [`candidates`] entry not set aside.
// t[impl process.suggested]
#[must_use]
pub fn suggest(input: &SuggestInput<'_>) -> Option<Suggestion> {
    candidates(input)
        .into_iter()
        .find(|s| !input.declined.iter().any(|d| *d == s.key()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn maintenance() -> ModeHint {
        ModeHint {
            label: "Errands & chores".into(),
            title: "Maintenance hour".into(),
            name: "Maintenance mode".into(),
            focus: vec!["errands".into(), "chores".into(), "chore".into()],
        }
    }

    fn task(title: &str, status: &str, due: Option<&str>) -> TaskInfo {
        let mut t = crate::capture(title);
        t.title = title.into();
        t.status = status.into();
        t.due = due.map(Into::into);
        t
    }

    #[test]
    // t[verify process.suggested]
    fn the_slot_walks_its_order_and_skips_what_was_set_aside() {
        let today = "2026-10-12";
        let planned = task("Comp lead vocals", "open", None);
        let errand = task("Post the parcel @errands", "open", None);
        let moving = task("Bounce stems", "in-progress", None);
        let older = task("Mix revisions", "open", Some("2026-09-24"));
        let late = task("Master review", "open", Some("2026-09-29"));
        let due = task("Send stems", "open", Some(today));
        let all = [&planned, &errand, &moving, &older, &late, &due];
        let hint = NowHint {
            planned: Some(("Studio block".into(), planned.id)),
            mode: Some(maintenance()),
            until: "5:00 PM".into(),
        };
        let mut declined: Vec<String> = Vec::new();
        let next = |declined: &mut Vec<String>| {
            let s = suggest(&SuggestInput {
                now: Some(&hint),
                mine: &all,
                timing: None,
                to_process: 3,
                today,
                declined,
            });
            if let Some(s) = &s {
                declined.push(s.key());
            }
            s
        };
        assert!(
            matches!(next(&mut declined), Some(Suggestion::StartPlanned { ref task, .. }) if task.title == "Comp lead vocals"),
            "the calendar's plan comes first"
        );
        assert!(matches!(
            next(&mut declined),
            Some(Suggestion::StartMode { waiting: 1, .. })
        ));
        assert!(
            matches!(next(&mut declined), Some(Suggestion::Continue(ref t)) if t.title == "Bounce stems")
        );
        assert!(
            matches!(next(&mut declined), Some(Suggestion::Overdue(ref t)) if t.title == "Mix revisions"),
            "oldest late first"
        );
        assert!(matches!(next(&mut declined), Some(Suggestion::Overdue(_))));
        assert_eq!(next(&mut declined), Some(Suggestion::Process(3)));
        assert!(
            matches!(next(&mut declined), Some(Suggestion::DueToday(ref t)) if t.title == "Send stems")
        );
        assert_eq!(next(&mut declined), None);
    }

    #[test]
    fn what_is_already_running_is_not_suggested_again() {
        let planned = task("Comp lead vocals", "in-progress", None);
        let hint = NowHint {
            planned: Some(("Studio block".into(), planned.id)),
            mode: None,
            until: "12:30 PM".into(),
        };
        let s = suggest(&SuggestInput {
            now: Some(&hint),
            mine: &[&planned],
            timing: Some(planned.id),
            to_process: 0,
            today: "2026-10-12",
            declined: &[],
        });
        assert_eq!(s, None);
    }

    #[test]
    fn the_plan_reads_into_a_hint() {
        use scheduling_proto::{
            BlockAssignment, BlockCategory, PlannedBlock, TimeBlockId, TimeOfDay,
        };
        let id = Uuid::new_v4();
        let block = |label: &str, s: u8, e: u8, category, assignment| PlannedBlock {
            id: TimeBlockId(label.into()),
            start: TimeOfDay::new(s, 0),
            end: TimeOfDay::new(e, 0),
            label: label.into(),
            category,
            note: None,
            assignment,
            fixed: false,
        };
        let day = vec![
            block("Getting ready", 7, 8, BlockCategory::Hygiene, None),
            block(
                "Studio block",
                9,
                12,
                BlockCategory::Allocatable,
                Some(BlockAssignment {
                    kind: "task".into(),
                    title: "Comp lead vocals".into(),
                    ref_id: Some(id.to_string()),
                }),
            ),
        ];
        let morning = now_hint(&day, 7 * 60 + 15).expect("getting ready steers");
        assert_eq!(
            morning.mode.as_ref().map(|m| m.name.as_str()),
            Some("Getting ready")
        );
        assert_eq!(morning.until, "8:00 AM");
        let studio = now_hint(&day, 10 * 60).expect("a task is planned");
        assert_eq!(studio.planned, Some(("Studio block".into(), id)));
        assert!(studio.mode.is_none(), "an allocatable block steers nothing");
        assert!(now_hint(&day, 13 * 60).is_none());
    }

    #[test]
    fn the_card_words_each_kind() {
        let s = Suggestion::StartMode {
            mode: maintenance(),
            until: "5:00 PM".into(),
            waiting: 2,
        };
        let c = s.card();
        assert_eq!(c.title, "Start maintenance mode");
        assert_eq!(c.eyebrow, "Maintenance hour · until 5:00 PM");
        assert_eq!(c.detail, "2 tasks waiting");
        assert_eq!(c.why, "Maintenance hour · until 5:00 PM · 2 tasks waiting");
        assert_eq!(c.action, "Start");
        let late = task("Mix", "open", Some("2026-09-24"));
        assert_eq!(
            Suggestion::Overdue(late).card().eyebrow,
            "Overdue · since Sep 24"
        );
    }
}
