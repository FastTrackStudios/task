//! Server-side [`MilestoneService`] backend. Walks the vault
//! on each call; resolves the owning project's slug when
//! `path` defaults need filling.

use std::path::{Path, PathBuf};

use chrono::Utc;
use uuid::Uuid;
use vault::Vault;

use crate::model::Milestone;
use crate::parse::{looks_like_milestone, parse_page};
use crate::service::{MilestoneError, MilestoneService};
use crate::write::{default_milestone_path, write_milestone};

#[derive(Clone, architect::HasDispatcher)]
pub struct MilestoneBackend {
    vault_root: PathBuf,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful mutation publishes the post-write state here
    /// (`MilestoneEvent::Upserted` / `MilestoneEvent::Deleted`).
    /// Sliding mailbox: a slow subscriber loses its *oldest* queued
    /// events, which is correct for state-shaped payloads. Clones
    /// share the hub (it's `Arc` inside).
    #[cfg(feature = "vox")]
    events: architect::PubSub<milestone_proto::MilestoneEvent>,
}

// Manual impl: `PubSub` carries no `Debug`.
impl std::fmt::Debug for MilestoneBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MilestoneBackend")
            .field("vault_root", &self.vault_root)
            .finish_non_exhaustive()
    }
}

impl MilestoneBackend {
    #[must_use]
    pub fn new(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            vault_root: vault_root.into(),
            #[cfg(feature = "vox")]
            events: architect::PubSub::sliding(256),
        }
    }

    /// Publish a milestone change to every `events` subscriber. Call
    /// only after the write succeeded — subscribers fold these into
    /// state fetched via `list()`, so a phantom event would desync
    /// them. No-op without the `vox` feature (no wire, no
    /// subscribers).
    fn publish(&self, event: milestone_proto::MilestoneEvent) {
        #[cfg(feature = "vox")]
        self.events.publish(event);
        #[cfg(not(feature = "vox"))]
        let _ = event;
    }

    #[must_use]
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    fn list_inner(&self) -> Result<Vec<Milestone>, MilestoneError> {
        let vault = Vault::open(&self.vault_root).map_err(|e| {
            MilestoneError::Io(format!("open vault {}: {e}", self.vault_root.display()))
        })?;
        let mut out = Vec::new();
        for page in &vault.pages {
            let proto = page.to_proto();
            if !looks_like_milestone(&proto) {
                continue;
            }
            match parse_page(&proto) {
                Ok(m) => out.push(m),
                Err(e) => tracing::warn!(path = %page.rel_path, ?e, "milestone parse failed"),
            }
        }
        Ok(out)
    }

    /// Resolve the owning project's vault-relative path
    /// (e.g. `Projects/Health/Health.md`). Used by `create`
    /// to derive the default milestone path. Walks the vault
    /// for the one project page whose frontmatter `id` matches.
    fn project_path(&self, project_id: Uuid) -> Result<String, MilestoneError> {
        let vault = Vault::open(&self.vault_root).map_err(|e| {
            MilestoneError::Io(format!("open vault {}: {e}", self.vault_root.display()))
        })?;
        for page in &vault.pages {
            let Some((map, _)) = vault_entity::frontmatter::mapping(&page.raw) else {
                continue;
            };
            if !looks_like_project(&map) {
                continue;
            }
            let id_match = vault_entity::yaml::str_at(&map, "id")
                .and_then(|s| Uuid::parse_str(&s).ok())
                == Some(project_id);
            if id_match {
                return Ok(page.rel_path.clone());
            }
        }
        Err(MilestoneError::BadRequest(format!(
            "no project with id {project_id} in this vault"
        )))
    }
}

/// Mirrors `project::looks_like_project`: `type:` or a tag, matched
/// case-insensitively because `type: Project` occurs in real vaults.
///
/// This used to be an exact `== Some("project")` on `type:` alone, so a
/// milestone could not find its owner on any page the project slice
/// itself happily recognised.
///
/// Duplicated rather than called: `milestone` does not depend on
/// `project`, and adding an edge to a peer slice's impl crate to reach
/// one predicate is the wrong trade. It collapses into one definition
/// when `project-proto` is extracted.
fn looks_like_project(map: &serde_yaml::Mapping) -> bool {
    const PROJECT: &str = "project";
    if vault_entity::yaml::str_at(map, "type").is_some_and(|t| t.eq_ignore_ascii_case(PROJECT)) {
        return true;
    }
    vault_entity::yaml::string_list_at(map, "tags")
        .iter()
        .any(|t| t.eq_ignore_ascii_case(PROJECT))
}

impl MilestoneService for MilestoneBackend {
    fn list(&self) -> Result<Vec<Milestone>, MilestoneError> {
        self.list_inner()
    }

