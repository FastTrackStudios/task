//! Server-side [`ProjectService`] backend.
//!
//! Walks the configured vault root on each call (cheap — the
//! vault page index lives in memory once `Vault::open` runs).
//! `ProjectBackend` is what the task-server mounts under
//! `/org/<slug>/vox`; the architect-rpc macro emits a sync
//! shim, so the server-bridge can call this directly even
//! though the trait surface is sync.
//!
//! Cheap to `Clone` — the inner [`std::path::PathBuf`] is
//! reused; each request re-opens the vault. Future
//! optimization: cache the parsed list with an mtime check.

use std::path::{Path, PathBuf};

use chrono::Utc;
use uuid::Uuid;
use vault::Vault;

use crate::model::ProjectInfo;
use crate::parts::{
    Audience, Component, Conflict, Deliverable, DeliverableItem, Divergence, Merged, Part, Piece,
    Scope,
};
use crate::scan::scan_vault;
use crate::service::{ProjectError, ProjectService};
use crate::write::{default_project_path, write_project};

/// File-backed `ProjectService` impl. Built once at server
/// boot per org, cloned into the vox bridge.
#[derive(Clone, architect::HasDispatcher)]
pub struct ProjectBackend {
    vault_root: PathBuf,
    /// Fan-out hub behind the `#[subscribe] fn events` stream —
    /// every successful mutation publishes the post-write state
    /// here ([`ProjectEvent::Upserted`] / [`ProjectEvent::Deleted`]).
    /// Sliding mailbox: a slow subscriber loses its *oldest* queued
    /// events, which is correct for state-shaped payloads. Clones
    /// share the hub (it's `Arc` inside), so the service mount and
    /// the stream mount can each hold a backend clone.
    #[cfg(feature = "vox")]
    events: architect::PubSub<crate::service::ProjectEvent>,
}

// Manual impl: `PubSub` carries no `Debug`.
impl std::fmt::Debug for ProjectBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProjectBackend")
            .field("vault_root", &self.vault_root)
            .finish_non_exhaustive()
    }
}

impl ProjectBackend {
    #[must_use]
    pub fn new(vault_root: impl Into<PathBuf>) -> Self {
        Self {
            vault_root: vault_root.into(),
            #[cfg(feature = "vox")]
            events: architect::PubSub::sliding(256),
        }
    }

    /// Publish a project change to every `events` subscriber. Call
    /// only after the write succeeded — subscribers fold these into
    /// state fetched via `list()`, so a phantom event would desync
    /// them. No-op without the `vox` feature (no wire, no
    /// subscribers).
    fn publish(&self, event: crate::service::ProjectEvent) {
        #[cfg(feature = "vox")]
        self.events.publish(event);
        #[cfg(not(feature = "vox"))]
        let _ = event;
    }

    /// Vault root this backend reads from.
    #[must_use]
    pub fn vault_root(&self) -> &Path {
        &self.vault_root
    }

    /// Write a project back to its page and announce it.
    ///
    /// The part verbs all have the same shape — read the page, change
    /// one list, save — and routing them through `update` would carry
    /// the caller's whole `ProjectInfo` when what changed is one field.
    fn save(&self, mut project: ProjectInfo) -> Result<ProjectInfo, ProjectError> {
        project.date_modified = Some(Utc::now());
        write_project(&self.vault_root, &mut project, true)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(project.clone()));
        Ok(project)
    }

    fn list_inner(&self) -> Result<Vec<ProjectInfo>, ProjectError> {
        let vault = Vault::open(&self.vault_root).map_err(|e| {
            ProjectError::Io(format!("open vault {}: {e}", self.vault_root.display()))
        })?;
        scan_vault(&vault).map_err(|e| ProjectError::Io(format!("scan: {e}")))
    }
}

