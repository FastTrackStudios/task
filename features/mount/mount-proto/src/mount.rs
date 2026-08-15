//! [`Mount`] — one project's content location on this machine.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::backend::BackendKind;

/// One project's content location, scoped to this machine.
///
/// Same `project_id` can resolve to wildly different rows on
/// two machines: a workstation might point a Filesystem mount
/// at `~/Projects/foo`, a phone might point a `VoxProxy` mount
/// at the home server, and a backup box might not have a mount
/// at all (project shows as `[remote]`).
///
/// Stored as a row in `~/.config/task/mounts.toml` by default;
/// SeaORM mirror emitted under `--features server` so a future
/// `task mount sync` can publish mounts across a user's
/// devices without re-implementing the TOML reader.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "mounts", repo)]
pub struct Mount {
    /// Project UUID — the federation-stable id assigned by the
    /// home org. PK so a project can have at most one mount
    /// per machine.
    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub project_id: Uuid,

    /// Which backend serves this mount. Drives `path` vs `url`
    /// interpretation.
    #[architect(json, filterable)]
    pub backend: BackendKind,

    /// Filesystem mounts: absolute local path
    /// (`/home/cody/Projects/foo`).
    /// Nextcloud / VoxProxy mounts: backend-relative path
    /// inside the remote (`Documents/Work/foo` /
    /// `<org-slug>/foo`).
    #[architect(filterable, sortable)]
    pub path: String,

    /// Remote URL for non-Filesystem backends. `None` for
    /// `Filesystem`.
    #[architect(filterable)]
    pub url: Option<String>,

    /// Optional human-facing label. Falls back to the project
    /// title at render time when empty.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub label: String,

    /// When this mount was first registered.
    #[architect(exclude(create, update), on_create = Utc::now(), filterable, sortable)]
    pub created_at: DateTime<Utc>,

    /// Last mutation (path edit, backend swap, etc.).
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now(), filterable, sortable)]
    pub updated_at: DateTime<Utc>,
}

impl Mount {
    /// Convenience constructor for the common case: a fresh
    /// Filesystem mount with no remote URL or label.
    #[must_use]
    pub fn filesystem(project_id: Uuid, path: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            project_id,
            backend: BackendKind::Filesystem,
            path: path.into(),
            url: None,
            label: String::new(),
            created_at: now,
            updated_at: now,
        }
    }
}
