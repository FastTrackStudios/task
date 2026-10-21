//! The guest view of a task.
//!
//! [`guest_task`] is the same shape as [`crate::guest_project`]: one
//! exhaustive destructure of [`TaskInfo`] (and of [`WorkflowAttrs`]),
//! every field carried, redacted or stripped by name.
//!
//! t[impl sharing.grant.exclusions] — `TaskInfo` holds no rate or
//! amount, but it does hold timer data and pointers into the owner's
//! operations, which the guest never receives:
//!
//! - `time_entries` — work-session rows (timer is money: what is logged
//!   is what is billed);
//! - `workflow.session` — the active `WorkSession` id;
//! - `agent_profile`, `dispatched_agent_tasks` — agent-dispatch
//!   internals and ids of `agent_tasks` rows outside any grant;
//! - `workflow.verify_command`, `workflow.capabilities`,
//!   `workflow.model` — runner internals (a shell command line, runner
//!   capability tags, an LLM id);
//! - `created_by` — the creator's principal ref (`user:<uuid>` /
//!   `bot:<uuid>`), an id into the owner org's directory and bot
//!   roster. The display name (`created_by_name`) is carried, so the
//!   guest still sees who made the task without learning an account id.
//!
//! `time_estimate` (minutes) and `workflow.estimate` (a size) are
//! planning, not money, and are carried.
//!
//! t[impl sharing.grant.absent] — every task reference (`parent`,
//! `blockers`, `relates_to`, `relations`, reverse relations) and every
//! container reference (`project_id`, `milestone_id`, `cycle`,
//! `workstream`, the `projects:` wikilinks) is redacted through the
//! scope predicate.

