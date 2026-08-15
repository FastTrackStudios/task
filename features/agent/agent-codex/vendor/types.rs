//! Minimal type shim for the vendored modules. `CodexMonitor`'s
//! `types.rs` is 1418 lines of UI config; the vendored
//! `app_server.rs` + `args.rs` only touch a tiny subset
//! (`entry.id`, `entry.path`, and `AppSettings.codex_args`).
//! This shim covers exactly that surface so the vendored
//! code compiles without dragging in unrelated config.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceEntry {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) kind: WorkspaceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) worktree: Option<WorktreeInfo>,
    #[serde(default)]
    pub(crate) settings: WorkspaceSettings,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum WorkspaceKind {
    Main,
    Worktree,
}

impl Default for WorkspaceKind {
    fn default() -> Self {
        WorkspaceKind::Main
    }
}

impl WorkspaceKind {
    #[allow(dead_code)]
    pub(crate) fn is_worktree(&self) -> bool {
        matches!(self, WorkspaceKind::Worktree)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct WorktreeInfo {
    pub(crate) branch: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct WorkspaceSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) launch_script: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) git_root: Option<String>,
}

/// Only the field used by the vendored `args.rs`. Other
/// `CodexMonitor` settings (theme, fonts, shortcuts, dictation)
/// are intentionally absent.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub(crate) struct AppSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) codex_args: Option<String>,
}
