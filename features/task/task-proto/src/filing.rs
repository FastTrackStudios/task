//! Filing — what a task **belongs to**, and whether it belongs to
//! anything at all.
//!
//! A task list where a row reads only `Telemetry + Observability:
//! Sentry` is a list you can't act on: it says what to do and nothing
//! about why, for whom, or under which effort. Rather than paper over
//! that with a decorative chip, the domain names the condition — a
//! task with no [`Anchor`], no GTD context and no meaningful tag is
//! **unfiled**, and unfiled work belongs in triage until somebody says
//! what it is.
//!
//! Tags count because **areas are tags here**. An area (Health, Bible
//! Study, Prayer) is a standing part of life you maintain and never
//! finish; a project has an outcome and can be completed. Keeping
//! areas out of the project list is what stops that list lying about
//! what is finishable.
//!
//! Two consequences, both applied by [`crate::relevance`]:
//!
//! - unfiled tasks are excluded from the Relevant view (they're not
//!   "what should I do now", they're "what haven't I sorted"), and
//! - filed tasks condense **one row per anchor**, not just per
//!   project — a triaged PRD contributes its next action, not all
//!   twelve subtasks.
//!
//! Pure functions on wire types, like the rest of the sibling
//! modules: the server's `TaskService::query`, the CLI, and the web
//! store all classify identically.

use uuid::Uuid;

use crate::model::TaskInfo;

/// What a task hangs off, in precedence order. The variants are
/// ordered the way a person reads a row: the project is the headline,
/// a parent task is the next-best answer to "part of what?", a
/// workstream is the loosest, and a milestone is a date-shaped
/// grouping that only stands alone when nothing else is set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Anchor {
    /// `TaskInfo::project_id` — the authoritative owning project.
    Project(Uuid),
    /// `workflow.parent` — this is a subtask of that task.
    Parent(Uuid),
    /// `workflow.workstream` — the parent-with-swarm construct.
    Workstream(Uuid),
    /// `milestone_id` — a project checkpoint.
    Milestone(Uuid),
}

impl Anchor {
    /// The underlying id, whatever the kind. Condensation groups on
    /// the whole [`Anchor`] (kind included) so a project id and a
    /// parent id can never collide.
    #[must_use]
    pub fn id(self) -> Uuid {
        match self {
            Self::Project(id) | Self::Parent(id) | Self::Workstream(id) | Self::Milestone(id) => id,
        }
    }

    /// Lowercase kind slug — `"project"`, `"parent"`, … . Handy for
    /// UI chip styling without matching the enum in three crates.
    #[must_use]
    pub fn kind(self) -> &'static str {
        match self {
            Self::Project(_) => "project",
            Self::Parent(_) => "parent",
            Self::Workstream(_) => "workstream",
            Self::Milestone(_) => "milestone",
        }
    }
}

/// The task's filing anchor, or `None` when it hangs off nothing.
///
/// Note what is deliberately *not* consulted: the `projects:`
/// wikilink array. It's a human-readable hint that may name a page
/// that doesn't exist, so resolving it needs the project list — a
/// lookup this pure function doesn't have. Callers holding that list
/// (the web store does) resolve wikilinks to a `project_id` first and
/// then ask; see `crate::relevance::condense_next_per_anchor`, which
/// takes the resolved anchor as a closure for exactly this reason.
#[must_use]
pub fn anchor(t: &TaskInfo) -> Option<Anchor> {
    if let Some(id) = t.project_id {
        return Some(Anchor::Project(id));
    }
    if let Some(w) = t.workflow.as_ref() {
        if let Some(id) = w.parent {
            return Some(Anchor::Parent(id));
        }
        if let Some(id) = w.workstream {
            return Some(Anchor::Workstream(id));
        }
    }
    t.milestone_id.map(Anchor::Milestone)
}

/// Tags that say nothing about what a task is *for*.
///
/// Three kinds of bookkeeping, none of which is a subject:
///
/// - `task`, the page discriminator;
/// - a tag naming a [`crate::model::Status`], echoing a field the row
///   already renders;
/// - an agent-lane [`crate::agent_lane::TriageLabel`]
///   (`ready-for-agent`, `needs-review`, …), which records that an
///   agent *evaluated* the task — process, not purpose.
///
/// The last one is why filing and triage stay independent questions:
/// labelling a bare capture `ready-for-agent` must not make it look
/// filed, or the agent lane would quietly empty the triage strip.
fn is_bookkeeping_tag(tag: &str) -> bool {
    let t = tag.trim().trim_start_matches('#');
    t.is_empty()
        || t.eq_ignore_ascii_case("task")
        || crate::model::Status::from_str(t).is_some()
        || crate::agent_lane::TriageLabel::parse(t).is_some()
}

