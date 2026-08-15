//! Derived progress rollups — the generalized rollup engine.
//!
//! Pure functions over the org's task list: nothing is stored,
//! the numbers are recomputed from `task::TaskInfo` rows on
//! every call. One engine ([`rollup_tasks`]) serves every
//! membership shape:
//!
//! - **workstream** ([`rollup`] / [`rollup_with`]) — members are
//!   tasks with `workflow.workstream == Some(id)`;
//! - **sub-issues** ([`subtask_rollup`]) — members are children,
//!   `workflow.parent == Some(id)` (the `task issue rollup` /
//!   `issue subtasks` surface).
//!
//! Semantics — classification routes through canonical *state
//! groups* (`project::states`), never raw status strings, so
//! per-project custom status names roll up correctly:
//! - `groups` carries the member count per canonical state
//!   group (backlog / unstarted / started / completed /
//!   cancelled) — sums to `total` — so rollup-only headers can
//!   render segmented bars without fetching member tasks.
//! - `done` counts the `completed` group only (cancelled tasks
//!   stay in `total` but aren't "progress").
//! - `in_progress` counts the `started` group (includes
//!   `waiting` — claimed work that's paused).
//! - `blocked` counts still-open attached tasks with at least
//!   one *unresolved* blocker. A blocker is resolved only when
//!   the referenced task exists in the provided list and its
//!   group is closed (completed / cancelled) — the same rule
//!   `task issue ready` uses, so a workstream's blocked count
//!   matches what agents see.
//! - `estimate_points_sum` weights XS/S/M/L/XL as 1/2/3/5/8;
//!   `Points { value }` counts at face value; no estimate = 0.

use std::collections::HashMap;

use project::states::{StateGroup, resolve_state_group};
use task::TaskInfo;
use task::model::Estimate;
use uuid::Uuid;
use workstream_proto::WorkstreamRollup;

/// Bucket weight for an estimate: XS/S/M/L/XL → 1/2/3/5/8,
/// `Points` at face value.
#[must_use]
pub fn estimate_points(e: Estimate) -> u32 {
    match e {
        Estimate::XS => 1,
        Estimate::S => 2,
        Estimate::M => 3,
        Estimate::L => 5,
        Estimate::XL => 8,
        Estimate::Points { value } => u32::from(value),
    }
}

/// The generalized rollup engine: derive progress over the
/// member tasks selected by `member`, classifying statuses via
/// `group_of` (per-project state registries plug in here — see
/// `project::states::resolve_state_group`).
///
/// `org_tasks` should be the *full* org list (not pre-filtered)
/// so blocker references resolve across membership boundaries.
/// Blocker resolution merges both relation encodings (legacy
/// `blockers` lists + typed `blocks` relations) via
/// `task::relations::blockers_of`.
pub fn rollup_tasks<M, G>(org_tasks: &[TaskInfo], member: M, group_of: G) -> WorkstreamRollup
where
    M: Fn(&TaskInfo) -> bool,
    G: Fn(&TaskInfo) -> StateGroup,
{
    let by_id: HashMap<Uuid, &TaskInfo> = org_tasks.iter().map(|t| (t.id, t)).collect();
    let blockers = task::relations::blockers_of(org_tasks);
    let is_closed = |t: &TaskInfo| group_of(t).is_closed();

    let mut out = WorkstreamRollup::default();
    for t in org_tasks {
        if !member(t) {
            continue;
        }
        out.total += 1;
        match group_of(t) {
            StateGroup::Backlog => out.groups.backlog += 1,
            StateGroup::Unstarted => out.groups.unstarted += 1,
            StateGroup::Started => out.groups.started += 1,
            StateGroup::Completed => out.groups.completed += 1,
            StateGroup::Cancelled => out.groups.cancelled += 1,
        }
        // Legacy aggregates — kept in lockstep with `groups` so
        // existing consumers (CLI summary lines, progress bars)
        // don't have to migrate.
        out.done = out.groups.completed;
        out.in_progress = out.groups.started;
        if let Some(w) = &t.workflow {
            out.estimate_points_sum += w.estimate.map_or(0, estimate_points);
        }
        // Blocked: still open, with >= 1 unresolved blocker
        // (either encoding). Unresolved = the referenced task
        // is missing from the org list OR its group isn't
        // closed yet.
        if !is_closed(t)
            && blockers.get(&t.id).is_some_and(|bs| {
                bs.iter()
                    .any(|bid| !by_id.get(bid).copied().is_some_and(is_closed))
            })
        {
            out.blocked += 1;
        }
    }
    out
}

