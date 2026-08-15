//! `recall` — markdown round-trip + disk-backed backend.
//!
//! Sits on top of `recall-proto`'s wire types. Two halves:
//!
//! 1. **Markdown ↔ proto** — [`parse`] + [`write`] round-trip a
//!    [`recall_proto::RecallCard`] as a plain markdown file. Frontmatter
//!    carries the structured fields (FSRS state under `sr-*` keys); the
//!    body holds the front/back.
//! 2. **Backend** — [`VaultRecall`] impls the [`recall_proto::Recall`]
//!    capability against a real vault directory, ready to mount as a
//!    vox service.
//!
//! Native-only because the parse/write paths touch the disk. The UI
//! crate builds for wasm and uses `recall-proto` directly.

#![cfg(not(target_arch = "wasm32"))]

pub mod parse;
pub mod vault_recall;
pub mod write;

pub use parse::{ParseError, parse_recall_card};
pub use vault_recall::{VaultRecall, VaultRecallError};
pub use write::{WriteError, serialize_recall_card};

pub use recall_proto::*;
