//! Versions a human named — Named Versions and Project Versions.
//!
//! Separate from [`super::version`] because the distinction is real:
//! that lane is what *happened* to a tree, this one is what someone
//! decided was worth pointing at. Curated versions are Vault entities
//! referencing `(root, change)` rather than store constructs, and naming
//! one exempts it from retention collection — which is what makes them
//! immortal where ordinary checkpoints are not.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::{ProjectVersionId, RootId, VersionId};
use crate::model::{NamedVersion, ProjectVersion, RestartMode};
use crate::path::RootPath;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum CurationEvent {
    VersionNamed(NamedVersion),
    /// Curation dropped. Carries the removed entity so a subscriber can
    /// drop it by id without a refetch.
    VersionUnnamed(NamedVersion),
    ProjectVersionStarted(ProjectVersion),
    ProjectVersionRestarted(ProjectVersion),
}

#[architect::rpc]
pub trait CurationService {
    /// Name a version. Any version can be named after the fact — the
    /// interesting state is usually only recognised later.
    async fn name_version(
        &self,
        root_id: RootId,
        version: VersionId,
        name: String,
    ) -> Result<NamedVersion, FilesFault>;

    /// Drop a name. The underlying version stays; it merely loses its
    /// exemption from retention.
    async fn unname_version(&self, root_id: RootId, version: VersionId)
    -> Result<NamedVersion, FilesFault>;

    /// Named versions of a root, or of one path within it.
    async fn named_versions(
        &self,
        root_id: RootId,
        path: Option<RootPath>,
    ) -> Result<Vec<NamedVersion>, FilesFault>;

    /// Resolve a name to the version it points at.
    async fn resolve_name(&self, root_id: RootId, name: String)
    -> Result<NamedVersion, FilesFault>;

    /// Begin a new Project Version — a named lineage of the whole tree,
    /// as distinct from a version of one file.
    async fn start_project_version(
        &self,
        root_id: RootId,
        name: String,
    ) -> Result<ProjectVersion, FilesFault>;

    /// A root's project versions, newest first.
    async fn project_versions(&self, root_id: RootId)
    -> Result<Vec<ProjectVersion>, FilesFault>;

    /// Restart a project version — begin again from its start, or from
    /// a chosen point, without losing what was there.
    async fn restart_project_version(
        &self,
        root_id: RootId,
        project_version: ProjectVersionId,
        mode: RestartMode,
    ) -> Result<ProjectVersion, FilesFault>;
}