use chrono::{DateTime, NaiveDate, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use task_proto::TaskInfo;
use task_proto::model::{Estimate, Relation, WorkflowAttrs};
use uuid::Uuid;
use workflows_proto::AgentRef;

use crate::link::{Linked, ObjectRef, redact_list, redact_opt};

/// `TaskInfo` fields (serde names) the guest view drops.
pub const STRIPPED_TASK_FIELDS: &[&str] = &[
    "timeEntries",
    "agentProfile",
    "dispatchedAgentTasks",
    "createdBy",
];

/// `TaskInfo` fields (serde names) the guest view carries, some of them
/// redacted. `path` and `details` are `#[serde(skip)]` on the source and
/// carried too.
pub const SHARED_TASK_FIELDS: &[&str] = &[
    "id",
    "title",
    "status",
    "priority",
    "due",
    "scheduled",
    "tags",
    "contexts",
    "projects",
    "projectId",
    "milestoneId",
    "timeEstimate",
    "recurrence",
    "recurrence_anchor",
    "complete_instances",
    "completedDate",
    "dateCreated",
    "dateModified",
    "createdByName",
    "workflow",
];

/// `WorkflowAttrs` fields (serde names) the guest view drops.
pub const STRIPPED_WORKFLOW_FIELDS: &[&str] = &["session", "verifyCommand", "capabilities", "model"];

/// `WorkflowAttrs` fields (serde names) the guest view carries.
pub const SHARED_WORKFLOW_FIELDS: &[&str] = &[
    "cycle",
    "workstream",
    "estimate",
    "parent",
    "assignees",
    "blockers",
    "relates_to",
    "relations",
];

/// A task as a grant guest receives it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[serde(rename_all = "camelCase")]
pub struct GuestTask {
    pub id: Uuid,
    pub path: String,
    pub title: String,
    pub status: String,
    pub priority: String,
    pub due: Option<String>,
    pub scheduled: Option<String>,
    pub tags: Vec<String>,
    pub contexts: Vec<String>,
    /// `[[Project]]` wikilinks; a name outside the grant is the marker.
    pub projects: Vec<Linked<String>>,
    pub project_id: Option<Linked<Uuid>>,
    pub milestone_id: Option<Linked<Uuid>>,
    pub time_estimate: Option<u32>,
    pub recurrence: Option<String>,
    pub recurrence_anchor: Option<String>,
    pub complete_instances: Vec<String>,
    pub completed_date: Option<NaiveDate>,
    pub date_created: Option<DateTime<Utc>>,
    pub date_modified: Option<DateTime<Utc>>,
    /// The creator's display name; the principal ref is stripped.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub created_by_name: Option<String>,
    pub details: String,
    pub workflow: Option<GuestWorkflow>,
}

/// A task's workflow attributes as a guest receives them.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[serde(rename_all = "camelCase")]
pub struct GuestWorkflow {
    pub cycle: Option<Linked<Uuid>>,
    pub workstream: Option<Linked<Uuid>>,
    pub estimate: Option<Estimate>,
    pub parent: Option<Linked<Uuid>>,
    pub assignees: Vec<AgentRef>,
    pub blockers: Vec<Linked<Uuid>>,
    pub relates_to: Vec<Linked<Uuid>>,
    pub relations: Vec<Linked<Relation>>,
}

/// Project a [`TaskInfo`] into its guest view. Pure; `in_scope` is the
/// grant's subtree predicate.
pub fn guest_task<S>(task: TaskInfo, in_scope: &S) -> GuestTask
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    // Exhaustive on purpose — no `..`. A new field must be decided here.
    let TaskInfo {
        id,
        path,
        title,
        status,
        priority,
        due,
        scheduled,
        tags,
        contexts,
        projects,
        project_id,
        milestone_id,
        time_estimate,
        // ── stripped: timer (t[impl sharing.grant.exclusions]) ──
        time_entries: _,
        // ── carried ──
        recurrence,
        recurrence_anchor,
        complete_instances,
        completed_date,
        // ── stripped: agent-dispatch internals ──
        agent_profile: _,
        dispatched_agent_tasks: _,
        // ── carried ──
        date_created,
        date_modified,
        // ── stripped: an owner-directory account id ──
        created_by: _,
        // ── carried ──
        created_by_name,
        details,
        workflow,
    } = task;

    GuestTask {
        id,
        path,
        title,
        status,
        priority,
        due,
        scheduled,
        tags: tags.0,
        contexts: contexts.0,
        projects: redact_list(projects.0, |s| ObjectRef::ProjectWikilink(s), in_scope),
        project_id: redact_opt(project_id, |p| ObjectRef::Project(*p), in_scope),
        milestone_id: redact_opt(milestone_id, |m| ObjectRef::Milestone(*m), in_scope),
        time_estimate,
        recurrence,
        recurrence_anchor,
        complete_instances: complete_instances.0,
        completed_date,
        date_created,
        date_modified,
        created_by_name,
        details,
        workflow: workflow.map(|w| guest_workflow(w, in_scope)),
    }
}

/// A list of tasks (`TaskService::list`), each projected. Tasks outside
/// the grant are not there at all (t[impl sharing.grant.absent]).
pub fn guest_tasks<S>(tasks: Vec<TaskInfo>, in_scope: &S) -> Vec<GuestTask>
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    tasks
        .into_iter()
        .filter(|t| in_scope(ObjectRef::Task(t.id)))
        .map(|t| guest_task(t, in_scope))
        .collect()
}

