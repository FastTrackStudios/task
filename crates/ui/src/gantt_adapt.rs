//! Live-data adapter: `task_proto::TaskInfo` → `view::gantt` shapes.
//!
//! Pure transform (no Dioxus, no IO) so it unit-tests cleanly. The
//! `/gantt` page ([`crate::pages::gantt`]) feeds the result straight
//! into the uncontrolled [`view::gantt::Gantt`] component.
//!
//! Rules:
//! - A task is **schedulable** when it has a `scheduled` and/or `due`
//!   date (a bare `timeEstimate` widens a one-sided date but can't
//!   place a task on its own). Tasks with neither are dropped — a
//!   Gantt bar needs a start and an end.
//! - A `workflow.parent` that is itself schedulable nests as a child;
//!   a parent that isn't schedulable but owns schedulable children is
//!   synthesized as a `Summary` spanning those children. A parent that
//!   is absent entirely flattens the child to the root.
//! - `workflow.blockers` become finish-to-start (`E2s`) dependency
//!   links when both ends survive the schedulable filter.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, NaiveDate, NaiveTime, TimeZone, Utc};
use task_proto::TaskInfo as DbTask;
use uuid::Uuid;
use view::gantt::{GanttLink, GanttTask, LinkType, TaskType};

/// Fallback bar length when a task has only one of `scheduled`/`due`
/// and no `timeEstimate` to size it.
const DEFAULT_SPAN: Duration = Duration::days(1);

/// Parse a TaskNotes date — `YYYY-MM-DD` or a full RFC-3339 timestamp.
/// Bare dates anchor at 09:00 UTC so day-granularity bars don't render
/// flush against the previous midnight boundary.
fn parse_date(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.with_timezone(&Utc));
    }
    let date = NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()?;
    let at_nine = date.and_time(NaiveTime::from_hms_opt(9, 0, 0)?);
    Some(Utc.from_utc_datetime(&at_nine))
}

/// Resolve a task's own `(start, end)` from its dates. `None` when the
/// task carries no schedulable date.
fn own_span(t: &DbTask) -> Option<(DateTime<Utc>, DateTime<Utc>)> {
    let scheduled = t.scheduled.as_deref().and_then(parse_date);
    let due = t.due.as_deref().and_then(parse_date);
    let est = t
        .time_estimate
        .map_or(DEFAULT_SPAN, |m| Duration::minutes(i64::from(m)));

    match (scheduled, due) {
        (Some(s), Some(d)) if d >= s => Some((s, d)),
        // due before scheduled — treat as a point, scheduled wins as start.
        (Some(s), Some(d)) => Some((s.min(d), s.max(d))),
        (Some(s), None) => Some((s, s + est)),
        (None, Some(d)) => Some((d - est, d)),
        (None, None) => None,
    }
}

/// Progress in `0.0..=1.0` inferred from status. Free-form statuses
/// fall through to "not started".
fn progress_of(t: &DbTask) -> f32 {
    if t.completed_date.is_some() {
        return 1.0;
    }
    match t.status.to_ascii_lowercase().as_str() {
        "done" | "complete" | "completed" => 1.0,
        "in-progress" | "in progress" | "doing" | "started" => 0.5,
        _ => 0.0,
    }
}

/// Theme-token bar tint by status, or `None` to use the chart default.
/// Tokens (not hex) so theme presets retint the chart — see AGENTS.md.
fn color_of(t: &DbTask) -> Option<String> {
    let token = match t.status.to_ascii_lowercase().as_str() {
        "done" | "complete" | "completed" => "var(--color-success)",
        "in-progress" | "in progress" | "doing" | "started" => "var(--color-primary)",
        "blocked" | "waiting" => "var(--color-warning)",
        "cancelled" | "canceled" => "var(--color-destructive)",
        _ => return None,
    };
    Some(token.to_string())
}

fn parent_of(t: &DbTask) -> Option<Uuid> {
    t.workflow
        .as_ref()
        .and_then(|w| w.parent)
        .filter(|p| *p != t.id)
}

/// Deterministic link id from its endpoints, so re-renders don't churn
/// ids (and there's no RNG, which wasm/tests dislike).
fn link_id(source: Uuid, target: Uuid) -> Uuid {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(source.as_bytes());
    bytes.extend_from_slice(target.as_bytes());
    Uuid::new_v5(&Uuid::NAMESPACE_OID, &bytes)
}

