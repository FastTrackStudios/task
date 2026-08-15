//! In-memory snapshot of the last reconciled state. The diff
//! between two snapshots is what we broadcast as
//! [`crate::SyncEvent::Email(...)`] entries.
//!
//! This is *not* the persistent cache — that lives in
//! `email-store`. We keep an in-memory snapshot anyway because
//! diff calculation against on-disk `SQLite` would be cycle-N×N
//! query traffic. Snapshot is the working set; the store is
//! the durable copy.

use email_proto::EmailEvent;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    /// folder-id → set of message-ids seen in the most recent
    /// envelope fetch for that folder.
    pub folders: BTreeMap<String, BTreeSet<String>>,
}

impl Snapshot {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Compute the events needed to go from `self` to `next`.
    /// Order: folder churn first (added/removed), then per-folder
    /// new messages, then deletes.
    #[must_use]
    pub fn diff(&self, next: &Snapshot) -> Vec<EmailEvent> {
        let mut events = Vec::new();

        let prev_keys: BTreeSet<&String> = self.folders.keys().collect();
        let next_keys: BTreeSet<&String> = next.folders.keys().collect();
        if prev_keys != next_keys {
            events.push(EmailEvent::FolderListChanged);
        }

        for (folder, next_msgs) in &next.folders {
            let prev_msgs = self.folders.get(folder);
            let empty = BTreeSet::new();
            let prev_msgs = prev_msgs.unwrap_or(&empty);
            for added in next_msgs.difference(prev_msgs) {
                events.push(EmailEvent::NewMessage {
                    folder: folder.clone(),
                    message_id: added.clone(),
                });
            }
            for removed in prev_msgs.difference(next_msgs) {
                events.push(EmailEvent::Deleted {
                    message_id: removed.clone(),
                });
            }
        }

        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str, msgs: &[&str]) -> (String, BTreeSet<String>) {
        (
            name.to_string(),
            msgs.iter().map(std::string::ToString::to_string).collect(),
        )
    }

    #[test]
    fn empty_to_empty_no_events() {
        let a = Snapshot::new();
        let b = Snapshot::new();
        assert!(a.diff(&b).is_empty());
    }

    #[test]
    fn new_folder_yields_folder_list_changed() {
        let a = Snapshot::new();
        let mut b = Snapshot::new();
        b.folders.extend([folder("INBOX", &["<m1>"])]);
        let events = a.diff(&b);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, EmailEvent::FolderListChanged))
        );
        assert!(events.iter().any(|e| matches!(e, EmailEvent::NewMessage { folder, message_id } if folder == "INBOX" && message_id == "<m1>")));
    }

    #[test]
    fn new_message_in_existing_folder() {
        let mut a = Snapshot::new();
        a.folders.extend([folder("INBOX", &["<m1>"])]);
        let mut b = Snapshot::new();
        b.folders.extend([folder("INBOX", &["<m1>", "<m2>"])]);
        let events = a.diff(&b);
        // No folder-list change (same keys).
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, EmailEvent::FolderListChanged))
        );
        assert_eq!(events.len(), 1);
        assert!(
            matches!(&events[0], EmailEvent::NewMessage { message_id, .. } if message_id == "<m2>")
        );
    }

    #[test]
    fn removed_message_emits_deleted() {
        let mut a = Snapshot::new();
        a.folders.extend([folder("INBOX", &["<m1>", "<m2>"])]);
        let mut b = Snapshot::new();
        b.folders.extend([folder("INBOX", &["<m1>"])]);
        let events = a.diff(&b);
        assert_eq!(events.len(), 1);
        assert!(matches!(&events[0], EmailEvent::Deleted { message_id } if message_id == "<m2>"));
    }

    #[test]
    fn idempotent_no_events_on_same_state() {
        let mut a = Snapshot::new();
        a.folders.extend([folder("INBOX", &["<m1>", "<m2>"])]);
        let b = a.clone();
        assert!(a.diff(&b).is_empty());
    }
}
