//! Slim domain trait — the get-with-defaults / upsert pair the app
//! drives.
//!
//! Plain CRUD on the row is the architect-emitted `UserPrefsRepo`;
//! this trait carries the two semantics that surface can't express:
//! `get` never errors on a missing row (first launch on a new device
//! must not be an error path), and `set` upserts so callers don't care
//! whether a row exists yet.

use uuid::Uuid;

use crate::error::PrefsError;
use crate::user_prefs::UserPrefs;

#[architect::rpc]
pub trait PrefsService {
    /// Read `user_id`'s preferences. A user with no stored row gets
    /// [`UserPrefs::defaults_for`] (task-board filters on, page +
    /// location unset) — never an error.
    async fn get(&self, user_id: Uuid) -> Result<UserPrefs, PrefsError>;

    /// Upsert the full preferences row (keyed by `prefs.user_id`) and
    /// return what was stored.
    async fn set(&self, prefs: UserPrefs) -> Result<UserPrefs, PrefsError>;
}
