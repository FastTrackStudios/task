//! The task board's one set of rules: which tasks are on it after the
//! person's filters, what each belongs to, and which are unfiled.
//!
//! The Tasks page, its header counts and the Work sidebar all read
//! these, so a list, a count and a sidebar cannot disagree — the
//! product's first principle ("never hide what is due") is broken the
//! moment two of them compute "today" differently.

use std::collections::HashMap;

use task_proto::TaskInfo;
use uuid::Uuid;

/// The relevance context for "now": the local clock and date, where
/// the person says they are, the project their timer runs on, and what
/// the current time block is for (`focus` — see
/// [`crate::now_block::use_now_block`]; empty when no block steers).
#[must_use]
pub fn relevance_ctx(
    location: &str,
    active_project: Option<Uuid>,
    focus: Vec<String>,
) -> task_proto::RelevanceContext {
    let now = chrono::Local::now();
    task_proto::RelevanceContext {
        local_hhmm: Some(now.format("%H:%M").to_string()),
        local_date: Some(now.format("%Y-%m-%d").to_string()),
        // FUTURE(relevancy-and-inbox): device from user-agent.
        location: (!location.is_empty()).then(|| location.to_owned()),
        device: None,
        active_project,
        focus,
    }
}

/// What the person's filters keep, and what they hold back.
pub struct Filtered<'a> {
    /// On the board.
    pub visible: Vec<&'a TaskInfo>,
    /// Finished tasks the Active filter holds back.
    pub hidden_done: usize,
    /// Open tasks the Relevant filter (time, place, timer) holds back.
    pub hidden_elsewhere: usize,
    /// The contexts those tasks wait for (`@home`, `@evening`), sorted
    /// and distinct — what the list names when it says what Relevant
    /// is holding back.
    pub held_by: Vec<String>,
    /// Open tasks in the other [`Scope`] — on the Tasks list, the
    /// project work it points to rather than shows.
    pub other_scope: usize,
}

impl Filtered<'_> {
    /// One clause for the header: what is hidden and why, or `None`.
    #[must_use]
    pub fn hidden_note(&self) -> Option<String> {
        match (self.hidden_done, self.hidden_elsewhere) {
            (0, 0) => None,
            (d, 0) => Some(format!("{d} done hidden")),
            (0, e) => Some(format!("{e} for later or elsewhere")),
            (d, e) => Some(format!("{d} done hidden · {e} for later or elsewhere")),
        }
    }
}

/// Which half of the work a board shows (see `task_proto::action`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// The Tasks list: what is yours to do — assigned to the signed-in
    /// person (Linear's "My issues"; see `task_proto::is_mine`).
    Actions,
    /// The Project work list: everything else — unassigned or someone
    /// else's, documented under its project.
    ProjectWork,
}

impl Scope {
    fn keeps(self, t: &TaskInfo, me: Option<&str>) -> bool {
        match self {
            Self::Actions => task_proto::is_mine(t, me),
            Self::ProjectWork => !task_proto::is_mine(t, me),
        }
    }
}

/// Apply the scope (next actions or project work), then the Active
/// (open only) and Relevant (`task_proto::is_relevant`) filters.
#[must_use]
pub fn filter<'a>(
    all: &[&'a TaskInfo],
    scope: Scope,
    me: Option<&str>,
    active_only: bool,
    relevant_only: bool,
    ctx: &task_proto::RelevanceContext,
) -> Filtered<'a> {
    let mut hidden_done = 0;
    let mut hidden_elsewhere = 0;
    let mut other_scope = 0;
    let mut held_by = std::collections::BTreeSet::new();
    let visible = all
        .iter()
        .copied()
        .filter(|t| {
            if !scope.keeps(t, me) {
                // Counted only while open: finished project work isn't
                // something the Tasks list should point you to.
                if task_proto::status_is_open(&t.status) {
                    other_scope += 1;
                }
                return false;
            }
            if active_only && !task_proto::status_is_open(&t.status) {
                hidden_done += 1;
                return false;
            }
            if relevant_only && !task_proto::is_relevant(t, ctx) {
                hidden_elsewhere += 1;
                held_by.extend(
                    t.contexts
                        .iter()
                        .map(|c| format!("@{}", c.trim().trim_start_matches('@').to_lowercase())),
                );
                return false;
            }
            true
        })
        .collect();
    Filtered {
        visible,
        hidden_done,
        hidden_elsewhere,
        held_by: held_by.into_iter().collect(),
        other_scope,
    }
}

/// Project names by id and by lower-cased title — what turns a
/// `projectId` into a chip and a `[[Project]]` wikilink into filing.
#[derive(Default)]
pub struct ProjectIndex {
    pub names: HashMap<Uuid, String>,
    by_title: HashMap<String, Uuid>,
}

impl ProjectIndex {
    #[must_use]
    pub fn new(projects: impl IntoIterator<Item = (Uuid, String)>) -> Self {
        let names: HashMap<Uuid, String> = projects.into_iter().collect();
        let by_title = names
            .iter()
            .map(|(id, title)| (title.to_lowercase(), *id))
            .collect();
        Self { names, by_title }
    }

