//! `VaultSync` — the canonical sync trait, decorated with
//! `#[architect::rpc]`.
//!
//! The macro derives the async vox face from this sync trait:
//! backends impl `VaultSync` directly (zero-cost in-process call
//! sites), and remote callers reach the same surface via the
//! auto-emitted [`VaultSyncClient`] over vox. See
//! `architect/DESIGN.md`.
//!
//! Backends carry whatever state they need (the server-side
//! `VaultSyncState` holds the filesystem root + per-vault
//! broadcast channels), and additionally implement
//! [`architect::HasDispatcher`] so the bridge knows how to
//! marshal sync method calls onto the right thread —
//! `TokioBlockingDispatcher` for the server, `CurrentThread`
//! for tests / in-process callers.

use crate::{
    BaseView, CollabAck, FileBytes, FolderIndex, IfMatch, Manifest, PutAck, VaultChange,
    VaultSyncError,
};

/// File-replication operations on a single server. Sync methods
/// (cheap when called in-process; marshaled through the
/// backend's `HasDispatcher` for remote callers). Live changes
/// arrive on the [`Self::changes`] `#[subscribe]` stream, served
/// from the backend's `architect::PubSub` hub.
#[architect::rpc]
pub trait VaultSync {
    /// List every file in `vault_id`. Empty vault = empty list,
    /// not an error.
    fn manifest(&self, vault_id: &str) -> Result<Manifest, VaultSyncError>;

    /// Read one file's bytes. Returns
    /// [`VaultSyncError::NotFound`] for missing paths.
    fn get_file(&self, vault_id: &str, path: &str) -> Result<FileBytes, VaultSyncError>;

    /// Write one file. Honors `if_match`; on conflict the
    /// returned error carries the server's current sha + bytes.
    fn put_file(
        &self,
        vault_id: &str,
        path: &str,
        bytes: Vec<u8>,
        if_match: IfMatch,
    ) -> Result<PutAck, VaultSyncError>;

    /// Remove one file. Idempotent: deleting a missing path
    /// succeeds.
    fn delete_file(
        &self,
        vault_id: &str,
        path: &str,
        if_match: IfMatch,
    ) -> Result<(), VaultSyncError>;

    /// Frontmatter-derived metadata for every `.md` page —
    /// path, basename, title, type, and the `folder` parent
    /// (Obsidian folder-note wikilink, resolved to a basename).
    /// Powers the virtual-folder sidebar without the client
    /// fetching + parsing each file.
    fn folder_index(&self, vault_id: &str) -> Result<FolderIndex, VaultSyncError>;

    /// Re-file a note: set or clear its `folder` frontmatter
    /// property. `parent` is the target folder note's basename,
    /// or `None` to move the note to the root. The edit is a
    /// surgical frontmatter splice (other properties + key order
    /// preserved). Honors `if_match` like [`Self::put_file`] and
    /// returns the freshly-committed sha.
    fn set_folder(
        &self,
        vault_id: &str,
        path: &str,
        parent: Option<String>,
        if_match: IfMatch,
    ) -> Result<PutAck, VaultSyncError>;

    /// Register `path` for per-file CRDT collaboration and return
    /// its doc id ([`crate::collab_doc_id`] of `(vault_id, path)`)
    /// plus the file's current sha. Validates the path exists
    /// ([`VaultSyncError::NotFound`] otherwise) and records the
    /// `doc_id → (vault_id, path)` reverse mapping the server's doc
    /// registry consults for admission and write-behind routing.
    /// Idempotent — re-opening an already-registered file refreshes
    /// the sha and returns the same id.
    fn open_collab(&self, vault_id: &str, path: &str) -> Result<CollabAck, VaultSyncError>;

    /// Run every view of the `.base` file at `base_path` against
    /// `vault_id`'s pages, returning the rendered tables (column headers
    /// + grouped, projected rows) for the in-app bases view. The query
    /// engine is native, so this is server-only;
    /// [`VaultSyncError::NotFound`] if the base doesn't exist.
    fn base_views(&self, vault_id: &str, base_path: &str) -> Result<Vec<BaseView>, VaultSyncError>;

    /// Every vault change, as it happens — fires on each
    /// successful PUT / DELETE / folder re-file, and on external
    /// edits picked up by the filesystem watcher. The stream is
    /// unfiltered (all vault ids this backend serves); each
    /// [`VaultChange`] carries its `vault_id` so subscribers can
    /// keep the one they browse. See [`VaultChange`] for the
    /// fetch-once-then-fold subscriber contract.
    #[subscribe]
    fn changes(&self) -> VaultChange;
}