/// Does this task carry *any* context a reader could act on?
///
/// Three sufficient forms, in descending strength:
///
/// - an [`Anchor`] — it belongs to a project, parent, workstream or
///   milestone;
/// - a GTD `@context` — `buy milk @errands` is a legitimate standalone
///   task, not something to file under a project;
/// - a meaningful **tag** — `#errand`, `#health`, `#dev`.
///
/// Tags count because that is how areas are modelled here. An *area*
/// (Health, Bible Study, Prayer) is a standing part of life you
/// maintain and never finish; a *project* has an outcome and can be
/// completed. Forcing areas into projects makes the project list lie
/// about what is finishable, so areas live as tags — and a task
/// tagged `#health` is every bit as filed as one under a project.
///
/// This deliberately reversed an earlier rule that ignored tags. In a
/// vault where every task was already tagged (`#errand`, `#dev`,
/// `#habit`) and NO task was attached to a project, that rule flagged
/// well-described work as needing triage and pushed the user to invent
/// projects for things that were never projects.
///
/// A bare title with none of the three is what this module exists to
/// catch — and it stays caught.
#[must_use]
pub fn is_filed(t: &TaskInfo) -> bool {
    anchor(t).is_some()
        || !t.contexts.is_empty()
        || t.tags.iter().any(|tag| !is_bookkeeping_tag(tag))
}

/// The inverse of [`is_filed`], named for the condition rather than
/// for one caller's reaction to it.
///
/// Deliberately *not* called `needs_triage`: the agent lane has a
/// separate, explicit notion of triage — see
/// [`crate::agent_lane`], where "untriaged" means an issue carries
/// none of the four triage labels. That is a statement about whether
/// an agent has evaluated the request. This one is a statement about
/// whether the task hangs off anything. Two different questions; one
/// word between them was unresolvable in the interface.
#[must_use]
pub fn is_unfiled(t: &TaskInfo) -> bool {
    !is_filed(t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::WorkflowAttrs;

    fn bare(title: &str) -> TaskInfo {
        TaskInfo::new(title)
    }

    #[test]
    fn a_bare_title_is_unfiled() {
        let t = bare("Telemetry + Observability: Sentry");
        assert_eq!(anchor(&t), None);
        assert!(is_unfiled(&t));
    }

    #[test]
    fn a_meaningful_tag_is_enough() {
        // The case that reversed the original rule: a vault where the
        // tags carry all the meaning and nothing is attached to a
        // project. `#errand` says what this is; demanding a project
        // for it invents structure the user never wanted.
        let mut t = bare("Reach out to Erica and Kevin");
        t.tags.push("errand".into());
        assert_eq!(anchor(&t), None);
        assert!(is_filed(&t));
    }

    #[test]
    fn bookkeeping_tags_do_not_count_as_filing() {
        // `task` is the page discriminator and `open` echoes a field
        // the row already renders — neither says what the task is for.
        let mut t = bare("Telemetry + Observability: Sentry");
        t.tags = vec!["task".into(), "open".into()].into();
        assert!(is_unfiled(&t), "a bare title stays caught: {:?}", t.tags);

        // …but one real tag alongside them is enough.
        t.tags.push("observability".into());
        assert!(is_filed(&t));
    }

    #[test]
    fn areas_are_tags_and_that_is_filing() {
        // Health / Bible Study / Prayer are areas: standing parts of
        // life with no completion, so they are tags rather than
        // projects. A task in an area is filed.
        for area in ["health", "bible-study", "prayer", "relationships"] {
            let mut t = bare("some ongoing thing");
            t.tags = vec!["task".into(), area.into()].into();
            assert!(is_filed(&t), "#{area} should count as filing");
        }
    }

    #[test]
    fn a_gtd_context_is_enough_to_stay_out_of_triage() {
        let mut t = bare("buy milk");
        t.contexts.push("@errands".into());
        assert_eq!(anchor(&t), None, "a context is not an anchor");
        assert!(is_filed(&t), "but it is context enough to act on");
    }

    #[test]
    fn project_wins_over_parent_and_workstream() {
        let (p, parent, ws) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        let mut t = bare("x");
        t.project_id = Some(p);
        t.workflow = Some(WorkflowAttrs {
            parent: Some(parent),
            workstream: Some(ws),
            ..Default::default()
        });
        assert_eq!(anchor(&t), Some(Anchor::Project(p)));
    }

    #[test]
    fn parent_wins_over_workstream() {
        let (parent, ws) = (Uuid::new_v4(), Uuid::new_v4());
        let mut t = bare("x");
        t.workflow = Some(WorkflowAttrs {
            parent: Some(parent),
            workstream: Some(ws),
            ..Default::default()
        });
        assert_eq!(anchor(&t), Some(Anchor::Parent(parent)));
    }

    #[test]
    fn workstream_alone_anchors() {
        let ws = Uuid::new_v4();
        let mut t = bare("x");
        t.workflow = Some(WorkflowAttrs {
            workstream: Some(ws),
            ..Default::default()
        });
        assert_eq!(anchor(&t), Some(Anchor::Workstream(ws)));
        assert!(!is_unfiled(&t));
    }

    #[test]
    fn milestone_is_the_last_resort_anchor() {
        let m = Uuid::new_v4();
        let mut t = bare("x");
        t.milestone_id = Some(m);
        assert_eq!(anchor(&t), Some(Anchor::Milestone(m)));
    }

    #[test]
    fn anchor_kinds_do_not_collide_on_a_shared_id() {
        let id = Uuid::new_v4();
        assert_ne!(Anchor::Project(id), Anchor::Parent(id));
        assert_eq!(Anchor::Project(id).id(), Anchor::Parent(id).id());
    }
}
