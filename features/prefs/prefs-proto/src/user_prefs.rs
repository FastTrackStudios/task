//! [`UserPrefs`] — one preferences row per user (per org database).

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::Entity, ::facet::Facet, Serialize, Deserialize, Clone, Debug, PartialEq, Eq,
)]
#[architect(table_name = "user_prefs", repo)]
pub struct UserPrefs {
    /// The architect-auth user id (`AuthUser::id`). One row per user —
    /// the org scoping is the per-org `prefs.sqlite`, not a column.
    #[architect(primary_key, auto_increment = false, filterable)]
    pub user_id: Uuid,

    /// Route the app opens on. Empty = app default.
    pub default_page: String,

    /// Task-board "active" filter default.
    pub tasks_active: bool,

    /// Task-board "relevant" filter default.
    pub tasks_relevant: bool,

    /// The user's last "I'm at" context for relevance gating.
    /// Empty = unset.
    pub location: String,

    /// Picked theme preset name (an `architect-ui` `theme_presets()` entry).
    /// Empty = never picked — the client keeps its built-in default.
    /// `#[serde(default)]` so pre-theme localStorage boot caches still
    /// deserialize.
    #[serde(default)]
    pub theme_preset: String,

    /// Light/dark choice: `"light"` / `"dark"`. Empty = never picked.
    #[serde(default)]
    pub theme_mode: String,

    /// Keyboard: app shortcuts win over browser defaults they shadow
    /// (Ctrl+P print, Ctrl+K omnibox, …). Default ON; turning it off
    /// keeps only the non-conflicting shortcuts active. `serde` default
    /// is `true` so pre-keyboard boot caches keep the on-by-default
    /// behavior.
    #[serde(default = "default_shortcuts_priority")]
    pub shortcuts_priority: bool,
}

/// Serde default for [`UserPrefs::shortcuts_priority`] — on.
fn default_shortcuts_priority() -> bool {
    true
}

impl UserPrefs {
    /// The defaults a user without a stored row gets — what
    /// [`crate::PrefsService::get`] returns before the first `set`.
    /// Both task-board filters default on; page + location + theme
    /// unset.
    #[must_use]
    pub fn defaults_for(user_id: Uuid) -> Self {
        Self {
            user_id,
            default_page: String::new(),
            tasks_active: true,
            tasks_relevant: true,
            location: String::new(),
            theme_preset: String::new(),
            theme_mode: String::new(),
            shortcuts_priority: true,
        }
    }
}
