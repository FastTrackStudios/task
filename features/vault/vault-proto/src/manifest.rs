//! Manifest types — one [`ManifestEntry`] per file in a
//! vault, aggregated into a [`Manifest`] returned by
//! [`crate::VaultSync::manifest`]. Used by clients to
//! compute diffs against their local state before pulling
//! individual files.
//!
//! `ManifestEntry` is an `architect::Entity` so the same
//! schema mounts to SQLite under the `server` feature —
//! useful for vaults large enough that scanning the full
//! manifest on every diff gets expensive. PK is `path`
//! (String) — the natural key for a file inside a vault.
//! `Manifest` stays a wire wrapper around `Vec<ManifestEntry>`.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "vault_manifest_entries", repo)]
pub struct ManifestEntry {
    /// Vault-relative path. PK so reverse lookups (by path)
    /// are an index hit instead of a scan.
    #[architect(primary_key, auto_increment = false)]
    pub path: String,
    /// Content hash. Lowercase hex sha256.
    #[architect(filterable)]
    pub sha256: String,
    /// File modification time in milliseconds since epoch.
    #[architect(filterable, sortable)]
    pub mtime_ms: i64,
    pub size: u64,
    /// Architect-managed timestamps. Track when this row
    /// was first observed on the server vs last refreshed.
    #[architect(exclude(create, update), on_create = Utc::now())]
    pub created_at: DateTime<Utc>,
    #[architect(exclude(create, update), on_create = Utc::now(), on_update = Utc::now())]
    pub updated_at: DateTime<Utc>,
}

/// Full file listing for one vault. `vault_id` echoes the
/// request so a client juggling several vaults can
/// double-check what came back. `files` is unordered — sort
/// client-side if a stable order matters.
#[derive(Debug, Clone, Facet)]
pub struct Manifest {
    pub vault_id: String,
    pub files: Vec<ManifestEntry>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{Manifest, ManifestEntry};
    unsafe impl vox_types::Reborrow for ManifestEntry {
        type Ref<'a> = ManifestEntry;
    }
    unsafe impl vox_types::Reborrow for Manifest {
        type Ref<'a> = Manifest;
    }
}
