//! FS-event source watcher toggle.

use crate::error::WikiError;

#[architect::rpc]
pub trait Watcher {
    /// Toggle the backend's source watcher. When enabled,
    /// FS events under `Wiki/raw/sources/` auto-enqueue
    /// ingest tasks. Returns the new state.
    fn set_watch(&self, wiki_id: &str, enabled: bool) -> Result<bool, WikiError>;
    fn is_watching(&self, wiki_id: &str) -> Result<bool, WikiError>;
}
