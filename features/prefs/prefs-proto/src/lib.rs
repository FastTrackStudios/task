//! `prefs-proto` — per-user preferences wire contract.
//!
//! One [`UserPrefs`] row per user per org (each org's server keeps its
//! own `prefs.sqlite`, so the org scoping is the database, not a
//! column). It carries the personalization the app used to stash in
//! localStorage (`crates/ui/src/prefs.rs`) — default landing page, the
//! task-board filter defaults, and the last "I'm at" location — so the
//! CLI and every device share the same setup.
//!
//! ## Storage
//!
//! `UserPrefs` uses `#[derive(architect::Entity)]`, so it gets the
//! wasm-clean wire struct, `UserPrefsCreate` / `UserPrefsUpdate` /
//! `UserPrefsList`, the `UserPrefsRepo` `#[vox::service]` trait, and
//! (with `--features server`) the SeaORM
//! `Model`/`Entity`/`Column`/`Relation`/`ActiveModel` plus
//! `UserPrefsRepoStorage<C>`.
//!
//! [`PrefsService`] is the slim trait the app actually drives:
//! `get` never errors on a missing row (it hands back
//! [`UserPrefs::defaults_for`]), and `set` upserts.

pub mod error;
pub mod service;
pub mod user_prefs;

pub use error::PrefsError;
pub use service::PrefsService;
pub use user_prefs::UserPrefs;

/// The architect-generated vox client for [`PrefsService`] — what the
/// app calls via `establish_for::<PrefsServiceClient>(slug)`.
#[cfg(feature = "vox")]
pub use service::PrefsServiceClient;
