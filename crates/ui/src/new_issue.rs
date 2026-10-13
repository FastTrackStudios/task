//! Capture — the app's one way to write something down, on
//! [`task_ui::Composer`]. A capture lands in the inbox unless it is given
//! what a task has (a person, a date, a priority…), in which case it is
//! a task; a project is the one deliberate choice.
//!
//! [`NewIssue`] is the context any surface opens it through (the capture
//! button, the Tasks header's New task, the desktop quick-add, `c` on a
//! desk); the [`NewIssueHost`] mounted once in the shell supplies what
//! the composer needs from the stores — the people (signed-in person
//! first, so new tasks are yours unless you say otherwise), the org's
//! projects, the labels in use — and writes what it makes: captures to
//! the inbox, tasks (subtasks included) to the shared optimistic task
//! store, projects through the inbox's process-into-project.

use dioxus::prelude::*;
use task_ui::{Composer, TaskMutation};
use uuid::Uuid;

use crate::stores;

/// Open the composer with this title text, as an issue or as a capture.
#[derive(Clone, Copy)]
pub struct NewIssue(pub Signal<Option<(String, task_ui::CaptureKind)>>);

impl NewIssue {
    /// A new issue, with every property in reach.
    pub fn open(mut self, title: impl Into<String>) {
        self.0.set(Some((title.into(), task_ui::CaptureKind::Task)));
    }

    /// A capture: written down for later unless the text (or the
    /// switch) says it is an issue or a project.
    pub fn capture(mut self, text: impl Into<String>) {
        self.0.set(Some((text.into(), task_ui::CaptureKind::Note)));
    }
}

/// Install the [`NewIssue`] context. Call once in the app shell.
pub fn provide_new_issue() {
    use_context_provider(|| NewIssue(Signal::new(None)));
}

/// The composer, while [`NewIssue`] holds a title.
#[component]
pub fn NewIssueHost() -> Element {
    let mut open = use_context::<NewIssue>().0;
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let account = use_context::<crate::auth::AuthCtx>().active;
    let task_store = stores::use_task_store();
    let project_store = stores::use_project_store();
    let muts = stores::use_task_mutations();
    let inbox = stores::use_inbox_mutations();
    let timers = stores::use_timer_mutations();
    let active_timer = crate::chrome::use_active_timer();
    let target = use_memo(move || crate::orgs::create_target(&selection.read(), &org_list.read()));
    let member_names = crate::pages::wiki_access::use_member_names(target);

    let Some((initial, kind)) = open() else {
        return rsx! {};
    };
    let slug = target();
    let place = org_list
        .read()
        .iter()
        .find(|o| o.slug == slug)
        .map(|o| o.name.clone())
        .unwrap_or_default();

    let me = account
        .read()
        .as_ref()
        .map(|a| (a.user_id.to_string(), a.name.clone()));
    let mut people: Vec<(String, String)> = member_names
        .read()
        .iter()
        .map(|(id, name)| (id.clone(), name.clone()))
        .filter(|(id, _)| me.as_ref().is_none_or(|(m, _)| m != id))
        .collect();
    people.sort_by(|a, b| a.1.to_lowercase().cmp(&b.1.to_lowercase()));
    if let Some(me) = me {
        people.insert(0, me);
    }
    let projects: Vec<(Uuid, String)> = project_store
        .list()
        .iter()
        .filter(|r| r.slug == slug && !r.project.archived)
        .map(|r| (r.project.id, r.project.title.clone()))
        .collect();
    let mut known_labels: Vec<String> = Vec::new();
    for r in task_store.list().iter() {
        for l in task_ui::labels_of(&r.task) {
            if !known_labels.iter().any(|k| k.eq_ignore_ascii_case(&l)) {
                known_labels.push(l);
            }
        }
    }
    known_labels.sort_by_key(|l| l.to_lowercase());

    rsx! {
        Composer {
            people,
            projects,
            known_labels,
            initial_title: initial,
            kind,
            // A note is a capture, kept verbatim in the inbox until it
            // is processed (`process.capture.single-path`).
            on_note: {
                let slug = slug.clone();
                move |text: String| {
                    let item = inbox_proto::InboxItem::capture(
                        Uuid::new_v4().to_string(),
                        text,
                        "ui",
                        chrono::Utc::now().to_rfc3339(),
                    );
                    inbox.capture(slug.clone(), item);
                }
            },
            // A project is made from a capture too, processed on the
            // spot, so it keeps the words it started as.
            on_project: {
                let slug = slug.clone();
                move |(name, about): (String, String)| {
                    let body = if about.is_empty() { name.clone() } else { format!("{name}\n\n{about}") };
                    let item = inbox_proto::InboxItem::capture(
                        Uuid::new_v4().to_string(),
                        body,
                        "ui",
                        chrono::Utc::now().to_rfc3339(),
                    );
                    let mut project = stores::draft_project(name);
                    project.details = about;
                    inbox.process_into_project(slug.clone(), item, project);
                }
            },
            on_timer: move |desc: String| {
                let Some((slug, org_id)) = crate::chrome::resolve_org(&selection.read(), &org_list.read()) else {
                    return;
                };
                let req = timer_proto::StartTimerRequest {
                    user_id: crate::chrome::owner_id(org_id),
                    org_id,
                    project_id: None,
                    project_path: String::new(),
                    task_note_path: String::new(),
                    description: desc,
                };
                match active_timer.as_ref() {
                    Some(at) => timers.switch(slug, at.session.id, req),
                    None => timers.start(slug, req),
                }
            },
            place,
            on_create: move |(mut task, subs): (task_proto::TaskInfo, Vec<String>)| {
                // The id is chosen here so the sub-issues can name their
                // parent before the server has answered.
                task.id = Uuid::new_v4();
                let parent = task.clone();
                muts.apply(&slug, TaskMutation::Create { task });
                for title in subs {
                    let mut sub = task_proto::capture(&title);
                    sub.tags.0.push("subtask".into());
                    sub.project_id = parent.project_id;
                    sub.projects = parent.projects.clone();
                    sub.workflow = Some(task_proto::model::WorkflowAttrs {
                        parent: Some(parent.id),
                        // A sub-issue starts with its parent's person.
                        assignees: parent.workflow.as_ref().map(|w| w.assignees.clone()).unwrap_or_default(),
                        ..Default::default()
                    });
                    muts.apply(&slug, TaskMutation::Create { task: sub });
                }
            },
            on_close: move |()| open.set(None),
        }
    }
}
