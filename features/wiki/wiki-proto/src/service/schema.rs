//! Bootstrap + schema/purpose docs + health snapshot.

use crate::error::WikiError;
use crate::health::WikiHealth;
use crate::schema::{PurposeDoc, SchemaDoc};

#[architect::rpc]
pub trait Schema {
    /// Initialize `Wiki/` in the named vault if it doesn't
    /// already exist. Writes default `schema.md` +
    /// `purpose.md`, scaffolds `index.md` + `log.md` +
    /// `raw/sources/` + `media/` + `_state/`. Idempotent.
    fn bootstrap(&self, wiki_id: &str) -> Result<(), WikiError>;
    fn read_schema(&self, wiki_id: &str) -> Result<SchemaDoc, WikiError>;
    fn read_purpose(&self, wiki_id: &str) -> Result<PurposeDoc, WikiError>;
    fn write_schema(&self, wiki_id: &str, markdown: &str) -> Result<(), WikiError>;
    fn write_purpose(&self, wiki_id: &str, markdown: &str) -> Result<(), WikiError>;
    fn health(&self, wiki_id: &str) -> Result<WikiHealth, WikiError>;
}
