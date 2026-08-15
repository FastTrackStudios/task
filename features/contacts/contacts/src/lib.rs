//! `contacts` — markdown round-trip + disk-backed backend.
//!
//! Sits on top of `contacts-proto`'s wire types. Three halves:
//!
//! 1. **Markdown ↔ proto** — [`parse`] + [`write`] round-trip a
//!    [`contacts_proto::Contact`] as a plain markdown file. Frontmatter
//!    carries the structured (vCard-mapped) fields; the body holds the
//!    free-form notes.
//! 2. **Backend** — [`VaultContacts`] impls the
//!    [`contacts_proto::Contacts`] capability against a real vault
//!    directory (contacts under `Records/contacts/`, sync accounts
//!    under `Records/contacts/.sync/`), ready to mount as a vox service.
//! 3. **CardDAV import** — [`carddav`] does the one-way pull (principal
//!    discovery → addressbook-query → vCard parse → upsert). Wired into
//!    [`VaultContacts::sync_account`].
//!
//! Native-only because the parse/write paths touch the disk and sync
//! hits the network. The UI crate builds for wasm and uses
//! `contacts-proto` directly.

#![cfg(not(target_arch = "wasm32"))]

pub mod carddav;
pub mod parse;
pub mod vault_contacts;
pub mod write;

pub use parse::{ParseError, parse_contact};
pub use vault_contacts::{VaultContacts, VaultContactsError};
pub use write::{WriteError, serialize_contact};

pub use contacts_proto::*;
