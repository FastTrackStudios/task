//! Inline reference value types extracted from block markdown.
//!
//! These are not full architect entities — they live inside `Block`
//! as a serialized `refs_json` blob. The parent crate owns the
//! `Block.refs_json` field; this module owns the shape.
//!
//! Round-trip rule: each variant carries an `original` string with
//! the exact span from the source markdown so serialize-back is
//! byte-identical for unchanged blocks.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// `[[Page]]` / `[[Page#Heading]]` / `[[Page#^block-id]]` / `[[Page|alias]]`.
#[derive(::facet::Facet, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LinkRef {
    pub target_linkpath: String,
    pub heading: Option<String>,
    pub block_id: Option<String>,
    pub alias: Option<String>,
    pub original: String,
}

/// `![[…]]` — same shape as `LinkRef` but indicates an embed.
#[derive(::facet::Facet, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EmbedRef {
    pub target_linkpath: String,
    pub heading: Option<String>,
    pub block_id: Option<String>,
    pub alias: Option<String>,
    pub original: String,
}

/// `#nested/tag` — `path` is the slash-split components.
#[derive(::facet::Facet, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TagRef {
    pub path: Vec<String>,
    pub original: String,
}

/// Our extension: `[[entity://kind/uuid]]` — typed reference to a
/// row in any other feature's CRDT (Task, Project, Person, …).
/// Lets a knowledge page mention live entities without baking the
/// other feature's schema into knowledge-proto.
#[derive(::facet::Facet, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EntityRef {
    pub kind: String,
    pub id: Uuid,
    pub display: Option<String>,
}

/// `((uuid))` — Logseq-style direct block reference. The target is a
/// `Block.id`; an `alias` lets the source span override the displayed
/// text. `original` carries the exact source span (incl. parens) for
/// byte-identical round-trip.
#[derive(::facet::Facet, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockRef {
    pub target_block_id: Uuid,
    pub alias: Option<String>,
    pub original: String,
}

/// `[text](url)` / `![alt](src)` — standard Markdown link or image.
/// Obsidian indexes these the same as `[[wikilinks]]` for the
/// backlink / orphan / unresolved-link graph, so we extract them
/// alongside the wikilink variants. `is_embed` distinguishes
/// `![…](…)` (image / file embed) from `[…](…)` (plain link).
#[derive(::facet::Facet, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MarkdownLinkRef {
    /// Raw URL / path as written. Vault-relative or absolute or
    /// external (`http://…`). Resolution against the vault is the
    /// caller's job.
    pub url: String,
    pub alias: Option<String>,
    pub is_embed: bool,
    pub original: String,
}

#[derive(::facet::Facet, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Ref {
    Link(LinkRef),
    Embed(EmbedRef),
    Tag(TagRef),
    Entity(EntityRef),
    BlockRef(BlockRef),
    MarkdownLink(MarkdownLinkRef),
}
