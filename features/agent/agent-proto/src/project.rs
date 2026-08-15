//! Project — a working-directory root that sessions belong
//! to. Mirrors `CodexMonitor`'s `WorkspaceInfo` + Hermes's
//! `Session.workspace` + `project_id` pair: a project pins
//! a path (typically a vault or git repo) and provides a
//! grouping for sessions.
//!
//! Projects are *implicit foreign keys* — sessions carry the
//! project id directly, no join table. Removing a project
//! does **not** delete its sessions; those become orphans
//! the curator can re-link.

use chrono::{DateTime, Utc};
use facet::Facet;

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct Project {
    pub id: String,
    pub name: String,
    /// Absolute filesystem path (canonicalized).
    pub path: String,
    /// Whether this project corresponds to a Task vault
    /// (relevant for wiki / task / etc. binding crates).
    pub is_vault: bool,
    /// Git metadata if the path is a worktree.
    pub git: Option<GitContext>,
    /// Per-project settings (launch scripts, default
    /// profile, ignored paths).
    pub settings: ProjectSettings,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct GitContext {
    /// Absolute path to the git repo root.
    pub repo_root: String,
    /// Currently checked-out branch.
    pub branch: String,
    /// Whether this path is a worktree (vs. main repo).
    pub is_worktree: bool,
    /// Parent project id if this is a worktree-derived
    /// project. Empty string when not.
    pub parent_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct ProjectSettings {
    /// Profile id to assume when starting a new session
    /// from this project. Empty = global default.
    pub default_profile_id: String,
    /// Backend id to use when there's no profile match.
    pub default_backend_id: String,
    /// Optional shell command to run before the first
    /// session of the day (env setup, worktree prep).
    pub launch_script: String,
    /// Paths to exclude from any agent's view of the
    /// project (`node_modules/`, `target/`, ...).
    pub ignore: Vec<String>,
}
