//! Read `.obsidian/` config so the importer can match Obsidian
//! semantics. Everything best-effort — missing files fall back to
//! defaults.

use std::path::Path;

use serde::Deserialize;

/// Subset of `.obsidian/app.json` we care about for sync. Fields we
/// don't need stay unread; serde's default `deny_unknown_fields` is
/// *off* so future Obsidian additions don't break the importer.
///
/// Obsidian writes `app.json` in camelCase
/// (`userIgnoreFilters`, `attachmentFolderPath`, …); the struct
/// uses `snake_case` + `rename_all` so Rust call sites stay
/// idiomatic.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ObsidianConfig {
    /// Where Obsidian stores attachments by default. Vault-relative.
    #[serde(default)]
    pub attachment_folder_path: String,

    /// `"shortest"` | `"relative"` | `"absolute"`. Matches Obsidian's
    /// `newLinkFormat` setting.
    #[serde(default = "default_new_link_format")]
    pub new_link_format: String,

    /// When true, links serialize as `[name](path.md)` instead of
    /// `[[name]]`.
    #[serde(default)]
    pub use_markdown_links: bool,

    /// Glob-ish patterns the user has told Obsidian to ignore. v1
    /// supports prefix (`prefix*`) and suffix (`*suffix`) only.
    /// Unrecognized patterns are ignored with a tracing warning at
    /// load time.
    #[serde(default)]
    pub user_ignore_filters: Vec<String>,
}

fn default_new_link_format() -> String {
    "shortest".into()
}

/// Load `.obsidian/app.json` (or return defaults if missing /
/// unparseable). Logs but does not fail on parse errors — Obsidian
/// itself tolerates partial configs.
pub fn read_obsidian_config(vault_root: &Path) -> ObsidianConfig {
    let path = vault_root.join(".obsidian").join("app.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return ObsidianConfig::default(),
    };
    match serde_json::from_str::<ObsidianConfig>(&raw) {
        Ok(cfg) => cfg,
        Err(e) => {
            tracing::warn!(
                ?path,
                ?e,
                "could not parse .obsidian/app.json — using defaults"
            );
            ObsidianConfig::default()
        }
    }
}
