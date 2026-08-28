//! Server-side [`GoalService`] backend — walks the vault for
//! `type: goal` pages.
//!
//! Cheap to `Clone`. Each call re-opens the vault; future
//! optimization: cache with mtime check (mirror cookbook /
//! project backend).

use std::path::{Path, PathBuf};

use chrono::Utc;
use uuid::Uuid;
use vault::Vault;

use crate::model::Goal;
use crate::parse::{looks_like_goal, parse_page};
use crate::service::{GoalError, GoalService};
use crate::write::{default_goal_path, write_goal};

#[derive(Clone, architect::HasDispatcher)]
pub struct GoalBackend {
    vault_root: PathBuf,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful mutation publishes the post-write state here
    /// ([`GoalEvent::Upserted`] / [`GoalEvent::Deleted`]). Sliding
    /// mailbox: a slow subscriber loses its *oldest* queued events,
    /// which is correct for state-shaped payloads. Clones share the
    /// hub (it's `Arc` inside).
    ///
    /// [`GoalEvent::Upserted`]: crate::service::GoalEvent::Upserted
    /// [`GoalEvent::Deleted`]: crate::service::GoalEvent::Deleted
    #[cfg(feature = "vox")]
    events: architect::PubSub<crate::service::GoalEvent>,
}

// Manual impl: `PubSub` carries no `Debug`.
impl std::fmt::Debug for GoalBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GoalBackend")
            .field("vault_root", &self.vault_root)
            .finish_non_exhaustive()
    }
}

impl GoalBackend {
    #[must_use]
    pub fn new(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            vault_root: vault_root.into(),
            #[cfg(feature = "vox")]
            events: architect::PubSub::sliding(256),
        }
    }

    /// Publish a goal change to every `events` subscriber. Call only
    /// after the write succeeded — subscribers fold these into state
    /// fetched via `list()`, so a phantom event would desync them.
    /// No-op without the `vox` feature (no wire, no subscribers).
    fn publish(&self, event: crate::service::GoalEvent) {
        #[cfg(feature = "vox")]
        self.events.publish(event);
        #[cfg(not(feature = "vox"))]
        let _ = event;
    }

    #[must_use]
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    fn list_inner(&self) -> Result<Vec<Goal>, GoalError> {
        let vault = Vault::open(&self.vault_root)
            .map_err(|e| GoalError::Io(format!("open vault {}: {e}", self.vault_root.display())))?;
        let mut out = Vec::new();
        for page in &vault.pages {
            let proto = page.to_proto();
            if !looks_like_goal(&proto) {
                continue;
            }
            match parse_page(&proto) {
                Ok(g) => out.push(g),
                Err(e) => tracing::warn!(path = %page.rel_path, ?e, "goal parse failed"),
            }
        }
        Ok(out)
    }
}

impl GoalService for GoalBackend {
    fn list(&self) -> Result<Vec<Goal>, GoalError> {
        self.list_inner()
    }

    fn get(&self, id: Uuid) -> Result<Goal, GoalError> {
        self.list_inner()?
            .into_iter()
            .find(|g| g.id == id)
            .ok_or_else(|| GoalError::NotFound(id.to_string()))
    }

    fn get_by_path(&self, path: &str) -> Result<Goal, GoalError> {
        self.list_inner()?
            .into_iter()
            .find(|g| g.path == path)
            .ok_or_else(|| GoalError::NotFound(path.to_owned()))
    }

    fn create(&self, mut goal: Goal) -> Result<Goal, GoalError> {
        if goal.title.trim().is_empty() {
            return Err(GoalError::BadRequest("title is required".into()));
        }
        if goal.id.is_nil() {
            goal.id = Uuid::new_v4();
        }
        if goal.path.is_empty() {
            goal.path = default_goal_path(&goal.title, None);
        }
        let abs = self.vault_root.join(&goal.path);
        if abs.exists() {
            return Err(GoalError::AlreadyExists(goal.path.clone()));
        }
        write_goal(&self.vault_root, &mut goal, false)
            .map_err(|e| GoalError::Io(format!("write: {e}")))?;
        self.publish(crate::service::GoalEvent::Upserted(goal.clone()));
        Ok(goal)
    }

    fn update(&self, goal: Goal) -> Result<Goal, GoalError> {
        let existing = self
            .list_inner()?
            .into_iter()
            .find(|g| g.id == goal.id)
            .ok_or_else(|| GoalError::NotFound(goal.id.to_string()))?;
        let mut next = goal;
        next.path = existing.path;
        next.date_created = existing.date_created.or(next.date_created);
        next.date_modified = Some(Utc::now());
        write_goal(&self.vault_root, &mut next, true)
            .map_err(|e| GoalError::Io(format!("write: {e}")))?;
        self.publish(crate::service::GoalEvent::Upserted(next.clone()));
        Ok(next)
    }

    fn rename(&self, id: Uuid, new_path: &str) -> Result<Goal, GoalError> {
        if new_path.is_empty() || new_path.contains("..") || new_path.starts_with('/') {
            return Err(GoalError::BadRequest(format!("bad path: {new_path}")));
        }
        let mut g = self
            .list_inner()?
            .into_iter()
            .find(|g| g.id == id)
            .ok_or_else(|| GoalError::NotFound(id.to_string()))?;
        if self.vault_root.join(new_path).exists() {
            return Err(GoalError::AlreadyExists(new_path.to_owned()));
        }
        // Through the vault's write path, like every other page mutation
        // here (`project.vault.write-path`).
        vault::move_page_at(&self.vault_root, &g.path, new_path)
            .map_err(|e| GoalError::Io(format!("rename: {e}")))?;
        g.path = new_path.to_owned();
        g.date_modified = Some(Utc::now());
        write_goal(&self.vault_root, &mut g, true)
            .map_err(|e| GoalError::Io(format!("write: {e}")))?;
        self.publish(crate::service::GoalEvent::Upserted(g.clone()));
        Ok(g)
    }

    fn delete(&self, id: Uuid) -> Result<(), GoalError> {
        let all = self.list_inner()?;
        let g = all
            .iter()
            .find(|g| g.id == id)
            .ok_or_else(|| GoalError::NotFound(id.to_string()))?;
        if let Some(child) = all.iter().find(|c| c.parent_id == Some(id)) {
            return Err(GoalError::BadRequest(format!(
                "{} is parent of {}; reparent or delete children first",
                g.title, child.title
            )));
        }
        vault::delete_page_at(&self.vault_root, &g.path)
            .map_err(|e| GoalError::Io(format!("remove {}: {e}", g.path)))?;
        self.publish(crate::service::GoalEvent::Deleted(id));
        Ok(())
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// `GoalService` impl above, on every successful mutation.
#[cfg(feature = "vox")]
impl crate::service::GoalServiceStreamSource for GoalBackend {
    fn events_hub(&self) -> &architect::PubSub<crate::service::GoalEvent> {
        &self.events
    }
}
