//! On-disk Maildir + `SQLite` FTS5 index for the email feature.
//!
//! Per-account layout:
//! ```text
//! <account_root>/
//!   index.db               SQLite (envelopes, FTS5, threads, queue)
//!   INBOX/{cur,new,tmp}/
//!   .Sent/{cur,new,tmp}/
//!   .Drafts/{cur,new,tmp}/
//!   ...
//! ```
//!
//! The maildir is the source of truth on disk. The index is a
//! disposable cache — `Store::rebuild_from_disk` walks the
//! maildir and replays every message through `upsert_envelope`,
//! reconstructing the table from scratch.
//!
//! Same shape as the rest of Task: markdown/maildir on disk is
//! canonical, `SQLite` is the query layer.

#![cfg(not(target_arch = "wasm32"))]

mod derivations;
mod error;
mod jwz;
mod notify;
mod outbox;
mod query;
mod schema;
mod store;
mod walker;

pub use error::{Result, StoreError};
pub use jwz::{ThreadAssignment, ThreadInput, compute_threads};
pub use outbox::OUTBOX_MAX_RETRIES;
pub use query::{SearchHit, StoredEnvelope};
pub use store::Store;
pub use walker::{MailEntry, walk_account, walk_folder};