impl ProjectService for ProjectBackend {
    fn list(&self) -> Result<Vec<ProjectInfo>, ProjectError> {
        // Aliases are not projects. They still resolve through `get` —
        // that is the whole point of keeping them — but a merged-away
        // half appearing in every listing is the duplication the merge
        // was performed to end.
        let mut all = self.list_inner()?;
        all.retain(|p| alias_target(p).is_none());
        Ok(all)
    }

    // t[impl project.lifecycle.merge-identity] — a former identity
    // resolves to the merged project rather than dangling
    fn get(&self, id: Uuid) -> Result<ProjectInfo, ProjectError> {
        let all = self.list_inner()?;
        let mut at = id;
        // Bounded, because a merge chain is a chain someone could make
        // circular by editing two pages in Obsidian. Following forever
        // would hang the lane; stopping says which id could not settle.
        for _ in 0..MERGE_HOPS {
            let found = all
                .iter()
                .find(|p| p.id == at)
                .ok_or_else(|| ProjectError::NotFound(at.to_string()))?;
            match alias_target(found) {
                Some(next) => at = next,
                None => return Ok(found.clone()),
            }
        }
        Err(ProjectError::BadRequest(format!(
            "{id} is in a merge chain that does not settle"
        )))
    }

    fn get_by_path(&self, path: &str) -> Result<ProjectInfo, ProjectError> {
        self.list_inner()?
            .into_iter()
            .find(|p| p.path == path)
            .ok_or_else(|| ProjectError::NotFound(path.to_owned()))
    }

