//! The org's task board as the Edit lane's tracker.
//!
//! `wiki.edit.tracked`: an Edit Request *is* an issue on the owning
//! org's tracker. `wiki-live` cannot depend on the task feature — a wiki
//! has to stay a wiki — so it declares [`Tracker`] and this is the
//! server's implementation of it over the org's own
//! [`task::TaskBackend`]. The row keeps the request's id, so the two
//! views name one thing.

use task::{Status, TaskBackend, TaskError, TaskInfo, TaskService as _};
use wiki_live::edits_backend::{NewIssue, Tracker};

/// The org's tasks, as the Edit lane sees them.
#[derive(Clone)]
pub struct TaskTracker {
    tasks: TaskBackend,
}

impl TaskTracker {
    #[must_use]
    pub fn new(tasks: TaskBackend) -> Self {
        Self { tasks }
    }
}

fn err(e: TaskError) -> String {
    e.to_string()
}

impl Tracker for TaskTracker {
    fn open_issue(&self, issue: NewIssue) -> Result<uuid::Uuid, String> {
        let mut info = TaskInfo::new(issue.title);
        info.id = issue.id;
        info.details = issue.details;
        info.tags = task_proto_tags(issue.tags);
        let made = self.tasks.create(info).map_err(err)?;
        Ok(made.id)
    }

    fn issue_status(&self, id: uuid::Uuid) -> Result<Option<String>, String> {
        match self.tasks.get(id) {
            // Normalised to the canonical word where it parses, so the
            // lane's "closed" test reads `done` for `completed` too.
            Ok(t) => Ok(Some(
                Status::from_str(&t.status)
                    .map(|s| s.as_str().to_owned())
                    .unwrap_or(t.status),
            )),
            Err(TaskError::NotFound(_)) => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    fn close_issue(&self, id: uuid::Uuid, status: &str, note: &str) -> Result<(), String> {
        let mut t = self.tasks.get(id).map_err(err)?;
        t.status = status.to_owned();
        if !note.trim().is_empty() {
            if !t.details.is_empty() {
                t.details.push_str("\n\n");
            }
            t.details.push_str(note.trim());
        }
        self.tasks.update(t).map_err(err)?;
        Ok(())
    }
}

fn task_proto_tags(tags: Vec<String>) -> task::model::StringList {
    task::model::StringList(tags)
}
