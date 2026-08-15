//! Vendored from `CodexMonitor/src-tauri/src/codex/args.rs`.
//! Visibility adapted (`pub(crate)` → `pub(super)`).

use super::types::{AppSettings, WorkspaceEntry};

pub(super) fn parse_codex_args(value: Option<&str>) -> Result<Vec<String>, String> {
    let raw = match value {
        Some(raw) if !raw.trim().is_empty() => raw.trim(),
        _ => return Ok(Vec::new()),
    };
    shell_words::split(raw)
        .map_err(|err| format!("Invalid Codex args: {err}"))
        .map(|args| args.into_iter().filter(|arg| !arg.is_empty()).collect())
}

#[allow(dead_code)]
pub(super) fn resolve_workspace_codex_args(
    _entry: &WorkspaceEntry,
    _parent_entry: Option<&WorkspaceEntry>,
    app_settings: Option<&AppSettings>,
) -> Option<String> {
    if let Some(settings) = app_settings {
        if let Some(value) = settings.codex_args.as_deref() {
            return normalize_codex_args(value);
        }
    }
    None
}

fn normalize_codex_args(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}
