//! Structural mutation — `files.write.surface`.
//!
//! The dominant gap in v1: there were no write RPCs at all, so nothing
//! could be created, renamed, moved or deleted over the network. Most of
//! what a cloud provider calls file management is this lane.
//!
//! Every operation here is transactional and wrapped in exactly one
//! version-store operation, so history records the user's action rather
//! than its constituent writes. Every one takes a set of paths as readily
//! as one, applying atomically across the set.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::error::FilesFault;
use crate::id::RootId;
use crate::path::RootPath;
use crate::service::tree::CatalogueEntry;

/// What to do when the destination exists.
///
/// No default and no inference: `files.write.upload` forbids choosing for
/// the user, and the same discipline applies to move and copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum OnConflict {
    /// Refuse, returning [`FilesFault::Exists`]. Ask a human.
    Fail,
    /// Keep both — the incoming one is placed under a derived name,
    /// returned in the outcome so the caller can show what happened.
    KeepBoth,
    /// Replace, recording a new version rather than discarding the old.
    Replace,
    /// Leave the existing one alone and drop the incoming.
    KeepExisting,
}

/// A move or copy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Relocation {
    pub from: RootPath,
    pub to: RootPath,
}

/// What actually happened to one path in a batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct Outcome {
    pub path: RootPath,
    /// Where it ended up. Differs from the requested destination under
    /// [`OnConflict::KeepBoth`], and is absent for a delete.
    pub landed_at: Option<RootPath>,
    /// True when nothing was done because of [`OnConflict::KeepExisting`].
    pub skipped: bool,
}

/// The result of one transactional batch.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(C)]
pub struct WriteReceipt {
    pub root_id: RootId,
    pub outcomes: Vec<Outcome>,
    /// The version-store operation this batch became — one per batch,
    /// so history shows the action and not its parts.
    pub operation: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[repr(u8)]
pub enum WriteEvent {
    Created(Vec<CatalogueEntry>),
    Moved(WriteReceipt),
    Copied(WriteReceipt),
    /// Deletion is a checkpoint, not a trash move: "recently deleted" is
    /// a lens over history, so nothing is stored twice and there is no
    /// second collector to maintain.
    Deleted(WriteReceipt),
}

#[architect::rpc]
pub trait WriteService {
    /// Create directories, including missing parents. Existing ones are
    /// not an error — this is idempotent by design, because a client
    /// retrying after a dropped connection must not fail.
    async fn create_dirs(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
    ) -> Result<WriteReceipt, FilesFault>;

    /// Rename in place. The parent is unchanged; use [`Self::move_paths`]
    /// to relocate.
    async fn rename(
        &self,
        root_id: RootId,
        path: RootPath,
        name: String,
    ) -> Result<WriteReceipt, FilesFault>;

    /// Move a set of paths, atomically across the set.
    ///
    /// Fails with [`FilesFault::IntoSelf`] for a destination inside its
    /// own source — the check that has to exist once rather than at each
    /// call site.
    async fn move_paths(
        &self,
        root_id: RootId,
        moves: Vec<Relocation>,
        on_conflict: OnConflict,
    ) -> Result<WriteReceipt, FilesFault>;

    /// Copy a set of paths. Content-addressed, so copying a 200 GB folder
    /// within a root transfers no bytes and costs no space.
    async fn copy_paths(
        &self,
        root_id: RootId,
        copies: Vec<Relocation>,
        on_conflict: OnConflict,
    ) -> Result<WriteReceipt, FilesFault>;

    /// Delete a set of paths, atomically across the set.
    ///
    /// History replaces trash: this checkpoints, and the content stays
    /// recoverable through [`super::version::VersionService`] until
    /// retention collects it.
    async fn delete_paths(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
    ) -> Result<WriteReceipt, FilesFault>;

    /// Open an archive stream over a selection.
    ///
    /// Returns a ticket the caller redeems on the byte lane. The archive
    /// is generated as it is sent and never materialised server-side —
    /// a 200 GB selection costs no disk.
    async fn archive(
        &self,
        root_id: RootId,
        paths: Vec<RootPath>,
    ) -> Result<crate::service::media::ByteTicket, FilesFault>;
}
