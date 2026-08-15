//! The `EmailLinks` service over [`LinkStore`].
//!
//! One store per org, shared behind a mutex — the same threading
//! posture as `email-store`. The service is deliberately thin: all it
//! does is translate the wire types to `EmailLink`/`EntityRef` and
//! back, because the interesting decisions (idempotent upsert, keying
//! on the bare Message-ID) already live in the store.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use email_proto::{EmailLinks, EmailSyncError, LinkTarget, MessageLink};

use crate::entity::{EntityKind, EntityRef};
use crate::link::{EmailLink, bare_message_id};
use crate::store::LinkStore;

/// Service handle. Cheap to clone.
#[derive(Clone, architect::HasDispatcher)]
pub struct LinkBackend {
    store: Arc<Mutex<LinkStore>>,
}

impl LinkBackend {
    /// Open (or create) `<root>/links.db`.
    pub fn open(root: impl AsRef<std::path::Path>) -> Result<Self, String> {
        LinkStore::open(root)
            .map(|s| Self {
                store: Arc::new(Mutex::new(s)),
            })
            .map_err(|e| e.to_string())
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, LinkStore>, EmailSyncError> {
        self.store
            .lock()
            .map_err(|_| EmailSyncError::Internal("link store mutex poisoned".into()))
    }
}

fn to_entity(target: &LinkTarget) -> EntityRef {
    EntityRef::new(EntityKind::new(target.kind.clone()), target.id.clone())
}

fn to_wire(link: &EmailLink) -> MessageLink {
    MessageLink {
        message_id: link.bare_message_id().to_owned(),
        target: LinkTarget {
            kind: link.entity.kind.as_str().to_owned(),
            id: link.entity.id.clone(),
        },
        linked_at_ms: link.linked_at.map_or(0, |t| t.timestamp_millis()),
        linked_by: link.linked_by.clone().unwrap_or_else(|| "user".to_owned()),
        user_tags: link.user_tags.clone(),
    }
}

fn map_err(e: crate::error::LinkError) -> EmailSyncError {
    EmailSyncError::Internal(e.to_string())
}

impl EmailLinks for LinkBackend {
    fn link(
        &self,
        message_id: &str,
        target: LinkTarget,
        linked_by: &str,
    ) -> Result<MessageLink, EmailSyncError> {
        // Normalize on the way in: callers hand us Message-IDs from
        // envelopes (usually bare) and from raw headers (usually
        // bracketed), and the two must land on the same row.
        let link = EmailLink {
            message_id: bare_message_id(message_id).to_owned(),
            entity: to_entity(&target),
            linked_at: Some(Utc::now()),
            linked_by: Some(linked_by.to_owned()),
            user_tags: Vec::new(),
        };
        self.lock()?.upsert(&link).map_err(map_err)?;
        Ok(to_wire(&link))
    }

    fn unlink(&self, message_id: &str, target: LinkTarget) -> Result<(), EmailSyncError> {
        self.lock()?
            .unlink(bare_message_id(message_id), &to_entity(&target))
            .map_err(map_err)
    }

    fn links_for_message(&self, message_id: &str) -> Result<Vec<MessageLink>, EmailSyncError> {
        Ok(self
            .lock()?
            .links_for_message(bare_message_id(message_id))
            .map_err(map_err)?
            .iter()
            .map(to_wire)
            .collect())
    }

    fn links_for_target(&self, target: LinkTarget) -> Result<Vec<MessageLink>, EmailSyncError> {
        Ok(self
            .lock()?
            .links_for_entity(&to_entity(&target))
            .map_err(map_err)?
            .iter()
            .map(to_wire)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backend() -> (tempfile::TempDir, LinkBackend) {
        let dir = tempfile::tempdir().unwrap();
        let b = LinkBackend::open(dir.path()).unwrap();
        (dir, b)
    }

    fn project(id: &str) -> LinkTarget {
        LinkTarget {
            kind: "project".into(),
            id: id.into(),
        }
    }

    #[test]
    fn a_message_links_to_a_project_and_back() {
        let (_d, b) = backend();
        b.link("<m1@example.com>", project("praise-set"), "user")
            .unwrap();

        let by_target = b.links_for_target(project("praise-set")).unwrap();
        assert_eq!(by_target.len(), 1);
        assert_eq!(by_target[0].message_id, "m1@example.com");
        assert_eq!(by_target[0].linked_by, "user");

        let by_message = b.links_for_message("m1@example.com").unwrap();
        assert_eq!(by_message.len(), 1);
        assert_eq!(by_message[0].target, project("praise-set"));
    }

    #[test]
    fn bracketed_and_bare_ids_are_the_same_message() {
        // Envelopes hand out bare ids, raw headers hand out bracketed
        // ones. Treating them as different messages would silently
        // split a thread's links across two rows.
        let (_d, b) = backend();
        b.link("<m1@example.com>", project("p"), "user").unwrap();
        b.link("m1@example.com", project("p"), "rule").unwrap();

        let links = b.links_for_target(project("p")).unwrap();
        assert_eq!(links.len(), 1, "one row, not two");
        assert_eq!(links[0].linked_by, "rule", "re-link updates in place");

        assert_eq!(b.links_for_message("<m1@example.com>").unwrap().len(), 1);
        assert_eq!(b.links_for_message("m1@example.com").unwrap().len(), 1);
    }

    #[test]
    fn a_message_can_belong_to_several_things() {
        let (_d, b) = backend();
        b.link("m1@example.com", project("praise-set"), "user")
            .unwrap();
        b.link(
            "m1@example.com",
            LinkTarget {
                kind: "task".into(),
                id: "t-1".into(),
            },
            "user",
        )
        .unwrap();
        assert_eq!(b.links_for_message("m1@example.com").unwrap().len(), 2);
        // …and each target sees only its own.
        assert_eq!(b.links_for_target(project("praise-set")).unwrap().len(), 1);
    }

    #[test]
    fn unlink_is_idempotent_and_scoped() {
        let (_d, b) = backend();
        b.link("m1@example.com", project("p"), "user").unwrap();
        b.link("m2@example.com", project("p"), "user").unwrap();

        b.unlink("m1@example.com", project("p")).unwrap();
        // Removing something already gone succeeds.
        b.unlink("m1@example.com", project("p")).unwrap();
        b.unlink("never@seen", project("p")).unwrap();

        let left = b.links_for_target(project("p")).unwrap();
        assert_eq!(left.len(), 1, "only the named link went");
        assert_eq!(left[0].message_id, "m2@example.com");
    }
}