fn guest_workflow<S>(w: WorkflowAttrs, in_scope: &S) -> GuestWorkflow
where
    S: Fn(ObjectRef<'_>) -> bool + ?Sized,
{
    let WorkflowAttrs {
        cycle,
        workstream,
        estimate,
        parent,
        assignees,
        blockers,
        relates_to,
        relations,
        // ── stripped: timer + runner internals ──
        session: _,
        verify_command: _,
        capabilities: _,
        model: _,
    } = w;
    GuestWorkflow {
        cycle: redact_opt(cycle, |c| ObjectRef::Cycle(*c), in_scope),
        workstream: redact_opt(workstream, |c| ObjectRef::Workstream(*c), in_scope),
        estimate,
        parent: redact_opt(parent, |t| ObjectRef::Task(*t), in_scope),
        assignees: assignees.0,
        blockers: redact_list(blockers.0, |t| ObjectRef::Task(*t), in_scope),
        relates_to: redact_list(relates_to.0, |t| ObjectRef::Task(*t), in_scope),
        relations: redact_list(relations.0, |r| ObjectRef::Task(r.target), in_scope),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use task_proto::RelationKind;
    use task_proto::model::{
        AgentRefList, RelationList, StringList, TimeEntries, TimeEntry, UuidList,
    };

    use super::*;
    use crate::link::{NOT_SHARED, ScopeSet};

    const TASK: Uuid = Uuid::from_u128(0x7A5C);
    const SIBLING: Uuid = Uuid::from_u128(0x7A5D);
    const CRESCENDUM: Uuid = Uuid::from_u128(0xC0E5_C0E5);
    const OUTSIDE_TASK: Uuid = Uuid::from_u128(0xBAD0_0001);
    const OUTSIDE_TASK_2: Uuid = Uuid::from_u128(0xBAD0_0002);
    const SESSION: Uuid = Uuid::from_u128(0x5E55_10A0);
    const CYCLE: Uuid = Uuid::from_u128(0xC1C1E);

    fn mix_task() -> TaskInfo {
        TaskInfo {
            id: TASK,
            path: "Projects/Crescendum/tasks/mix.md".into(),
            title: "Mix the overture".into(),
            status: "open".into(),
            priority: "high".into(),
            due: Some("2026-11-01".into()),
            scheduled: None,
            tags: StringList(vec!["task".into()]),
            contexts: StringList(vec!["@studio".into()]),
            projects: StringList(vec!["[[Crescendum]]".into(), "[[Secret Client]]".into()]),
            project_id: Some(CRESCENDUM),
            milestone_id: Some(Uuid::from_u128(0x3115)),
            time_estimate: Some(90),
            time_entries: TimeEntries(vec![TimeEntry {
                start_time: DateTime::<Utc>::MIN_UTC,
                end_time: None,
            }]),
            recurrence: None,
            recurrence_anchor: None,
            complete_instances: StringList::default(),
            completed_date: None,
            agent_profile: "secret-agent-profile".into(),
            dispatched_agent_tasks: StringList(vec!["a9e47000-dead-beef-0000-000000000000".into()]),
            date_created: None,
            date_modified: None,
            created_by: Some("user:0b5e55ed-0000-0000-0000-00000000c0d1".into()),
            created_by_name: Some("Cody".into()),
            details: "Balance strings.".into(),
            workflow: Some(WorkflowAttrs {
                cycle: Some(CYCLE),
                workstream: None,
                estimate: Some(Estimate::M),
                parent: Some(SIBLING),
                assignees: AgentRefList(vec![]),
                blockers: UuidList(vec![OUTSIDE_TASK, SIBLING]),
                relates_to: UuidList(vec![OUTSIDE_TASK_2]),
                relations: RelationList(vec![
                    Relation {
                        kind: RelationKind::Blocks,
                        target: OUTSIDE_TASK,
                    },
                    Relation {
                        kind: RelationKind::Relates,
                        target: SIBLING,
                    },
                ]),
                session: Some(SESSION),
                verify_command: Some("cargo test --secret-verify".into()),
                capabilities: StringList(vec!["secret-runner-cap".into()]),
                model: Some("secret-model-id".into()),
            }),
        }
    }

    fn pred(r: ObjectRef<'_>) -> bool {
        // The S4 resolver's answer for Crescendum: the project, its
        // tasks, its milestone, and its own wikilink by name.
        let set = ScopeSet::new([TASK, SIBLING, CRESCENDUM, Uuid::from_u128(0x3115)]);
        set.contains(r) || r == ObjectRef::ProjectWikilink("[[Crescendum]]")
    }

    fn guest_json() -> serde_json::Value {
        serde_json::to_value(guest_task(mix_task(), &pred)).unwrap()
    }

    macro_rules! stripped {
        ($($test:ident: $obj:literal / $key:literal => $value:expr;)*) => {$(
            // t[verify sharing.grant.exclusions]
            #[test]
            fn $test() {
                let json = guest_json();
                let obj = if $obj == "workflow" { &json["workflow"] } else { &json };
                assert!(
                    obj.as_object().unwrap().get($key).is_none(),
                    "{} leaked",
                    $key
                );
                let value: String = $value;
                assert!(!json.to_string().contains(&value), "{}'s value leaked", $key);
            }
        )*};
    }

    stripped! {
        time_entries_are_absent: "task" / "timeEntries" => "startTime".to_owned();
        agent_profile_is_absent: "task" / "agentProfile" => "secret-agent-profile".to_owned();
        dispatched_agent_tasks_are_absent: "task" / "dispatchedAgentTasks" => "a9e47000".to_owned();
        work_session_is_absent: "workflow" / "session" => SESSION.to_string();
        verify_command_is_absent: "workflow" / "verifyCommand" => "secret-verify".to_owned();
        runner_capabilities_are_absent: "workflow" / "capabilities" => "secret-runner-cap".to_owned();
        model_is_absent: "workflow" / "model" => "secret-model-id".to_owned();
        created_by_ref_is_absent: "task" / "createdBy" => "0b5e55ed".to_owned();
    }

    #[test]
    fn every_task_field_is_classified_shared_or_stripped() {
        let full = serde_json::to_value(mix_task()).unwrap();
        let keys: BTreeSet<&str> = full.as_object().unwrap().keys().map(String::as_str).collect();
        let classified: BTreeSet<&str> = SHARED_TASK_FIELDS
            .iter()
            .chain(STRIPPED_TASK_FIELDS)
            .copied()
            .collect();
        // Every key on the wire is classified (fields serialized only
        // when set are set in `mix_task` where it matters).
        assert!(keys.is_subset(&classified), "unclassified: {:?}", keys.difference(&classified));

        // WorkflowAttrs: every field, set or not.
        let full_wf: BTreeSet<&str> = full["workflow"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        let classified_wf: BTreeSet<&str> = SHARED_WORKFLOW_FIELDS
            .iter()
            .chain(STRIPPED_WORKFLOW_FIELDS)
            .copied()
            .collect();
        assert!(
            full_wf.is_subset(&classified_wf),
            "unclassified: {:?}",
            full_wf.difference(&classified_wf)
        );
    }

    // t[verify sharing.grant.absent]
    #[test]
    fn task_links_outside_the_grant_are_one_marker_with_no_id() {
        let g = guest_task(mix_task(), &pred);
        let w = g.workflow.clone().unwrap();
        assert_eq!(w.blockers, vec![Linked::NotShared, Linked::Shared(SIBLING)]);
        assert_eq!(w.relates_to, vec![Linked::NotShared]);
        assert_eq!(w.relations.len(), 2);
        assert!(w.relations[0].is_not_shared());
        assert_eq!(w.parent, Some(Linked::Shared(SIBLING)));
        assert_eq!(w.cycle, Some(Linked::NotShared));
        assert_eq!(
            g.projects,
            vec![
                Linked::Shared("[[Crescendum]]".to_owned()),
                Linked::NotShared
            ]
        );
        assert_eq!(g.project_id, Some(Linked::Shared(CRESCENDUM)));

        let text = serde_json::to_string(&g).unwrap();
        for leak in [
            OUTSIDE_TASK.to_string(),
            OUTSIDE_TASK_2.to_string(),
            CYCLE.to_string(),
            "Secret Client".to_owned(),
        ] {
            assert!(!text.contains(&leak), "{leak} leaked");
        }
        assert!(text.contains(NOT_SHARED));
    }

    #[test]
    fn planning_fields_are_carried() {
        let g = guest_task(mix_task(), &pred);
        assert_eq!(g.time_estimate, Some(90));
        assert_eq!(g.workflow.unwrap().estimate, Some(Estimate::M));
        assert_eq!(g.details, "Balance strings.");
        assert_eq!(g.path, "Projects/Crescendum/tasks/mix.md");
        assert_eq!(g.created_by_name.as_deref(), Some("Cody"));
    }

    #[test]
    fn a_task_list_drops_tasks_outside_the_grant() {
        let mut other = mix_task();
        other.id = OUTSIDE_TASK;
        let got = guest_tasks(vec![mix_task(), other], &pred);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].id, TASK);
    }
}
