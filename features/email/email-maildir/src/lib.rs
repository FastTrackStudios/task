//! Local Maildir backend. Read/write a Maildir tree on disk.
//! First backend implemented — lets the rest of the stack
//! develop against a fixture mailbox with no network.
//!
//! Layout convention:
//! - Each account maps to one root directory.
//! - The root itself is the **INBOX** maildir
//!   (`<root>/{cur,new,tmp}`).
//! - Subfolders use the Maildir++ convention: a sibling
//!   directory whose name starts with `.`. Hierarchy is encoded
//!   in the name with `.` separators
//!   (`.Lists.rust-users`).
//!
//! The read side (`list_folders` / `fetch_envelopes` /
//! `fetch_message` / `fetch_attachment`) walks the tree
//! directly. `send` composes an outbound [`Submit`] transport
//! (SMTP via `email-smtp` in production) with a Sent-folder
//! maildir write + a `NewMessage` event on the changes stream.
//! `set_flags` / `move_message` / `delete_message` are
//! implemented against the filename conventions: flags live in
//! the `:2,<FLAGS>` suffix and only exist under `cur`, so each
//! mutation promotes a message out of `new` first (which is what
//! "mark as read" means here). Each publishes its
//! `FlagsChanged` / `Moved` / `Deleted` event, so the `/email`
//! page updates without a refetch.
//!
//! `append_draft` still returns
//! [`email_proto::EmailSyncError::Unsupported`] — the outbox in
//! `email-product` is the drafting path today. `subscribe` is
//! wired through a per-account broadcast channel, but the FS
//! watcher attachment lands later (mirrors
//! `vault::sync::Backend::start_watcher`), so *externally*
//! delivered mail still needs a manual refresh.

#![cfg(not(target_arch = "wasm32"))]

mod backend;
mod folder;
mod parse;
mod submit;

pub use backend::{AccountEntry, Backend};
pub use folder::FolderName;
pub use submit::Submit;
