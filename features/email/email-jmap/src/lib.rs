//! JMAP backend. Wraps `stalwartlabs/jmap-client` to implement
//! `email_proto::EmailSync`. Works against Fastmail (the
//! reference deployment) and any RFC 8620 + 8621 server —
//! Stalwart, Apache James 4+, the Cyrus + Topicbox stack.
//!
//! Folder aliases are honored at the wire boundary, same shape
//! as `email-imap` and `email-maildir`. The backend matches JMAP
//! mailbox IDs by the server-given name through the alias map.
//!
//! Phase-1 scope:
//! - `accounts()` — echo the configured account.
//! - `list_folders()` — `Mailbox/get` of every mailbox.
//! - `fetch_envelopes()` — `Email/query` + `Email/get` for the
//!   selected mailbox, mapped to `Envelope`.
//! - `fetch_message()` — `Email/get` with full body parts +
//!   `blobId` resolution for the raw RFC822.
//! - `fetch_attachment()` — `Email/get` + `Blob/download`.
//! - Writes (`set_flags`, `move_message`, `delete_message`,
//!   `append_draft`, `send`) return `Unsupported` for now; the
//!   `Email/set` shape lands next.
//! - `subscribe()` is wired through a per-account broadcast
//!   channel — the eventual `EventSource` attachment plugs in
//!   the same way `email-imap::start_idle` does for IMAP.

#![cfg(not(target_arch = "wasm32"))]

mod backend;

pub use backend::Backend;
