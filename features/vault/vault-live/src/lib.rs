//! Filesystem-backed vault.
//!
//! Native-only crate. `.md` files on disk are authoritative; this
//! crate provides:
//!
//! - [`Vault`]: an in-memory snapshot of a vault root, rebuildable
//!   from disk in one pass.
//! - [`BlockIndex`]: a `uuid → (page, byte offset)` map covering
//!   every `id:: <uuid>` block-id property line across the vault.
//!   Enables cross-page block refs (`((uuid))`) and embeds.
//! - [`watch`]: a debounced filesystem watcher that emits change
//!   events the editor can use to refresh state.
//!
//! Architectural shape mirrors [`vault-obsidian`]: small
//! focused modules, native-only `cfg`, vault snapshot +
//! walker + watcher + a thin mutate layer for create /
//! append / save. The difference is block-awareness:
//! every vault page is parsed for its `id:: <uuid>` lines,
//! and the [`BlockIndex`] gives O(1) cross-vault lookup of
//! a block's containing page and offset.
//!
//! Compared to the older [`editor-outliner::db::Vault`] this
//! replaces (still in-tree under `crates/editor-outliner/` until
//! its consumers migrate):
//!
//! - File content is canonical; we don't materialize a separate
//!   `Block` table parallel to the markdown source. The block
//!   index points back into the file.
//! - Pages can live anywhere under the vault root (matches
//!   Obsidian's free-form folders), not just `pages/` and
//!   `journals/`.
//! - Block-property storage is the file itself (`id:: <uuid>`,
//!   `key:: value` lines under a block), not a `properties_json`
//!   blob.
//! - `editor-outliner` made the page-and-block tables the source
//!   of truth in memory and re-serialized markdown on save —
//!   reading round-trips were lossy. Here, raw file bytes ARE the
//!   doc; rewrites touch only the byte ranges we identified.

#![cfg(not(target_arch = "wasm32"))]

// Format-agnostic file parsers, migrated out of the old
// `knowledge-proto` crate so vault-live owns the file-format
// layer it loads:
//   bases           — `.base` query DSL parser
//   refs            — wikilink / blockref / tag value types
//   property_schema — frontmatter type coercion (Obsidian's
//                      `.obsidian/types.json` shape)
//   lexorank        — fractional-index sort keys
// `knowledge-proto` keeps thin compat re-exports for now;
// they go away in the slice-3 server rip.
pub mod bases;
pub mod blocks;
pub mod cook;
pub mod lexorank;
pub mod lookup;
pub mod mutate;
pub mod property_schema;
pub mod refs;
/// `VaultSync` backend — canonical filesystem impl of the
/// [`vault_proto::VaultSync`] wire trait. Consumers use
/// `vault::sync::Backend` (also re-exported as
/// [`Backend`]).
pub mod sync;
pub mod vault;
pub mod walker;
pub mod watcher;

pub use blocks::{BlockIndex, BlockLocation};
pub use lookup::VaultLookupView;
pub use mutate::{MutateError, append_to_page, create_page, delete_page, save_page};
pub use sync::Backend;
pub use vault::{LoadError, SaveError, Vault, VaultPage};
pub use vault::{PropertyTypes, VaultBase};
pub use walker::{VaultEntry, VaultEntryKind, walk_vault};
pub use watcher::{FsEvent, VaultEvent, WatchError, watch, watch_any};
