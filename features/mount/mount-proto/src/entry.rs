//! [`Entry`] — one row returned by [`crate::ContentBackend::list`].

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

/// One entry under a mount. Lightweight stat — no body, no
/// frontmatter parsing. Consumers wanting that fall back to
/// [`crate::ContentBackend::read`].
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct Entry {
    /// Mount-relative path. Forward slashes regardless of OS
    /// so the wire form is stable.
    pub path: String,
    /// True if this is a directory.
    pub is_dir: bool,
    /// File size in bytes. Zero for directories.
    pub size: u64,
    /// Mtime as wall-clock UTC. Backends without a reliable
    /// mtime (some object stores) round-trip whatever they
    /// have, falling back to the epoch — consumers should
    /// treat this as advisory, not authoritative.
    pub modified: DateTime<Utc>,
}
