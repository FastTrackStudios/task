//! Root lifecycle and adoption — `files.adopt.*`.
//!
//! Most content does not arrive by upload. It is already on disk, written
//! by the applications that made it, and has to become ours without
//! ceasing to be theirs. Adoption is therefore not a bulk import: it
//! publishes structure immediately and reads bytes behind it.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::RootId;
use crate::model::{FileRootInfo, RootFlavor};

/// How far adoption has got. A root is usable at every stage but the
/// first — `files.adopt.catalogue-first`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum AdoptionPhase {
    /// Walking the tree, publishing entries from name/size/mtime.
    Enumerating,
    /// Structure is complete and browsable; content addresses are still
    /// being computed. Entries without one are marked unverified.
    Hashing,
    /// Every entry has a verified content address.
    Complete,
    /// Stopped before completing. Resuming continues rather than
    /// restarting — `files.adopt.resumable`.
    Paused,
}

/// Adoption's progress, for a UI that must not imply the tree is
/// incomplete when it is merely unverified.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AdoptionProgress {
    pub root_id: RootId,
    pub phase: AdoptionPhase,
    /// Entries published so far — browsable now, whatever the phase.
    pub entries_seen: u64,
    /// Of those, how many carry a verified content address.
    pub entries_hashed: u64,
    pub bytes_seen: u64,
    pub bytes_hashed: u64,
    /// Set when the walk finished, so a percentage is honest before then.
    pub entries_total: Option<u64>,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Everything needed to adopt a tree. A struct rather than four params
/// because this is exactly the signature that would break next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct AdoptRequest {
    /// The existing directory. Nothing under it is moved, copied or
    /// renamed — `files.adopt.in-place`.
    pub path: String,
    pub name: String,
    /// Fixed for the root's life.
    pub flavor: RootFlavor,
    /// Read bytes as well as structure. `false` publishes the catalogue
    /// and stops, for a tree being surveyed rather than taken on.
    pub hash_content: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum RootEvent {
    Created(FileRootInfo),
    /// Adoption advanced. Emitted on phase changes and periodically
    /// during `Hashing`, not per file.
    AdoptionProgressed(AdoptionProgress),
    Renamed(FileRootInfo),
    /// The root is no longer tracked. Its bytes are untouched on disk —
    /// releasing is not deleting.
    Released(RootId),
}

#[architect::rpc]
pub trait RootsService {
    /// Adopt an existing directory as a File Root.
    ///
    /// Returns as soon as the root has an identity — before the tree is
    /// walked, let alone hashed. Follow [`RootEvent::AdoptionProgressed`]
    /// for the rest. The applications already writing this tree keep
    /// writing it throughout; a file modified mid-hash is re-hashed
    /// rather than recorded wrongly.
    ///
    /// Fails with [`FilesFault::AlreadyRoot`] if the path is one already,
    /// and [`FilesFault::NotADirectory`] if it is not a directory.
    async fn adopt(&self, request: AdoptRequest) -> Result<FileRootInfo, FilesFault>;

    /// Resume an adoption left in [`AdoptionPhase::Paused`]. Continues
    /// from where it stopped; work in flight when it stopped is redone,
    /// nothing else is.
    async fn resume_adoption(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault>;

    /// Stop an adoption in progress, leaving what has been published
    /// browsable.
    async fn pause_adoption(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault>;

    /// Where an adoption has got to.
    async fn adoption_progress(&self, root_id: RootId) -> Result<AdoptionProgress, FilesFault>;

    /// Host a root's structure without holding its content.
    ///
    /// `files.peering.replication`: an org's structure converges across
    /// every server hosting it, and content moves only where placement
    /// says. This is the receiving side of that — the root becomes real
    /// here, with the same id it has everywhere, and no tree underneath
    /// it.
    ///
    /// Not adoption. [`Self::adopt`] takes a folder that is already on
    /// this disk and gives it an identity; this takes an identity that
    /// already exists elsewhere and gives it a presence here. The id is
    /// the caller's, because it is the id in the folder's own marker on
    /// whichever host does hold the tree, and minting a new one would
    /// make the same root two roots.
    ///
    /// Idempotent: hosting a root already hosted is not an error, which
    /// is what lets a peer re-run reconciliation without checking first.
    async fn host_structure(
        &self,
        root_id: RootId,
        name: String,
        flavor: crate::model::RootFlavor,
    ) -> Result<FileRootInfo, FilesFault>;

    /// Every root this org can reach.
    async fn list(&self) -> Result<Vec<FileRootInfo>, FilesFault>;

    /// One root.
    async fn get(&self, root_id: RootId) -> Result<FileRootInfo, FilesFault>;

    /// Rename the root. Its identity, path and history are unaffected —
    /// this is the display name only.
    ///
    /// `rename_root`, not `rename`, because [`WriteService::rename`]
    /// renames a path *inside* a root. Both are implemented on the same
    /// backend, and a bare `rename` there is ambiguous in a way a reader
    /// cannot resolve — the compiler catches it, but only after someone
    /// has written the wrong one.
    ///
    /// [`WriteService::rename`]: crate::service::write::WriteService::rename
    async fn rename_root(&self, root_id: RootId, name: String)
    -> Result<FileRootInfo, FilesFault>;

    /// Stop tracking the root. The directory and every byte in it stay
    /// exactly where they are; only our record of it goes. Re-adopting
    /// the same path recovers the history, which lives in the tree.
    async fn release(&self, root_id: RootId) -> Result<(), FilesFault>;
}