    /// The task's structural anchor, widened: a `projects: [[Name]]`
    /// wikilink naming a real project counts as membership even before
    /// `projectId` is backfilled.
    #[must_use]
    pub fn anchor(&self, t: &TaskInfo) -> Option<task_proto::Anchor> {
        task_proto::anchor(t).or_else(|| {
            t.projects
                .0
                .iter()
                .find_map(|s| {
                    let name = s
                        .trim()
                        .trim_start_matches("[[")
                        .trim_end_matches("]]")
                        .trim();
                    self.by_title.get(&name.to_lowercase()).copied()
                })
                .map(task_proto::Anchor::Project)
        })
    }

    /// Open work that belongs to nothing — no project (by id or
    /// wikilink), no parent, no workstream, no milestone, no context.
    #[must_use]
    pub fn is_unfiled(&self, t: &TaskInfo) -> bool {
        self.anchor(t).is_none() && t.contexts.is_empty() && task_proto::status_is_open(&t.status)
    }
}

/// Apply a board mutation, offering Undo for the ones a person regrets:
/// completing or dropping a task (back to its previous status) and
/// deleting one (restored whole, in its own org).
///
/// The Undo closure holds only app-lifetime handles (the task store),
/// so it still works after the row — or the page that raised it — is
/// gone.
pub fn apply_with_undo(
    muts: crate::stores::TaskMutations,
    toast: dioxus_primitives::toast::Toasts,
    create_slug: &str,
    mu: task_ui::TaskMutation,
) {
    use dioxus_primitives::toast::ToastOptions;
    use task_ui::TaskMutation;
    match &mu {
        TaskMutation::SetStatus { id, status } if task_proto::status_is_terminal(status) => {
            if let Some(before) = muts.row(*id) {
                if !task_proto::status_is_terminal(&before.task.status) {
                    let (id, prev) = (*id, before.task.status.clone());
                    let slug = before.slug.clone();
                    let verb = if status == "done" {
                        "Completed"
                    } else {
                        "Marked won't do"
                    };
                    toast.success(
                        format!("{verb}: {}", before.task.title),
                        ToastOptions::new().action("Undo", move || {
                            muts.apply(
                                &slug,
                                TaskMutation::SetStatus {
                                    id,
                                    status: prev.clone(),
                                },
                            );
                        }),
                    );
                }
            }
        }
        TaskMutation::Delete { id } => {
            if let Some(before) = muts.row(*id) {
                let title = before.task.title.clone();
                toast.info(
                    format!("Deleted: {title}"),
                    ToastOptions::new().action("Undo", move || muts.restore(before.clone())),
                );
            }
        }
        _ => {}
    }
    muts.apply(create_slug, mu);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(title: &str, status: &str) -> TaskInfo {
        let mut t = TaskInfo::new(title);
        t.status = status.into();
        t
    }

    /// A next action: something with a where (an `@context`).
    fn action(title: &str, status: &str) -> TaskInfo {
        let mut t = task(title, status);
        t.contexts.push("@computer".to_owned());
        t
    }

    #[test]
    fn the_hidden_note_names_what_and_why() {
        let open = action("a", "open");
        let done = action("b", "done");
        let all = [&open, &done];
        let ctx = relevance_ctx("computer", None, Vec::new());
        let f = filter(&all, Scope::Actions, None, true, false, &ctx);
        assert_eq!(f.visible.len(), 1);
        assert_eq!(f.hidden_note().as_deref(), Some("1 done hidden"));
        assert!(
            filter(&all, Scope::Actions, None, false, false, &ctx)
                .hidden_note()
                .is_none()
        );
    }

    #[test]
    fn relevant_names_the_contexts_it_holds_back() {
        let mut home = task("water plants", "open");
        home.contexts.push("@home".to_owned());
        let mut errand = task("buy strings", "open");
        errand.contexts.push("errands".to_owned());
        let mut today = task("mix", "open");
        today.due = Some(chrono::Local::now().format("%Y-%m-%d").to_string());
        let all = [&home, &errand, &today];
        let f = filter(
            &all,
            Scope::Actions,
            None,
            true,
            true,
            &relevance_ctx("studio", None, Vec::new()),
        );
        assert_eq!(f.visible.len(), 1);
        assert_eq!(f.held_by, ["@errands", "@home"]);
        assert_eq!(f.hidden_note().as_deref(), Some("2 for later or elsewhere"));
    }

    #[test]
    fn tasks_shows_next_actions_and_points_to_project_work() {
        let errand = action("post the parcel", "open");
        let epic = task("EPIC: orchestral renderer", "open");
        let shipped = task("old issue", "done");
        let all = [&errand, &epic, &shipped];
        let ctx = relevance_ctx("computer", None, Vec::new());
        let tasks = filter(&all, Scope::Actions, None, true, false, &ctx);
        assert_eq!(tasks.visible.len(), 1);
        assert_eq!(
            tasks.other_scope, 1,
            "finished project work isn't pointed to"
        );
        let work = filter(&all, Scope::ProjectWork, None, true, false, &ctx);
        assert_eq!(work.visible.len(), 1);
        assert_eq!(work.visible[0].title, "EPIC: orchestral renderer");
    }

    #[test]
    fn a_wikilink_to_a_real_project_files_a_task() {
        let pid = Uuid::new_v4();
        let index = ProjectIndex::new([(pid, "Track Two".to_owned())]);
        let mut t = task("mix", "open");
        assert!(index.is_unfiled(&t));
        t.projects.push("[[Track Two]]".to_owned());
        assert!(!index.is_unfiled(&t));
        assert_eq!(index.anchor(&t), Some(task_proto::Anchor::Project(pid)));
    }
}