    fn get(&self, id: Uuid) -> Result<Milestone, MilestoneError> {
        self.list_inner()?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| MilestoneError::NotFound(id.to_string()))
    }

    fn get_by_path(&self, path: &str) -> Result<Milestone, MilestoneError> {
        self.list_inner()?
            .into_iter()
            .find(|m| m.path == path)
            .ok_or_else(|| MilestoneError::NotFound(path.to_owned()))
    }

    fn create(&self, mut m: Milestone) -> Result<Milestone, MilestoneError> {
        if m.title.trim().is_empty() {
            return Err(MilestoneError::BadRequest("title is required".into()));
        }
        if m.project_id.is_nil() {
            return Err(MilestoneError::BadRequest("project_id is required".into()));
        }
        if m.id.is_nil() {
            m.id = Uuid::new_v4();
        }
        if m.path.is_empty() {
            let project_rel = self.project_path(m.project_id)?;
            m.path = default_milestone_path(&project_rel, &m.title);
        }
        let abs = self.vault_root.join(&m.path);
        if abs.exists() {
            return Err(MilestoneError::AlreadyExists(m.path.clone()));
        }
        write_milestone(&self.vault_root, &mut m, false)
            .map_err(|e| MilestoneError::Io(format!("write: {e}")))?;
        self.publish(milestone_proto::MilestoneEvent::Upserted(m.clone()));
        Ok(m)
    }

    fn update(&self, milestone: Milestone) -> Result<Milestone, MilestoneError> {
        let existing = self
            .list_inner()?
            .into_iter()
            .find(|m| m.id == milestone.id)
            .ok_or_else(|| MilestoneError::NotFound(milestone.id.to_string()))?;
        let mut next = milestone;
        next.path = existing.path;
        next.date_created = existing.date_created.or(next.date_created);
        next.date_modified = Some(Utc::now());
        write_milestone(&self.vault_root, &mut next, true)
            .map_err(|e| MilestoneError::Io(format!("write: {e}")))?;
        self.publish(milestone_proto::MilestoneEvent::Upserted(next.clone()));
        Ok(next)
    }

    fn rename(&self, id: Uuid, new_path: &str) -> Result<Milestone, MilestoneError> {
        if new_path.is_empty() || new_path.contains("..") || new_path.starts_with('/') {
            return Err(MilestoneError::BadRequest(format!("bad path: {new_path}")));
        }
        let mut m = self
            .list_inner()?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| MilestoneError::NotFound(id.to_string()))?;
        if self.vault_root.join(new_path).exists() {
            return Err(MilestoneError::AlreadyExists(new_path.to_owned()));
        }
        // Through the vault's write path, like every other page mutation
        // here (`project.vault.write-path`).
        vault::move_page_at(&self.vault_root, &m.path, new_path)
            .map_err(|e| MilestoneError::Io(format!("rename: {e}")))?;
        m.path = new_path.to_owned();
        m.date_modified = Some(Utc::now());
        write_milestone(&self.vault_root, &mut m, true)
            .map_err(|e| MilestoneError::Io(format!("write: {e}")))?;
        self.publish(milestone_proto::MilestoneEvent::Upserted(m.clone()));
        Ok(m)
    }

    fn delete(&self, id: Uuid) -> Result<(), MilestoneError> {
        let m = self
            .list_inner()?
            .into_iter()
            .find(|m| m.id == id)
            .ok_or_else(|| MilestoneError::NotFound(id.to_string()))?;
        // Refuse if any task points at this milestone — drop
        // the link first, otherwise downstream rollups would
        // silently float to the project root.
        let vault = Vault::open(&self.vault_root)
            .map_err(|e| MilestoneError::Io(format!("open vault: {e}")))?;
        for page in &vault.pages {
            let Some((map, _)) = vault_entity::frontmatter::mapping(&page.raw) else {
                continue;
            };
            if vault_entity::yaml::str_at(&map, "type").as_deref() != Some("task") {
                continue;
            }
            let linked = vault_entity::yaml::str_at(&map, "milestoneId")
                .and_then(|s| Uuid::parse_str(&s).ok())
                == Some(id);
            if linked {
                return Err(MilestoneError::BadRequest(format!(
                    "{} is referenced by task {}; unlink first",
                    m.title, page.rel_path
                )));
            }
        }
        vault::delete_page_at(&self.vault_root, &m.path)
            .map_err(|e| MilestoneError::Io(format!("remove {}: {e}", m.path)))?;
        self.publish(milestone_proto::MilestoneEvent::Deleted(id));
        Ok(())
    }
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// `MilestoneService` impl above, on every successful mutation.
#[cfg(feature = "vox")]
impl milestone_proto::MilestoneServiceStreamSource for MilestoneBackend {
    fn events_hub(&self) -> &architect::PubSub<milestone_proto::MilestoneEvent> {
        &self.events
    }
}

#[cfg(test)]
mod tests {
    use super::looks_like_project;

    fn map(src: &str) -> serde_yaml::Mapping {
        serde_yaml::from_str(src).unwrap()
    }

    /// `type: Project` occurs in real vaults and the project slice
    /// accepts it. An exact match here meant a milestone could not find
    /// an owner the rest of the system could see.
    #[test]
    fn project_type_is_case_insensitive() {
        assert!(looks_like_project(&map("type: project")));
        assert!(looks_like_project(&map("type: Project")));
        assert!(looks_like_project(&map("type: PROJECT")));
    }

    /// The tag form counts too, same as `project::looks_like_project`.
    #[test]
    fn the_tag_form_counts() {
        assert!(looks_like_project(&map("tags:\n  - Project")));
        assert!(looks_like_project(&map("tags: project")));
    }

    #[test]
    fn other_pages_are_not_projects() {
        assert!(!looks_like_project(&map("type: task")));
        assert!(!looks_like_project(&map("tags:\n  - milestone")));
        assert!(!looks_like_project(&map("name: nope")));
    }
}
