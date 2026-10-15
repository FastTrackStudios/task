//! Vault documents: the codec between a vault file's Loro document and
//! its markdown, and the device-side replica that syncs it.
//!
//! The rules this crate is for (`docs/spec/offline.md`):
//!
//! - **`offline.doc.shape`** — one Loro document per vault file: a
//!   `body` text and a `props` map of its frontmatter fields
//!   ([`doc`], over the container names in `vault_proto`).
//! - **`offline.doc.materialise`** — markdown is always produced from the
//!   document by one conversion ([`doc::render`] / [`markdown::materialise`]),
//!   byte-stable for unchanged documents; text arriving from outside is
//!   merged into the document as a change of its own ([`doc::merge_text`]),
//!   never adopted as "the file wins".
//! - **`offline.doc.replica`** — the device keeps each document it has
//!   opened, synced with the server's document over `DocSync`, with what
//!   the server has not acknowledged tracked honestly ([`replica`]).
//!
//! Wasm-clean and Dioxus-free on purpose: the server's mirror
//! (`vault-collab`), the shell's editors, a desktop folder writer +
//! watcher and the records' task/capture documents all use the same
//! code. File identity: a document's `id` prop ([`ID_PROP`]) names it
//! across moves and renames; nothing here invents one, but nothing
//! drops one either.

pub mod body;
pub mod book;
pub mod doc;
pub mod markdown;
pub mod peer;
pub mod replica;

pub use book::{Book, Entry, Index, IndexRow, MemoryIndex, Projected, Projector, SyncReport};
pub use doc::{
    Merged, SEED_ORIGIN, adopt_text, commit_as, is_deleted, merge_text, migrate_whole_file, render,
    seed_from_file, set_deleted,
};
pub use markdown::{Front, Parsed, Props, is_mid_edit, materialise, parse};
pub use replica::{
    FOREIGN_LINEAGE, Replica, SessionEnd, SessionEvent, SharedStore, SyncLane, is_foreign_lineage,
};

/// A host that keeps authored records as documents and can write one
/// **through** its document now — the server's vault-collab registry.
/// What a service that thinks in whole records (`TaskService`, the inbox)
/// calls instead of writing the file (`offline.doc.materialise`): the
/// edit is merged into the document as a change, and the file is
/// rendered from the document before the call returns, so a reader of
/// the file right after sees it, and every concurrent change the
/// document holds (a device's, synced a moment ago) is in it too.
///
/// Sync on purpose: the services are sync traits run on the blocking
/// pool.
pub trait WriteThrough: Send + Sync {
    /// Merge `new` (made from `base`; `None` — make the document read as
    /// `new`, for a record being created) into `path`'s document as a
    /// change attributed to `origin`, write the file from the document,
    /// and return the text written.
    fn write(
        &self,
        path: &str,
        origin: &str,
        base: Option<&str>,
        new: &str,
    ) -> Result<String, String>;
}

/// The prop that names a document independently of its path — what a
/// folder watcher recognises a moved file by.
pub const ID_PROP: &str = "id";