    fn create(&self, mut project: ProjectInfo) -> Result<ProjectInfo, ProjectError> {
        if project.title.trim().is_empty() {
            return Err(ProjectError::BadRequest("title is required".into()));
        }
        if project.id.is_nil() {
            project.id = Uuid::new_v4();
        }
        if project.path.is_empty() {
            project.path = default_project_path(&project.title);
        }
        let now = Utc::now();
        if project.date_created.is_none() {
            project.date_created = Some(now);
        }
        project.date_modified = Some(now);

        let abs = self.vault_root.join(&project.path);
        if abs.exists() {
            return Err(ProjectError::AlreadyExists(project.path.clone()));
        }
        write_project(&self.vault_root, &mut project, false)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(project.clone()));
        Ok(project)
    }

    fn update(&self, project: ProjectInfo) -> Result<ProjectInfo, ProjectError> {
        let existing = self
            .list_inner()?
            .into_iter()
            .find(|p| p.id == project.id)
            .ok_or_else(|| ProjectError::NotFound(project.id.to_string()))?;
        // Carry the on-disk path forward — caller cannot
        // smuggle a rename through `update`.
        let mut next = project;
        next.path = existing.path;
        next.date_created = existing.date_created.or(next.date_created);
        next.date_modified = Some(Utc::now());
        write_project(&self.vault_root, &mut next, true)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(next.clone()));
        Ok(next)
    }

    fn rename(&self, id: Uuid, new_path: &str) -> Result<ProjectInfo, ProjectError> {
        if new_path.is_empty() || new_path.contains("..") || new_path.starts_with('/') {
            return Err(ProjectError::BadRequest(format!("bad path: {new_path}")));
        }
        let mut p = self
            .list_inner()?
            .into_iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ProjectError::NotFound(id.to_string()))?;
        if self.vault_root.join(new_path).exists() {
            return Err(ProjectError::AlreadyExists(new_path.to_owned()));
        }
        // Through the vault's write path, like every other page mutation
        // here (`project.vault.write-path`).
        vault::move_page_at(&self.vault_root, &p.path, new_path)
            .map_err(|e| ProjectError::Io(format!("rename: {e}")))?;
        p.path = new_path.to_owned();
        p.date_modified = Some(Utc::now());
        // Re-serialize so frontmatter mtime tracks the move.
        write_project(&self.vault_root, &mut p, true)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(p.clone()));
        Ok(p)
    }

    fn parts(&self, project: Uuid) -> Result<Vec<Part>, ProjectError> {
        Ok(self.get(project)?.parts.0)
    }

    fn add_part(&self, project: Uuid, name: &str) -> Result<Part, ProjectError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProjectError::BadRequest("a part needs a name".into()));
        }
        let mut p = self.get(project)?;
        if p.parts.has_name(name) {
            return Err(ProjectError::AlreadyExists(format!(
                "{} already has a part called {name}",
                p.title
            )));
        }
        // An id now, not at promotion time — see `project_proto::parts`.
        let part = Part {
            id: Uuid::new_v4(),
            name: name.to_owned(),
            references: None,
            components: Vec::new(),
        };
        p.parts.0.push(part.clone());
        self.save(p)?;
        Ok(part)
    }

    fn rename_part(&self, project: Uuid, part: Uuid, name: &str) -> Result<Part, ProjectError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ProjectError::BadRequest("a part needs a name".into()));
        }
        let mut p = self.get(project)?;
        // A rename onto a name another part holds is the same collision
        // `add_part` refuses; renaming a part to what it already is, is
        // not.
        if p.parts
            .0
            .iter()
            .any(|x| x.id != part && x.name.eq_ignore_ascii_case(name))
        {
            return Err(ProjectError::AlreadyExists(format!(
                "{} already has a part called {name}",
                p.title
            )));
        }
        let found = p
            .parts
            .0
            .iter_mut()
            .find(|x| x.id == part)
            .ok_or_else(|| ProjectError::NotFound(part.to_string()))?;
        found.name = name.to_owned();
        let renamed = found.clone();
        self.save(p)?;
        Ok(renamed)
    }

    fn remove_part(&self, project: Uuid, part: Uuid) -> Result<(), ProjectError> {
        let mut p = self.get(project)?;
        let before = p.parts.len();
        p.parts.0.retain(|x| x.id != part);
        if p.parts.len() == before {
            return Err(ProjectError::NotFound(part.to_string()));
        }
        self.save(p)?;
        Ok(())
    }

    // t[impl project.part.listing] — one list, the roster's order, with
    // promoted-ness read off the pages rather than stored beside the
    // roster: one question, one answer, from the place that declares it
    fn pieces(&self, project: Uuid) -> Result<Vec<Piece>, ProjectError> {
        let all = self.list_inner()?;
        let parent = all
            .iter()
            .find(|p| p.id == project)
            .ok_or_else(|| ProjectError::NotFound(project.to_string()))?;
        // Promoted-ness is read off the pages, not off the roster: one
        // question, one answer, from the place that declares it.
        Ok(parent
            .parts
            .0
            .iter()
            .map(|part| Piece {
                id: part.id,
                name: part.name.clone(),
                promoted: all.iter().any(|p| p.id == part.id),
            })
            .collect())
    }

    // t[impl project.part.promotion] — the part's own id becomes the
    // project's id, so there is no mapping table and nothing referencing
    // the piece has to be found and rewritten
    // t[impl project.identity.stable] — identity survives promotion by
    // being the same identity
    fn promote_part(&self, project: Uuid, part: Uuid) -> Result<ProjectInfo, ProjectError> {
        let all = self.list_inner()?;
        let parent = all
            .iter()
            .find(|p| p.id == project)
            .ok_or_else(|| ProjectError::NotFound(project.to_string()))?;
        let named = parent
            .parts
            .get(part)
            .ok_or_else(|| ProjectError::NotFound(part.to_string()))?
            .clone();
        if all.iter().any(|p| p.id == part) {
            return Err(ProjectError::AlreadyExists(format!(
                "{} is already a project",
                named.name
            )));
        }

        // The part's own id becomes the project's id. That is the whole
        // mechanism `project.part.promotion` rests on — no mapping
        // table, so nothing referencing this piece has to be found and
        // rewritten, and nothing breaks on a machine holding half the
        // project.
        let mut promoted = ProjectInfo {
            id: named.id,
            parent_id: Some(project),
            title: named.name.clone(),
            // Inherited, because a song of a music-production album is
            // music production. `project.capability.mutable` is how it
            // becomes something else.
            capabilities: parent.capabilities.clone(),
            // Everything else is a project with nothing said about it:
            // a song is not "active since March at £90/hour" because its
            // album is.
            ..ProjectInfo::default()
        };
        promoted.path = default_project_path(&named.name);
        if self.vault_root.join(&promoted.path).exists() {
            return Err(ProjectError::AlreadyExists(promoted.path));
        }
        let now = Utc::now();
        promoted.date_created = Some(now);
        promoted.date_modified = Some(now);
        write_project(&self.vault_root, &mut promoted, false)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        // The roster is not touched — see `project.part.listing`. The
        // album still lists ten songs, in the same order.
        self.publish(crate::service::ProjectEvent::Upserted(promoted.clone()));
        Ok(promoted)
    }

    // t[impl project.part.demotable] — refuses only what a part cannot
    // hold, and names it. Content is not an obstacle: a part is
    // addressable and carries exactly that
    fn demote_project(&self, project: Uuid) -> Result<Part, ProjectError> {
        let all = self.list_inner()?;
        let subproject = all
            .iter()
            .find(|p| p.id == project)
            .ok_or_else(|| ProjectError::NotFound(project.to_string()))?;
        let Some(parent_id) = subproject.parent_id else {
            return Err(ProjectError::BadRequest(format!(
                "{} has no parent, so there is nothing for it to be a part of",
                subproject.title
            )));
        };
        // A part cannot have subprojects, so a subproject that has them
        // cannot become one. Named, rather than refused vaguely: the
        // caller's next move is to deal with that child.
        if let Some(child) = all.iter().find(|c| c.parent_id == Some(project)) {
            return Err(ProjectError::BadRequest(format!(
                "{} has a subproject ({}), and a part cannot; \
                 reparent or demote it first",
                subproject.title, child.title
            )));
        }
        let parent = all
            .iter()
            .find(|p| p.id == parent_id)
            .ok_or_else(|| ProjectError::NotFound(parent_id.to_string()))?;

        // The roster entry it was promoted from, still where it was.
        // Absent only if someone removed it while this was a project,
        // in which case demotion puts it back at the end rather than
        // failing — the piece existing matters more than its position,
        // and refusing would leave a project nobody can un-promote.
        let part = parent.parts.get(project).cloned().unwrap_or(Part {
            id: project,
            name: subproject.title.clone(),
            references: None,
            components: Vec::new(),
        });
        if parent.parts.get(project).is_none() {
            let mut parent = parent.clone();
            parent.parts.0.push(part.clone());
            self.save(parent)?;
        }

        vault::delete_page_at(&self.vault_root, &subproject.path)
            .map_err(|e| ProjectError::Io(format!("remove {}: {e}", subproject.path)))?;
        self.publish(crate::service::ProjectEvent::Deleted(project));
        Ok(part)
    }

    // t[impl project.form.grammar] — reports, never refuses: a studio's
    // real tree diverges from every grammar somebody writes for it
    fn divergences(&self, project: Uuid) -> Result<Vec<Divergence>, ProjectError> {
        let p = self.get(project)?;
        Ok(project_proto::parts::divergences(p.form, &p.parts))
    }

    // t[impl project.form.components] — the component lands on the roster
    // entry, so promotion (which does not touch the roster) cannot
    // disturb it
    fn attach_component(
        &self,
        project: Uuid,
        part: Uuid,
        component: Component,
    ) -> Result<Part, ProjectError> {
        let name = component.name.trim().to_owned();
        if name.is_empty() {
            return Err(ProjectError::BadRequest("a component needs a name".into()));
        }
        let mut p = self.get(project)?;
        let found = p
            .parts
            .0
            .iter_mut()
            .find(|x| x.id == part)
            .ok_or_else(|| ProjectError::NotFound(part.to_string()))?;
        if found
            .components
            .iter()
            .any(|c| c.name.eq_ignore_ascii_case(&name))
        {
            return Err(ProjectError::AlreadyExists(name));
        }
        found.components.push(Component { name, ..component });
        let updated = found.clone();
        self.save(p)?;
        Ok(updated)
    }

    fn detach_component(
        &self,
        project: Uuid,
        part: Uuid,
        name: &str,
    ) -> Result<Part, ProjectError> {
        let mut p = self.get(project)?;
        let found = p
            .parts
            .0
            .iter_mut()
            .find(|x| x.id == part)
            .ok_or_else(|| ProjectError::NotFound(part.to_string()))?;
        let before = found.components.len();
        found
            .components
            .retain(|c| !c.name.eq_ignore_ascii_case(name));
        if found.components.len() == before {
            return Err(ProjectError::NotFound(name.to_owned()));
        }
        let updated = found.clone();
        self.save(p)?;
        Ok(updated)
    }

    fn deliverables(&self, project: Uuid) -> Result<Vec<Deliverable>, ProjectError> {
        Ok(self.get(project)?.deliverables.0)
    }

    fn declare_deliverable(
        &self,
        project: Uuid,
        mut deliverable: Deliverable,
    ) -> Result<Deliverable, ProjectError> {
        let name = deliverable.name.trim().to_owned();
        if name.is_empty() {
            return Err(ProjectError::BadRequest(
                "a deliverable needs a name".into(),
            ));
        }
        deliverable.name = name;
        let mut p = self.get(project)?;
        if p.deliverables.has_name(&deliverable.name) {
            return Err(ProjectError::AlreadyExists(format!(
                "{} already owes something called {}",
                p.title, deliverable.name
            )));
        }
        if deliverable.id.is_nil() {
            deliverable.id = Uuid::new_v4();
        }
        p.deliverables.0.push(deliverable.clone());
        self.save(p)?;
        Ok(deliverable)
    }

    fn withdraw_deliverable(&self, project: Uuid, deliverable: Uuid) -> Result<(), ProjectError> {
        let mut p = self.get(project)?;
        let before = p.deliverables.len();
        p.deliverables.0.retain(|d| d.id != deliverable);
        if p.deliverables.len() == before {
            return Err(ProjectError::NotFound(deliverable.to_string()));
        }
        self.save(p)?;
        Ok(())
    }

    // t[impl project.deliverable.scope] — the expansion, derived on read.
    // Storing it would make "stays in step" a job somebody has to
    // remember to run, and the failure would be an album quietly owing
    // ten deliverables after growing an eleventh song
    fn deliverable_items(&self, project: Uuid) -> Result<Vec<DeliverableItem>, ProjectError> {
        let pieces = self.pieces(project)?;
        let declared = self.get(project)?;
        Ok(expand(&declared, &pieces))
    }

    // t[impl project.deliverable.client-view] — the filter is the surface,
    // not a parameter: there is nothing to pass, so nothing to pass wrong
    fn client_deliverables(&self, project: Uuid) -> Result<Vec<DeliverableItem>, ProjectError> {
        let mut items = self.deliverable_items(project)?;
        items.retain(|i| i.audience > Audience::Internal);
        // Organised by scope then medium, which is what the rule asks a
        // client view to be: the whole performance, then a song.
        items.sort_by(|a, b| {
            a.part
                .is_some()
                .cmp(&b.part.is_some())
                .then_with(|| format!("{:?}", a.medium).cmp(&format!("{:?}", b.medium)))
        });
        Ok(items)
    }

    // t[impl project.identity.adoption] — one page written inside a tree
    // that is not moved, copied or renamed. The applications writing to
    // it never notice, which is the entire requirement
    fn adopt(&self, dir: &str, title: &str) -> Result<ProjectInfo, ProjectError> {
        if dir.is_empty() || dir.contains("..") || dir.starts_with('/') {
            return Err(ProjectError::BadRequest(format!("bad directory: {dir}")));
        }
        let abs = self.vault_root.join(dir);
        if !abs.is_dir() {
            return Err(ProjectError::NotFound(format!(
                "{dir} is not a directory in this vault"
            )));
        }

        // Already adopted? Come back as what it is. Adoption is
        // something a person may reasonably do twice — having forgotten,
        // or after a scan proposed it again — and the second time must
        // not produce a second project over one tree.
        let page = format!("{}/project.md", dir.trim_end_matches('/'));
        if let Ok(existing) = self.get_by_path(&page) {
            return Ok(existing);
        }

        let title = if title.trim().is_empty() {
            // The directory's own name, which is what a person called
            // it when they made it.
            std::path::Path::new(dir)
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or(dir)
                .to_owned()
        } else {
            title.trim().to_owned()
        };

        let now = Utc::now();
        let mut adopted = ProjectInfo {
            id: Uuid::new_v4(),
            path: page,
            title,
            date_created: Some(now),
            date_modified: Some(now),
            ..ProjectInfo::default()
        };
        // Nothing is moved: the page is written *into* the tree, beside
        // whatever was already there.
        write_project(&self.vault_root, &mut adopted, false)
            .map_err(|e| ProjectError::Io(format!("write: {e}")))?;
        self.publish(crate::service::ProjectEvent::Upserted(adopted.clone()));
        Ok(adopted)
    }

    // t[impl project.setlist.source] — by reference: the roster entry
    // holds the referenced piece's id, and the project owning that piece
    // is not touched. Reordering is reordering this list
    fn set_setlist(&self, project: Uuid, songs: Vec<Uuid>) -> Result<Vec<Piece>, ProjectError> {
        let all = self.list_inner()?;
        let mut setlist = all
            .iter()
            .find(|p| p.id == project)
            .cloned()
            .ok_or_else(|| ProjectError::NotFound(project.to_string()))?;

        let mut roster = Vec::with_capacity(songs.len());
        for song in songs {
            // Resolve the reference now so a setlist cannot be built out
            // of ids that name nothing — a performance is a bad moment
            // to discover a song was a typo.
            let named = all
                .iter()
                .find_map(|p| p.parts.get(song).map(|part| part.name.clone()))
                .or_else(|| {
                    // A promoted song is a project, and its name is its
                    // title. `project.setlist.source` says a setlist
                    // references songs "promoted or not", so both.
                    all.iter().find(|p| p.id == song).map(|p| p.title.clone())
                })
                .ok_or_else(|| ProjectError::NotFound(song.to_string()))?;
            roster.push(Part {
                // A setlist entry has its own id — it is a position in
                // this performance, not the song. Two setlists holding
                // one song hold two entries, and reordering one does not
                // reorder the other.
                id: Uuid::new_v4(),
                name: named,
                references: Some(song),
                components: Vec::new(),
            });
        }
        setlist.parts = crate::Parts(roster);
        let saved = self.save(setlist)?;
        Ok(saved
            .parts
            .0
            .iter()
            .map(|part| Piece {
                id: part.id,
                name: part.name.clone(),
                promoted: false,
            })
            .collect())
    }

    fn setlist(&self, project: Uuid) -> Result<Vec<Piece>, ProjectError> {
        let all = self.list_inner()?;
        let setlist = all
            .iter()
            .find(|p| p.id == project)
            .ok_or_else(|| ProjectError::NotFound(project.to_string()))?;
        Ok(setlist
            .parts
            .0
            .iter()
            .filter(|part| part.references.is_some())
            .map(|part| Piece {
                id: part.references.unwrap_or(part.id),
                name: part.name.clone(),
                // Whether the referenced song has a page of its own.
                promoted: part
                    .references
                    .is_some_and(|r| all.iter().any(|p| p.id == r)),
            })
            .collect())
    }

    // t[impl project.lifecycle.merge] — capabilities union, parts and
    // deliverables combine, and every disagreement comes back as a
    // `Conflict` with both values rather than being resolved quietly
    // t[impl project.lifecycle.merge-identity] — the absorbed id keeps
    // answering, because its page becomes an alias rather than being
    // deleted
    fn merge(&self, into: Uuid, absorbed: Uuid) -> Result<Merged, ProjectError> {
        if into == absorbed {
            return Err(ProjectError::BadRequest(
                "a project cannot absorb itself".into(),
            ));
        }
        let all = self.list_inner()?;
        let mut keep = all
            .iter()
            .find(|p| p.id == into)
            .cloned()
            .ok_or_else(|| ProjectError::NotFound(into.to_string()))?;
        let gone = all
            .iter()
            .find(|p| p.id == absorbed)
            .cloned()
            .ok_or_else(|| ProjectError::NotFound(absorbed.to_string()))?;

        let mut conflicts = Vec::new();

        // Titles. Both halves named the job, and neither name is wrong.
        if !gone.title.eq_ignore_ascii_case(&keep.title) {
            conflicts.push(Conflict {
                field: "title".into(),
                kept: keep.title.clone(),
                absorbed: gone.title.clone(),
            });
        }
        // Form. `None` on either side is not a disagreement — one half
        // simply did not say, and the half that did is the answer.
        match (keep.form, gone.form) {
            (Some(a), Some(b)) if a != b => conflicts.push(Conflict {
                field: "form".into(),
                kept: a.to_string(),
                absorbed: b.to_string(),
            }),
            (None, Some(b)) => keep.form = Some(b),
            _ => {}
        }

        // Capabilities union. A concert that was recorded and filmed
        // holds both, and that is the rule's own example of normal.
        let mut held = keep.capabilities.held.clone();
        for c in &gone.capabilities.held {
            if !held.contains(c) {
                held.push(*c);
            }
        }
        keep.capabilities.held = held;

        // Parts. Same id is the same piece — two halves of one job that
        // already agreed. Same *name* under different ids is the case
        // the rule means by "the identity of a part": both are kept, and
        // a human decides whether they are one song.
        for part in &gone.parts.0 {
            if keep.parts.get(part.id).is_some() {
                continue;
            }
            if keep.parts.has_name(&part.name) {
                conflicts.push(Conflict {
                    field: format!("part:{}", part.name),
                    kept: "already named here under a different id".into(),
                    absorbed: part.id.to_string(),
                });
            }
            keep.parts.0.push(part.clone());
        }

        // Deliverables, on the same terms.
        for d in &gone.deliverables.0 {
            if keep.deliverables.get(d.id).is_some() {
                continue;
            }
            if keep.deliverables.has_name(&d.name) {
                conflicts.push(Conflict {
                    field: format!("deliverable:{}", d.name),
                    kept: "already declared here under a different id".into(),
                    absorbed: d.id.to_string(),
                });
                continue;
            }
            keep.deliverables.0.push(d.clone());
        }

        // Anything parented to the absorbed project is now parented
        // here. Without this its subprojects would point at an alias,
        // and every listing that walks parentage would lose them.
        for child in all.iter().filter(|c| c.parent_id == Some(absorbed)) {
            let mut child = child.clone();
            child.parent_id = Some(into);
            self.save(child)?;
        }

        self.save(keep)?;

        // The absorbed page stops being a project and becomes an alias.
        // Not deleted: `project.lifecycle.merge-identity` needs its id to
        // keep answering for links, tasks, time and a share link already
        // in somebody's hands.
        let mut alias = gone.clone();
        alias.same_as = Some(into.to_string());
        // "The merge records what it absorbed so the history stays
        // legible to someone who only knew one half."
        alias.details = format!(
            "{}\n\nMerged into {into}. This page is an alias: the id above still \
             resolves, and resolves to the merged project.\n",
            gone.details.trim_end()
        );
        self.save(alias)?;

        Ok(Merged {
            project: into,
            absorbed,
            conflicts,
        })
    }

    fn delete(&self, id: Uuid) -> Result<(), ProjectError> {
        let all = self.list_inner()?;
        let p = all
            .iter()
            .find(|p| p.id == id)
            .ok_or_else(|| ProjectError::NotFound(id.to_string()))?;
        // Refuse if any other project parents off this one —
        // orphans here would silently float to the root in the
        // UI and quietly break agent reasoning.
        if let Some(child) = all.iter().find(|c| c.parent_id == Some(id)) {
            return Err(ProjectError::BadRequest(format!(
                "{} is parent of {}; reparent or delete children first",
                p.title, child.title
            )));
        }
        // Through the vault's write path, like `write_project` — a bare
        // `remove_file` here was the last delete that bypassed it.
        crate::write::delete_project(&self.vault_root, &p.path)
            .map_err(|e| ProjectError::Io(format!("remove {}: {e}", p.path)))?;
        self.publish(crate::service::ProjectEvent::Deleted(id));
        Ok(())
    }
}

