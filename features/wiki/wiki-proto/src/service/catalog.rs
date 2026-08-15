//! `index.md` catalog + `log.md` operation timeline.

use crate::error::WikiError;
use crate::log::{LogEntry, WikiIndex};

#[architect::rpc]
pub trait Catalog {
    fn read_index(&self, wiki_id: &str) -> Result<WikiIndex, WikiError>;
    fn rebuild_index(&self, wiki_id: &str) -> Result<WikiIndex, WikiError>;
    fn append_log(&self, wiki_id: &str, entry: LogEntry) -> Result<(), WikiError>;
}
