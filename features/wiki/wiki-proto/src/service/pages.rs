//! Curated page read/list/write — the wiki UI's editor surface.

use crate::error::WikiError;
use crate::pages::{PageInfo, WikiPageDoc};

#[architect::rpc]
pub trait Pages {
    /// Every curated `.md` under the wiki root (the `raw/`,
    /// `_state/` and `media/` subtrees are excluded — the raw
    /// layer has its own service). Sorted by path.
    fn list_pages(&self, wiki_id: &str) -> Result<Vec<PageInfo>, WikiError>;

    /// Read one page. `path` is wiki-root-relative.
    fn read_page(&self, wiki_id: &str, path: &str) -> Result<WikiPageDoc, WikiError>;

    /// Write one page, optimistically guarded: when `base_sha256`
    /// is non-empty and the file's current sha differs, the write
    /// is rejected with [`WikiError::IllegalState`] so the editor
    /// can surface the conflict instead of clobbering. Empty
    /// `base_sha256` writes unconditionally (also creates new
    /// pages). Returns the saved doc (fresh sha).
    fn write_page(
        &self,
        wiki_id: &str,
        path: &str,
        markdown: &str,
        base_sha256: &str,
    ) -> Result<WikiPageDoc, WikiError>;
}
