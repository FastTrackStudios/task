//! Next actions and project work — what belongs on a personal task list.
//!
//! The Tasks list is for **next actions**: the things a person can pick
//! up and do. A row that only names a piece of a project ("EPIC:
//! Orchestral renderer", "W1 · Session ⇄ Task hydration bridge") is
//! **project work** — real, tracked, worth doing, but an issue in a
//! project rather than something to do in the next free hour. Mixing
//! the two is how a personal list turns into a backlog nobody acts on.
//!
//! The line is drawn by shape, not by who made the task: a task is a
//! next action when it says *where or when* — an `@context` (errands,
//! home, calls), a due or scheduled date, a repeat — or when it is
//! already moving (in progress, waiting on someone). Processing an
//! inbox item gives a task exactly those, so everything that comes
//! through capture → process lands on the list; issues created for a
//! project don't, and stay with the project until someone gives one a
//! context or a date.
//!
//! **Assignment decides it when there is someone to ask.** "Mine" —
//! [`is_assigned_to`] the signed-in person — is what puts an issue on
//! their Tasks list (Linear's "My issues"); unassigned or someone else's
//! is documented work, under its project. The shape test above is the
//! fallback for when nobody is signed in. Processing and the composer
//! assign new work to whoever made it, so a capture that becomes a task
//! lands on its maker's list.
//!
//! Pure functions on wire types, like the sibling modules: every
//! surface that draws the list classifies identically.

use crate::model::{Status, TaskInfo};

/// Whether `user_id` is one of the task's human assignees.
#[must_use]
pub fn is_assigned_to(task: &TaskInfo, user_id: &str) -> bool {
    task.workflow.as_ref().is_some_and(|w| {
        w.assignees.iter().any(
            |a| matches!(a, workflows_proto::AgentRef::Human { user_id: u, .. } if u == user_id),
        )
    })
}

/// Whether the task belongs on `me`'s Tasks list: assigned to them, or —
/// with nobody signed in — shaped like a next action.
#[must_use]
// t[impl process.outcome.mine]
pub fn is_mine(task: &TaskInfo, me: Option<&str>) -> bool {
    match me {
        Some(id) => is_assigned_to(task, id),
        None => is_next_action(task),
    }
}

/// Whether the task is a next action (see the module docs). Status
/// does not decide it except to say "already moving" — a finished
/// next action is still one, so the Done section keeps its shape.
#[must_use]
pub fn is_next_action(task: &TaskInfo) -> bool {
    let set = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
    !task.contexts.is_empty()
        || set(&task.due)
        || set(&task.scheduled)
        || set(&task.recurrence)
        || matches!(
            Status::from_str(&task.status),
            Some(Status::InProgress | Status::Waiting)
        )
}

/// The other half: project work, which lives under its project.
#[must_use]
pub fn is_project_work(task: &TaskInfo) -> bool {
    !is_next_action(task)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bare(title: &str) -> TaskInfo {
        let mut t = crate::capture(title);
        t.priority = "high".into();
        t
    }

    #[test]
    fn an_issue_with_only_a_priority_is_project_work() {
        let epic = bare("EPIC: Orchestral renderer");
        assert!(is_project_work(&epic));
        let mut sub = bare("W1 · Session ⇄ Task hydration bridge");
        sub.project_id = Some(uuid::Uuid::new_v4());
        assert!(
            is_project_work(&sub),
            "a project alone doesn't make it a next action"
        );
    }

    #[test]
    fn where_or_when_makes_a_next_action() {
        assert!(is_next_action(&crate::capture("Post the parcel @errands")));
        assert!(is_next_action(&crate::capture("Call the bank tomorrow")));
        let mut sched = bare("Mix review");
        sched.scheduled = Some("2026-10-14".into());
        assert!(is_next_action(&sched));
        assert!(is_next_action(&crate::capture(
            "Water the plants every week"
        )));
    }

    #[test]
    fn work_already_moving_is_a_next_action() {
        let mut t = bare("Comp lead vocals");
        t.status = "in-progress".into();
        assert!(is_next_action(&t));
        t.status = "waiting".into();
        assert!(is_next_action(&t));
        t.status = "open".into();
        assert!(!is_next_action(&t));
    }

    #[test]
    // t[verify process.outcome.mine]
    fn assignment_decides_mine_and_shape_is_the_signed_out_fallback() {
        use workflows_proto::AgentRef;
        let mut t = crate::capture("Bounce stems @errands");
        assert!(
            !is_mine(&t, Some("u1")),
            "a context alone doesn't make it mine"
        );
        assert!(is_mine(&t, None), "signed out, the shape decides");
        let mut wf = crate::model::WorkflowAttrs::default();
        wf.assignees.0.push(AgentRef::Agent {
            name: "claude".into(),
            model_version: None,
        });
        t.workflow = Some(wf.clone());
        assert!(!is_assigned_to(&t, "u1"), "an agent isn't the person");
        wf.assignees.0.push(AgentRef::Human {
            user_id: "u1".into(),
            display_name: None,
        });
        t.workflow = Some(wf);
        assert!(is_mine(&t, Some("u1")));
        assert!(
            !is_mine(&t, Some("u2")),
            "someone else's is documented work to u2"
        );
    }

    #[test]
    fn a_blank_date_is_no_date() {
        let mut t = bare("Pending");
        t.due = Some("  ".into());
        assert!(is_project_work(&t));
    }
}
