//! `inbox` — markdown round-trip + disk-backed backend.
//!
//! Sits on top of `inbox-proto`'s wire types. Two halves:
//!
//! 1. **Markdown ↔ proto** — [`parse`] + [`write`] round-trip an
//!    [`inbox_proto::InboxItem`] as a plain markdown file. Frontmatter
//!    carries the structured fields; the body is the verbatim capture.
//! 2. **Backend** — [`VaultInbox`] impls the [`inbox_proto::Inbox`]
//!    capability against a real vault directory, ready to mount as a
//!    vox service.
//!
//! Native-only because the parse/write paths touch the disk. The UI
//! crate builds for wasm and uses `inbox-proto` directly.

#![cfg(not(target_arch = "wasm32"))]

pub mod parse;
pub mod vault_inbox;
pub mod write;

pub use parse::{ParseError, parse_inbox_item};
pub use vault_inbox::{VaultInbox, VaultInboxError};
pub use write::{WriteError, serialize_inbox_item};

pub use inbox_proto::*;
