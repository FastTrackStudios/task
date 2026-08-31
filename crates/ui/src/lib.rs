//! Complete application UI shell.
//!
//! Hosts the router, sidebar, mobile chrome, and every top-level
//! page. Feature crates (under `features/*/x-ui`) provide
//! reusable components; this crate composes them into the
//! product surface.
//!
//! Two seams keep this crate from being *everything*:
//!
//! - [`task_ui_core`] holds what every Task UI crate needs (vox
//!   endpoint + connection root, display formatting, frontmatter
//!   reads). It is re-exported here as [`format`] / [`vox_session`] so
//!   existing `crate::…` paths still resolve.
//! - [`task_player_ui`] owns the browser session player (audio, charts,
//!   Now Playing) — the only reason this crate ever depended on `daw`,
//!   `daw-standalone`, `session` or the keyflow engraver.
//!
//! See `ARCHITECTURE.md` for the recipe for extracting the next slice.

pub mod actions;
pub mod app;
pub mod app_views;
pub mod auth;
pub mod chrome;
pub mod collab;
pub mod device_pairing;
pub mod document_session;
pub mod feeds;
pub mod forge_views;
pub mod fuzzy;
pub mod gantt_adapt;
pub mod guest_share;
pub mod media_session;
pub mod nav;
pub mod orgs;
pub mod pages;
pub mod palette;
pub mod plugin_gate;
pub mod prefs;
pub mod presence;
pub mod project_declaration;
pub mod routes;
pub mod search;
pub mod server_registry;
pub mod shell;
pub mod shortcuts;
pub mod stores;
pub mod tabs;
pub mod tag_icon;
pub mod task_sort;
pub mod theming;
pub mod vault_lookup;
pub mod vox_clients;
pub mod watch_sync;

// Re-exported from `task-ui-core` at their historical paths — see the
// module docs above. `crate::format::money`, `crate::vox_session::vox_url`
// and friends resolve exactly as before.
pub use task_ui_core::{format, states, vox_session};

pub use app::App;
pub use routes::Route;
