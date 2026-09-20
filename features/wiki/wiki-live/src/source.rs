//! What a subscription refresh reads from, and nothing more.
//!
//! # Why this is not `VaultSync`
//!
//! [`materialize::refresh`](crate::materialize::refresh) used to be
//! generic over the whole of `vault_proto::VaultSync` — nine methods,
//! four of which write. It calls exactly two of them, `manifest` and
//! `get_file`, and its own docs say why: *"There is no code path from a
//! refresh to a `put_file`."*
//!
//! That sentence was a promise the types did not keep. A subscription
//! deliberately withholds the subscriber's edits from upstream
//! (`wiki.subscribe.local-copy`), so handing the refresh a surface it
//! could push through was an invitation for a later change to push by
//! accident — against somebody else's organisation, which is the one
//! direction that must never happen silently.
//!
//! So the refresh takes [`SourceVault`]: read the list, read a file.
//! Now "a refresh cannot write upstream" is a fact about the type
//! rather than a claim in a comment.
//!
//! # Why that is what makes a remote source possible
//!
//! The narrowing is also what lets a source live on another server. A
//! remote implementation owes two methods instead of nine, and neither
//! of the two it owes has an `if_match` or a conflict to resolve.
//! Everything Task serves over vox is `async`, and `VaultSync` is
//! `sync`, so a remote source has to bridge — and bridging two read
//! calls is a thing one can read and check, while bridging a
//! write-with-compare-and-swap is a distributed-systems question.

use std::path::PathBuf;
use std::sync::Arc;

use vault_proto::{FileBytes, Manifest, VaultSync, VaultSyncError};

/// The read-only half of a vault: enough to pull a copy, and no more.
///
/// Implemented for free by anything that is a `VaultSync` — the local
/// disk backend (`vault_live::Backend`) arrives here unchanged — and by
/// hand for a source reached over the wire.
pub trait SourceVault: Send + Sync {
    /// Every file the source holds, with its hash.
    ///
    /// # Errors
    ///
    /// Whatever reading the source's index fails with: an IO error for
    /// a local source, a transport error for a remote one.
    fn manifest(&self, vault_id: &str) -> Result<Manifest, VaultSyncError>;

    /// One file's bytes.
    ///
    /// # Errors
    ///
    /// [`VaultSyncError::NotFound`] when the path is not in the source,
    /// which a refresh treats as the file having gone away between
    /// listing it and fetching it rather than as a failure.
    fn get_file(&self, vault_id: &str, path: &str) -> Result<FileBytes, VaultSyncError>;
}

impl<T: VaultSync + Send + Sync> SourceVault for T {
    fn manifest(&self, vault_id: &str) -> Result<Manifest, VaultSyncError> {
        VaultSync::manifest(self, vault_id)
    }

    fn get_file(&self, vault_id: &str, path: &str) -> Result<FileBytes, VaultSyncError> {
        VaultSync::get_file(self, vault_id, path)
    }
}

/// Where a subscribed source actually is.
///
/// The shape [`crate::subscriptions_backend::Upstream`] answers with,
/// and the reason it is an enum rather than an `Option<PathBuf>`: a
/// path can only name a publisher on this disk. The previous signature
/// could not express a remote source at all, which is why the remote
/// half went unwritten while its doc comment claimed the shape would
/// carry it.
pub enum Source {
    /// A publisher on this same data root — its directory.
    ///
    /// Still a path rather than a [`SourceVault`], because a publisher on
    /// this disk is cheaper to read as a tree: `materialize::refresh_assets`
    /// copies a shelf or a corpus file by file with no round trip and no
    /// size to bound, which is exactly what the wire cannot offer.
    Local(PathBuf),
    /// A publisher on another server, reached over the wire.
    ///
    /// Built by `task_server::federated_orgs`, which is where dialling
    /// lives — this crate has no transport and should not grow one. It
    /// serves a wiki and an asset shelf: the vault manifest carries any
    /// file, so what limits it is size rather than kind
    /// (`materialize::REMOTE_FILE_LIMIT`). A project's media and a
    /// Resource's corpus take other routes, and a refresh says which
    /// rather than failing as if the source were missing.
    Remote(Arc<dyn SourceVault>),
}