/// Workstream rollup with an explicit group classifier —
/// members are tasks with `workflow.workstream == Some(id)`.
pub fn rollup_with<G>(workstream_id: Uuid, org_tasks: &[TaskInfo], group_of: G) -> WorkstreamRollup
where
    G: Fn(&TaskInfo) -> StateGroup,
{
    rollup_tasks(
        org_tasks,
        |t| {
            t.workflow
                .as_ref()
                .and_then(|w| w.workstream)
                .is_some_and(|ws| ws == workstream_id)
        },
        group_of,
    )
}

/// Derive the progress rollup for one workstream from the org's
/// task list, classifying with the default state registry.
/// Callers that know per-project registries use [`rollup_with`]
/// (the backend resolves each task's owning project).
#[must_use]
pub fn rollup(workstream_id: Uuid, org_tasks: &[TaskInfo]) -> WorkstreamRollup {
    rollup_with(workstream_id, org_tasks, |t| {
        resolve_state_group(None, &t.status)
    })
}

/// Sub-issue rollup: derived progress over the direct children
/// of `parent_id` (`workflow.parent == Some(parent_id)`). Same
/// shape as the workstream rollup — done / total / in-progress /
/// blocked / estimate-points — surfaced by `task issue rollup`
/// and the `issue subtasks` header.
pub fn subtask_rollup<G>(parent_id: Uuid, org_tasks: &[TaskInfo], group_of: G) -> WorkstreamRollup
where
    G: Fn(&TaskInfo) -> StateGroup,
{
    rollup_tasks(
        org_tasks,
        |t| {
            t.workflow
                .as_ref()
                .and_then(|w| w.parent)
                .is_some_and(|p| p == parent_id)
        },
        group_of,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use task::model::{UuidList, WorkflowAttrs};

    fn ws_task(ws: Option<Uuid>, status: &str) -> TaskInfo {
        let mut t = task::parse_str("tasks/x.md", "x", "---\ntype: task\ntitle: x\n---\n")
            .expect("minimal task parses");
        t.id = Uuid::new_v4();
        t.status = status.into();
        if let Some(ws) = ws {
            t.workflow = Some(WorkflowAttrs {
                workstream: Some(ws),
                ..Default::default()
            });
        }
        t
    }

    fn with_estimate(mut t: TaskInfo, e: Estimate) -> TaskInfo {
        t.workflow
            .get_or_insert_with(WorkflowAttrs::default)
            .estimate = Some(e);
        t
    }

    #[test]
    fn rollup_counts_statuses_and_ignores_other_workstreams() {
        let ws = Uuid::new_v4();
        let other = Uuid::new_v4();
        let tasks = vec![
            ws_task(Some(ws), "done"),
            ws_task(Some(ws), "in-progress"),
            ws_task(Some(ws), "open"),
            ws_task(Some(other), "done"),
            ws_task(None, "open"),
        ];
        let r = rollup(ws, &tasks);
        assert_eq!(r.total, 3);
        assert_eq!(r.done, 1);
        assert_eq!(r.in_progress, 1);
        assert_eq!(r.blocked, 0);
        // Per-group counts mirror the same classification and
        // sum to total.
        assert_eq!(r.groups.completed, 1);
        assert_eq!(r.groups.started, 1);
        assert_eq!(r.groups.unstarted, 1, "open classifies as unstarted");
        assert_eq!(r.groups.backlog, 0);
        assert_eq!(r.groups.cancelled, 0);
        assert_eq!(
            r.groups.backlog
                + r.groups.unstarted
                + r.groups.started
                + r.groups.completed
                + r.groups.cancelled,
            r.total
        );
    }

    #[test]
    fn rollup_groups_cover_all_five_buckets() {
        use project::states::{StateDef, StateGroup, StatesConfig, resolve_state_group};

        let ws = Uuid::new_v4();
        // Custom registry: `shipped-to-client` → completed —
        // the acceptance case: a custom Completed state counts
        // as done only via the registry, never by string match.
        let cfg = StatesConfig(vec![StateDef {
            name: "shipped-to-client".into(),
            group: StateGroup::Completed,
            color: String::new(),
            default: false,
            order: 0,
        }]);
        let tasks = vec![
            ws_task(Some(ws), "triage"),            // backlog
            ws_task(Some(ws), "open"),              // unstarted
            ws_task(Some(ws), "in-progress"),       // started
            ws_task(Some(ws), "waiting"),           // started
            ws_task(Some(ws), "shipped-to-client"), // completed (custom!)
            ws_task(Some(ws), "cancelled"),         // cancelled
        ];

        let r = rollup_with(ws, &tasks, |t| resolve_state_group(Some(&cfg), &t.status));
        assert_eq!(r.total, 6);
        assert_eq!(r.groups.backlog, 1);
        assert_eq!(r.groups.unstarted, 1);
        assert_eq!(r.groups.started, 2);
        assert_eq!(r.groups.completed, 1, "custom completed state counts");
        assert_eq!(r.groups.cancelled, 1);
        assert_eq!(r.done, 1, "done == groups.completed");
        assert_eq!(r.in_progress, 2, "in_progress == groups.started");

        // Default registry: `shipped-to-client` is unknown →
        // unstarted, so it does NOT count as done.
        let r2 = rollup(ws, &tasks);
        assert_eq!(r2.done, 0, "custom name needs the registry");
        assert_eq!(r2.groups.unstarted, 2);
    }

    #[test]
    fn rollup_blocked_requires_unresolved_blocker() {
        let ws = Uuid::new_v4();
        let done_blocker = ws_task(None, "done");
        let open_blocker = ws_task(None, "open");

        // Blocked: open task, blocker still open.
        let mut blocked = ws_task(Some(ws), "open");
        blocked.workflow.as_mut().unwrap().blockers = UuidList(vec![open_blocker.id]);
        // Not blocked: every blocker closed.
        let mut clear = ws_task(Some(ws), "open");
        clear.workflow.as_mut().unwrap().blockers = UuidList(vec![done_blocker.id]);
        // Blocked: dangling blocker reference counts as unresolved.
        let mut dangling = ws_task(Some(ws), "open");
        dangling.workflow.as_mut().unwrap().blockers = UuidList(vec![Uuid::new_v4()]);
        // Not blocked: the task itself is already done — a closed
        // task can't be "blocked" no matter its blocker list.
        let mut done = ws_task(Some(ws), "done");
        done.workflow.as_mut().unwrap().blockers = UuidList(vec![open_blocker.id]);

        let tasks = vec![done_blocker, open_blocker, blocked, clear, dangling, done];
        let r = rollup(ws, &tasks);
        assert_eq!(r.total, 4);
        assert_eq!(r.blocked, 2, "open-blocker + dangling-blocker tasks");
    }

    #[test]
    fn rollup_estimate_points_sum_uses_bucket_weights() {
        let ws = Uuid::new_v4();
        let tasks = vec![
            with_estimate(ws_task(Some(ws), "open"), Estimate::XS), // 1
            with_estimate(ws_task(Some(ws), "open"), Estimate::S),  // 2
            with_estimate(ws_task(Some(ws), "open"), Estimate::M),  // 3
            with_estimate(ws_task(Some(ws), "open"), Estimate::L),  // 5
            with_estimate(ws_task(Some(ws), "done"), Estimate::XL), // 8
            with_estimate(ws_task(Some(ws), "open"), Estimate::Points { value: 13 }),
            ws_task(Some(ws), "open"), // no estimate → 0
        ];
        let r = rollup(ws, &tasks);
        assert_eq!(r.estimate_points_sum, 1 + 2 + 3 + 5 + 8 + 13);
        assert_eq!(r.total, 7);
    }

    #[test]
    fn rollup_empty_workstream_is_all_zero() {
        let r = rollup(Uuid::new_v4(), &[]);
        assert_eq!(r, WorkstreamRollup::default());
    }

    fn child_task(parent: Option<Uuid>, status: &str) -> TaskInfo {
        let mut t = ws_task(None, status);
        if let Some(p) = parent {
            t.workflow = Some(WorkflowAttrs {
                parent: Some(p),
                ..Default::default()
            });
        }
        t
    }

    #[test]
    fn subtask_rollup_counts_children_via_state_groups() {
        use project::states::{StateDef, StateGroup, StatesConfig, resolve_state_group};

        let parent = Uuid::new_v4();
        // Custom registry: `shipped` → completed, `building` → started.
        let cfg = StatesConfig(vec![
            StateDef {
                name: "shipped".into(),
                group: StateGroup::Completed,
                color: String::new(),
                default: false,
                order: 0,
            },
            StateDef {
                name: "building".into(),
                group: StateGroup::Started,
                color: String::new(),
                default: false,
                order: 1,
            },
        ]);
        let tasks = vec![
            child_task(Some(parent), "shipped"),  // done (custom name!)
            child_task(Some(parent), "building"), // in-progress
            child_task(Some(parent), "open"),     // unstarted (builtin fallback)
            child_task(None, "shipped"),          // not a child
            ws_task(None, "open"),                // unrelated
        ];
        let r = subtask_rollup(parent, &tasks, |t| {
            resolve_state_group(Some(&cfg), &t.status)
        });
        assert_eq!(r.total, 3);
        assert_eq!(r.done, 1, "custom `shipped` classifies as completed");
        assert_eq!(r.in_progress, 1, "custom `building` classifies as started");
        assert_eq!(r.blocked, 0);

        // Same data through the default registry: nothing is
        // done (`shipped` aliases to completed via builtins —
        // pick a truly custom name to prove the difference).
        let r2 = subtask_rollup(parent, &tasks, |t| resolve_state_group(None, &t.status));
        assert_eq!(r2.total, 3);
        assert_eq!(r2.done, 1, "builtin alias still maps shipped");
    }

    #[test]
    fn subtask_rollup_custom_name_needs_registry() {
        use project::states::{StateDef, StateGroup, StatesConfig, resolve_state_group};
        let parent = Uuid::new_v4();
        let tasks = vec![child_task(Some(parent), "qa-review")];
        // Without a registry: unknown → unstarted (not done).
        let r = subtask_rollup(parent, &tasks, |t| resolve_state_group(None, &t.status));
        assert_eq!((r.total, r.done, r.in_progress), (1, 0, 0));
        // With `qa-review` registered as started → in-progress.
        let cfg = StatesConfig(vec![StateDef {
            name: "qa-review".into(),
            group: StateGroup::Started,
            color: String::new(),
            default: false,
            order: 0,
        }]);
        let r = subtask_rollup(parent, &tasks, |t| {
            resolve_state_group(Some(&cfg), &t.status)
        });
        assert_eq!((r.total, r.done, r.in_progress), (1, 0, 1));
    }

    #[test]
    fn rollup_blocked_merges_typed_blocks_relations() {
        use task::model::{Relation, RelationKind, RelationList};

        let ws = Uuid::new_v4();
        let open_blocker = ws_task(None, "open");
        // Typed encoding: blocker declares `blocks → victim`
        // (no legacy `blockers` list on the victim at all).
        let victim = ws_task(Some(ws), "open");
        let mut blocker = open_blocker.clone();
        blocker.id = Uuid::new_v4();
        blocker.workflow = Some(WorkflowAttrs {
            relations: RelationList(vec![Relation {
                kind: RelationKind::Blocks,
                target: victim.id,
            }]),
            ..Default::default()
        });

        let tasks = vec![blocker, victim];
        let r = rollup(ws, &tasks);
        assert_eq!(r.total, 1);
        assert_eq!(r.blocked, 1, "typed blocks edge counts as a blocker");
    }
}
