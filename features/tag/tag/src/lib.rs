//! Tag feature backend.
//!
//! The per-org tag registry — name → icon/color decorations — persisted
//! as a single `Records/tags.json` file in the vault and served over the
//! [`tag_proto::TagService`] vox RPC. Native-only: the round-trip touches
//! disk. The web UI binds `tag-proto` directly on wasm. Mirrors the
//! `inbox` crate (minus per-item files — the registry is small, so one
//! JSON document is simpler than a file per tag).

mod vault_tags;

pub use vault_tags::{VaultTags, VaultTagsError};
