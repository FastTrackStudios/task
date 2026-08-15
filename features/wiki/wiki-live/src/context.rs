//! `schema.md` + `purpose.md` + `index.md` reads.

use std::fs;

use wiki_proto::paths;
use wiki_proto::schema::{default_purpose_doc, default_schema_doc};

use crate::error::WikiLiveError;
use crate::vault::WikiLive;

/// Bundle the three context docs the ingest prompt needs.
#[derive(Debug, Clone)]
pub struct WikiContext {
    pub schema_markdown: String,
    pub purpose_markdown: String,
    pub index_markdown: String,
    pub overview_markdown: String,
}

impl WikiLive {
    /// Read the schema + purpose + index + overview docs.
    /// Missing files are returned as empty strings — the
    /// caller's prompt template handles `{wiki_overview}`
    /// gracefully.
    pub fn read_context(&self) -> Result<WikiContext, WikiLiveError> {
        if !self.is_bootstrapped() {
            return Err(WikiLiveError::NotBootstrapped);
        }
        let root = self.wiki_root();
        Ok(WikiContext {
            schema_markdown: read_or_empty(&root.join(paths::SCHEMA_MD))?,
            purpose_markdown: read_or_empty(&root.join(paths::PURPOSE_MD))?,
            index_markdown: read_or_empty(&root.join(paths::INDEX_MD))?,
            overview_markdown: read_or_empty(&root.join(paths::OVERVIEW_MD))?,
        })
    }
}

fn read_or_empty(path: &std::path::Path) -> Result<String, WikiLiveError> {
    if path.is_file() {
        Ok(fs::read_to_string(path)?)
    } else {
        Ok(String::new())
    }
}

pub(crate) fn ensure_schema(wiki: &WikiLive) -> Result<bool, WikiLiveError> {
    let path = wiki.wiki_root().join(paths::SCHEMA_MD);
    if path.is_file() {
        return Ok(false);
    }
    fs::write(&path, default_schema_doc())?;
    Ok(true)
}

pub(crate) fn ensure_purpose(wiki: &WikiLive) -> Result<bool, WikiLiveError> {
    let path = wiki.wiki_root().join(paths::PURPOSE_MD);
    if path.is_file() {
        return Ok(false);
    }
    fs::write(&path, default_purpose_doc())?;
    Ok(true)
}
