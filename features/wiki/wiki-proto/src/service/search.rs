//! Token + (optional) vector search.

use crate::error::WikiError;
use crate::search::{SearchHits, SearchOpts};

#[architect::rpc]
pub trait Search {
    fn search(&self, wiki_id: &str, opts: SearchOpts) -> Result<SearchHits, WikiError>;
}
