//! Server-side prefs impl.
//!
//! Sits on top of the SeaORM `Model`/`Entity` items emitted by
//! `#[derive(architect::Entity)]` on `prefs-proto`'s [`UserPrefs`];
//! adds the [`Store`] implementing `PrefsService`:
//!
//! - `get` returns [`prefs_proto::UserPrefs::defaults_for`] when no
//!   row exists — a fresh user/device is never an error path.
//! - `set` upserts the whole row keyed by `user_id`.

pub mod entity;
pub mod error;
pub mod migrations;
pub mod store;

pub use error::PrefsDbError;
pub use migrations::Migrator;
pub use prefs_proto::UserPrefs;
pub use store::Store;