/// Convert a flat list of persisted tasks into Gantt bars + dependency
/// links. See the module docs for the filtering rules.
#[must_use]
pub fn to_gantt(tasks: &[DbTask]) -> (Vec<GanttTask>, Vec<GanttLink>) {
    let by_id: HashMap<Uuid, &DbTask> = tasks.iter().map(|t| (t.id, t)).collect();

    // Pass 1: every task that can place itself on the timeline.
    let mut spans: HashMap<Uuid, (DateTime<Utc>, DateTime<Utc>)> = HashMap::new();
    for t in tasks {
        if let Some(span) = own_span(t) {
            spans.insert(t.id, span);
        }
    }

    // Pass 2: parents that lack dates but own placed children are
    // synthesized as summaries spanning those children.
    let mut synthesized: HashSet<Uuid> = HashSet::new();
    for t in tasks {
        let Some(parent) = parent_of(t) else { continue };
        if !spans.contains_key(&t.id) {
            continue; // child isn't placed — nothing to roll up.
        }
        if spans.contains_key(&parent) || !by_id.contains_key(&parent) {
            continue; // parent already placed, or doesn't exist at all.
        }
        let (cs, ce) = spans[&t.id];
        let entry = spans.entry(parent).or_insert((cs, ce));
        entry.0 = entry.0.min(cs);
        entry.1 = entry.1.max(ce);
        synthesized.insert(parent);
    }

    // Which placed tasks are parents of other placed tasks → summaries.
    let mut has_kept_child: HashSet<Uuid> = HashSet::new();
    for t in tasks {
        if !spans.contains_key(&t.id) {
            continue;
        }
        if let Some(p) = parent_of(t) {
            if spans.contains_key(&p) {
                has_kept_child.insert(p);
            }
        }
    }

    // Emit bars in input order (synthesized parents tacked on after, in
    // input order too) so layout is stable across reloads.
    let mut bars: Vec<GanttTask> = Vec::with_capacity(spans.len());
    let mut links: Vec<GanttLink> = Vec::new();
    for t in tasks {
        let Some(&(start, end)) = spans.get(&t.id) else {
            continue;
        };

        let task_type = if synthesized.contains(&t.id) || has_kept_child.contains(&t.id) {
            TaskType::Summary
        } else {
            TaskType::Task
        };
        let parent = parent_of(t).filter(|p| spans.contains_key(p));

        bars.push(GanttTask {
            id: t.id,
            parent,
            text: t.title.clone(),
            start,
            end,
            progress: progress_of(t),
            task_type,
            open: true,
            rollup: false,
            details: (!t.details.is_empty()).then(|| t.details.clone()),
            color: color_of(t),
        });

        if let Some(w) = t.workflow.as_ref() {
            for blocker in w.blockers.iter() {
                if spans.contains_key(blocker) {
                    links.push(GanttLink {
                        id: link_id(*blocker, t.id),
                        source: *blocker,
                        target: t.id,
                        link_type: LinkType::E2s,
                        lag: 0,
                    });
                }
            }
        }
    }

    (bars, links)
}

#[cfg(test)]
mod tests {
    use super::*;

    use task_proto::model::WorkflowAttrs;

    fn base(title: &str) -> DbTask {
        task_proto::capture(title)
    }

    #[test]
    fn drops_tasks_without_dates() {
        let tasks = vec![base("no dates")];
        let (bars, links) = to_gantt(&tasks);
        assert!(bars.is_empty());
        assert!(links.is_empty());
    }

    #[test]
    fn scheduled_and_due_span_the_bar() {
        let mut t = base("ranged");
        t.scheduled = Some("2026-01-01".into());
        t.due = Some("2026-01-05".into());
        let (bars, _) = to_gantt(&[t]);
        assert_eq!(bars.len(), 1);
        assert!(bars[0].end > bars[0].start);
    }

    #[test]
    fn due_only_falls_back_to_default_span() {
        let mut t = base("deadline");
        t.due = Some("2026-01-10".into());
        let (bars, _) = to_gantt(&[t]);
        assert_eq!(bars.len(), 1);
        assert_eq!(bars[0].end - bars[0].start, DEFAULT_SPAN);
    }

    #[test]
    fn estimate_sizes_a_one_sided_task() {
        let mut t = base("estimated");
        t.scheduled = Some("2026-01-01".into());
        t.time_estimate = Some(120);
        let (bars, _) = to_gantt(&[t]);
        assert_eq!(bars[0].end - bars[0].start, Duration::minutes(120));
    }

    #[test]
    fn dateless_parent_is_synthesized_as_summary() {
        let parent = base("epic");
        let mut child = base("subtask");
        child.scheduled = Some("2026-02-01".into());
        child.due = Some("2026-02-03".into());
        child.workflow = Some(WorkflowAttrs {
            parent: Some(parent.id),
            ..Default::default()
        });
        let parent_id = parent.id;
        let (bars, _) = to_gantt(&[parent, child]);
        let summary = bars.iter().find(|b| b.id == parent_id).unwrap();
        assert_eq!(summary.task_type, TaskType::Summary);
        assert_eq!(summary.start, parse_date("2026-02-01").unwrap());
        assert_eq!(summary.end, parse_date("2026-02-03").unwrap());
    }

    #[test]
    fn blocker_becomes_dependency_link() {
        let mut a = base("first");
        a.due = Some("2026-03-01".into());
        let mut b = base("second");
        b.due = Some("2026-03-05".into());
        b.workflow = Some(WorkflowAttrs {
            blockers: vec![a.id].into(),
            ..Default::default()
        });
        let (a_id, b_id) = (a.id, b.id);
        let (_, links) = to_gantt(&[a, b]);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].source, a_id);
        assert_eq!(links[0].target, b_id);
        assert_eq!(links[0].link_type, LinkType::E2s);
    }

    #[test]
    fn completed_date_implies_full_progress() {
        let mut t = base("shipped");
        t.due = Some("2026-01-10".into());
        t.completed_date = Some(NaiveDate::from_ymd_opt(2026, 1, 9).unwrap());
        let (bars, _) = to_gantt(&[t]);
        assert!((bars[0].progress - 1.0).abs() < f32::EPSILON);
    }
}
