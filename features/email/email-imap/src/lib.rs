//! IMAP backend. `async-imap` at the runtime edge driving the
//! TCP/TLS connection + state machine; an internal [`op`] layer
//! describes the high-level operations the backend supports
//! (List folders, Fetch envelopes, …) as plain data so they're
//! unit-testable against recorded transcripts without a socket.
//!
//! Phase-2 scope (this crate):
//! - `accounts()` — echo the configured account
//! - `list_folders()` — `LIST "" "*"`
//! - `fetch_envelopes()` — `UID FETCH … (UID FLAGS RFC822.SIZE
//!   BODY.PEEK[HEADER])` parsed via `mail-parser`
//! - `fetch_message()` — `UID FETCH … BODY.PEEK[]`
//! - Writes return `Unsupported` for now — phase 3.
//! - `send` returns `Unsupported` ("use `email-smtp`").
//! - `subscribe` is wired through a per-account broadcast
//!   channel; the IDLE attachment lands later.
//!
//! **Steal #1 from himalaya:** the internal `Op` enum is the
//! seam pimalaya/io-imap turns into a full sans-io coroutine.
//! For now we just use it as a typed boundary between "what we
//! ask IMAP to do" and "how it actually happens." Next pass
//! swaps the `async-imap` runtime adapter for `imap-codec`
//! without disturbing callers.

#![cfg(not(target_arch = "wasm32"))]

mod backend;
mod connect;
mod op;
mod parse;

pub use backend::Backend;
pub use op::{Op, OpResult};
