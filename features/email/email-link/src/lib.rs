//! Cross-entity link layer for the email feature.
//!
//! Connects a message (identified by Message-ID) to any entity
//! elsewhere in Task — a Task, a Project, a Note, a Person.
//! Many-to-many: one email may relate to several projects; one
//! project's `emails:` frontmatter list may name many messages.
//!
//! Storage shape: **frontmatter is canonical, index is
//! disposable.** When a user links a message to a Project, we
//! write the message-id into the project's `.md` frontmatter
//! under `emails:` (mirrors Obsidian's convention) AND insert a
//! row into the `email_links` `SQLite` table for fast reverse
//! lookup. The vault file is the source of truth — rebuild the
//! index by walking the vault if it goes stale.
//!
//! Wikilink form: `[[email://<message-id>|Subject text]]`.
//! Hand-editable in markdown; the parser tolerates encoded +
//! bare Message-IDs interchangeably.

#![cfg(not(target_arch = "wasm32"))]

mod backend;
mod entity;
mod error;
mod frontmatter;
mod link;
mod parse;
mod store;
mod walker;

pub use entity::{EntityKind, EntityRef};
pub use error::{LinkError, Result};
pub use frontmatter::{ParsedFrontmatter, parse_frontmatter, split};
pub use link::EmailLink;
pub use parse::{EmailWikilink, format_wikilink, parse_wikilinks};
pub use store::LinkStore;
pub use walker::{EntityResolver, WalkedFile, collect_links, default_resolver, walk_vault};

pub use backend::LinkBackend;
