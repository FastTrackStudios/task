//! [`BackendKind`] discriminator + [`ContentBackend`] trait.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::entry::Entry;
use crate::error::MountError;

/// Which backend impl serves a [`crate::Mount`]. Stored as a
/// JSON column on the row + as a TOML string in `mounts.toml`
/// — kebab-case serde rename keeps both readable.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(u8)]
#[serde(rename_all = "kebab-case")]
pub enum BackendKind {
    /// Local filesystem. `path` is an absolute path on this
    /// machine. The default for everything Phase 2 ships.
    #[default]
    Filesystem,
    /// Nextcloud WebDAV remote. `url` points at the Nextcloud
    /// instance; `path` is the vault-relative sub-tree. Phase
    /// 4 work — variant reserved so older mounts.toml files
    /// stay deserializable once it lands.
    Nextcloud,
    /// Remote task-server proxy over the existing vault-sync
    /// vox RPC. Lets a thin client (phone, kiosk) read a
    /// project's content without holding a local copy. Phase
    /// 3 work — variant reserved.
    VoxProxy,
}

/// Per-backend access surface. Synchronous because Phase 2 is
/// shipped against the existing synchronous `vault-live`
/// backend; the future async backends (Nextcloud, VoxProxy)
/// will gain a parallel async trait or wrap blocking I/O
/// behind a worker pool — that decision is deferred until the
/// async impls actually arrive.
///
/// All paths are mount-relative. Implementations are
/// responsible for joining the mount root (filesystem) or
/// remote prefix (nextcloud / vox-proxy) on the inside.
pub trait ContentBackend {
    /// List entries at `prefix`. Empty prefix → mount root.
    /// Order is implementation-defined; consumers sort if they
    /// need stability.
    fn list(&self, prefix: &str) -> Result<Vec<Entry>, MountError>;

    /// Read a file's bytes. Returns `MountError::NotFound` for
    /// missing paths; other I/O is wrapped as `MountError::Io`.
    fn read(&self, path: &str) -> Result<Vec<u8>, MountError>;

    /// Write a file. Creates parent directories as needed.
    /// Existing files are overwritten — concurrency is the
    /// caller's problem at this layer (CRDT / vault-sync
    /// handles conflict reconciliation upstream).
    fn write(&self, path: &str, bytes: &[u8]) -> Result<(), MountError>;

    /// Remove a file. Idempotent — removing a missing path is
    /// not an error.
    fn delete(&self, path: &str) -> Result<(), MountError>;

    /// Cheap existence probe. Should not error for the
    /// missing-file case — return `Ok(false)`.
    fn exists(&self, path: &str) -> Result<bool, MountError>;
}
