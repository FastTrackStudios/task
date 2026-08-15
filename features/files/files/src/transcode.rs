//! Transcode integration (issue #269): building a
//! [`files_transcode::TranscodePipeline`] over a root's chunk store,
//! and mapping the wire [`files_proto::RenditionKind`] to the engine's.
//!
//! The pipeline lives per-root (its rendition index sits beside the
//! version store, under `renditions/`), sharing the root's chunk store
//! for both source reads and rendition storage. The backend owns the
//! injected [`files_transcode::Transcoder`]; this module just wires a
//! pipeline from the two.

use std::path::PathBuf;

use files_transcode::RenditionStore;

use crate::error::{Error, Result};
use crate::repo_open;

/// Map the wire rendition kind to the engine's.
#[must_use]
pub fn engine_kind(kind: files_proto::RenditionKind) -> files_transcode::RenditionKind {
    use files_proto::RenditionKind as W;
    use files_transcode::RenditionKind as E;
    match kind {
        W::Proxy1080 => E::Proxy1080,
        W::Proxy720 => E::Proxy720,
        W::Audio => E::Audio,
        W::Peaks => E::Peaks,
        W::Filmstrip => E::Filmstrip,
    }
}

/// The rendition store directory for a root (under its store dir).
#[must_use]
pub fn rendition_dir(root_path: &std::path::Path) -> PathBuf {
    repo_open::store_dir(root_path).join("renditions")
}

/// Open a root's rendition store on its own (for GC / introspection).
pub async fn open_store(root_path: &std::path::Path) -> Result<RenditionStore> {
    RenditionStore::open(rendition_dir(root_path))
        .await
        .map_err(|e| Error::Repo(format!("rendition store: {e}")))
}

#[cfg(test)]
mod tests {
    use super::engine_kind;

    /// The wire kind's `tag()` (what the UI builds streaming URLs with)
    /// must stay in lockstep with the engine tag the server parses the
    /// route's `{kind}` segment against.
    #[test]
    fn wire_tags_mirror_engine_tags() {
        use files_proto::RenditionKind as W;
        for kind in [W::Proxy1080, W::Proxy720, W::Audio, W::Peaks, W::Filmstrip] {
            assert_eq!(kind.tag(), engine_kind(kind).tag(), "{kind:?}");
        }
    }
}