/// Expand a project's declarations against its pieces.
///
/// A whole-project declaration is one item; a per-part one is an item
/// per piece, in the roster's order; an excerpt expands to nothing,
/// because an excerpt is picked rather than derived.
///
/// Promotion is not consulted, and that is the point — `project.deliverable.scope`
/// says per-part deliverables are "unaffected by whether a part is
/// promoted", and the pieces list already made that free.
fn expand(project: &ProjectInfo, pieces: &[Piece]) -> Vec<DeliverableItem> {
    let mut out = Vec::new();
    for d in &project.deliverables.0 {
        match d.scope {
            Scope::WholeProject => out.push(DeliverableItem {
                deliverable: d.id,
                name: d.name.clone(),
                medium: d.medium,
                audience: d.audience,
                part: None,
                title: d.name.clone(),
            }),
            Scope::PerPart => out.extend(pieces.iter().map(|piece| DeliverableItem {
                deliverable: d.id,
                name: d.name.clone(),
                medium: d.medium,
                audience: d.audience,
                part: Some(piece.id),
                title: piece.name.clone(),
            })),
            // Nothing to derive. An excerpt exists once somebody chooses
            // one, which is a binding this lane does not yet have — see
            // `project.deliverable.binding` in the spec.
            Scope::Excerpt => {}
        }
    }
    out
}

