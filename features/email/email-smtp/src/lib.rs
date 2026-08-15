//! SMTP submission. Backends compose [`SmtpSender`] to satisfy
//! the `send` end of `email_proto::EmailSync`.
//!
//! Two layers:
//! - [`build_message`] turns an `email_proto::Draft` into an
//!   RFC5322 byte vector via `mail-builder`. Wasm-clean, pure
//!   data — `email-imap`'s `append_draft` reuses this to push
//!   sent copies into the IMAP `Sent` folder.
//! - [`SmtpSender`] takes the bytes + the recipient list and
//!   submits over `mail-send`. Native-only (TLS + sockets).
//!
//! Threading headers (`In-Reply-To`, `References`) come straight
//! from the draft — the wizard / compose UI is responsible for
//! populating them when crafting a reply.

#![cfg(not(target_arch = "wasm32"))]

mod build;
mod sender;

pub use build::{BuildError, build_message};
pub use sender::{SendError, SmtpSender};