/// How many merges deep a chain may be before it is called circular.
///
/// Generous: a project absorbed into one that was itself absorbed is an
/// ordinary sequence of events over a year, and each hop is a page read
/// from a list already in memory.
const MERGE_HOPS: usize = 16;

/// The project this page is an alias for, if it is one.
///
/// `same_as` predates merge and was parsed by the entity reader and used
/// by nothing. It means "this row is a reference to the canonical
/// project", which is exactly what a merged-away half is — so merge
/// gives the field the meaning its own doc comment always claimed.
///
/// A non-uuid value is not an alias. `same_as` also holds the
/// `@org/slug` federation form, which points somewhere this lane cannot
/// follow; treating it as a local alias would make a federated reference
/// resolve to `NotFound` instead of to itself.
fn alias_target(project: &ProjectInfo) -> Option<Uuid> {
    project
        .same_as
        .as_deref()
        .and_then(|s| Uuid::parse_str(s.trim()).ok())
}

/// The `#[subscribe]` backend contract: hand the emitted stream host
/// the hub it attaches subscriber sinks to. Publishing happens in the
/// `ProjectService` impl above, on every successful mutation.
#[cfg(feature = "vox")]
impl crate::service::ProjectServiceStreamSource for ProjectBackend {
    fn events_hub(&self) -> &architect::PubSub<crate::service::ProjectEvent> {
        &self.events
    }
}
