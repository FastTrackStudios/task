//! The **Assets tier** — a shelf inside the vault, not a second vault.
//!
//! ADR 0004 decision 1. This module holds the whole of the tier's
//! addressing and recognition, and it is deliberately tiny, because the
//! tier is deliberately not a subsystem.
//!
//! # Why this lives in `vault-proto`
//!
//! Because that is the claim. ADR 0004 says an asset is a vault item
//! mechanically and a separate shelf conceptually: *"Being vault items
//! is not an implementation detail to be hidden: it is precisely what
//! buys them collaboration, tags, wikilinks and search without building
//! any of it twice."* Putting the tier's constants in the vault's own
//! wire contract — beside [`crate::PageMeta`] and [`crate::IfMatch`] —
//! makes that structural rather than aspirational. A future reader
//! looking for "where is the asset store?" finds this file and learns
//! there isn't one.
//!
//! Per-kind concerns (a chart's path, a chart's source encoding) live
//! with the lane that owns the kind — `resources_proto::assets` for
//! charts — and re-export from here. Task knows about a shelf; the app
//! knows what it put on it.
//!
//! # What went wrong under ADR 0003, precisely
//!
//! 0003 filed charts under `<org>/resources/`, and the reasoning was
//! about *reach*: `SourceKind::Resource` subscriptions and the
//! `/org/{slug}/media/{*path}` route already served that tree, so a
//! chart put there could be named from another organisation. The
//! consequence was that a chart got none of what makes a note a note.
//! `resources`' own module doc says it plainly — "the resources tier
//! isn't the vault" — so `upsert_chart` was a bare `std::fs::write`:
//! no CRDT document, no collaborative editing, no wikilinks, no tags,
//! no search, no presence in Task's own UI.
//!
//! Every one of those already exists, once, for vault files.
//! `vault-collab` keys a Loro document by `(vault_id, path)` for
//! **any** path under a vault root; the graph indexes any `.md` under
//! it; the note editor opens it. So the entire content of "make charts
//! collaborative" is: *put the file in the vault*. Nothing else is
//! built. If a future reader finds a parallel store, a second sync
//! path, or an asset-shaped persistence layer, it was added against the
//! grain of this decision.
//!
//! # Where they live, and why the convention is a directory
//!
//! `<vault>/Assets/<Kind>/…` — for charts, `Assets/Charts/<slug>.md`.
//!
//! A directory rather than a naming scheme or a database column,
//! because the organising convention has to be legible to the two
//! readers that matter and neither of them can run code: a person
//! looking at the folder in Obsidian, and `git diff`. "Assets are more
//! outside the Vault conceptually" is a statement about how a human
//! files things, and a folder is how humans file things.
//!
//! An application never guesses the path — it reads the `rel_path` the
//! upsert RPC hands back. The constants are public so the *server*'s
//! own pieces (migration, seed, node resolution, the UI's exclusion
//! rule) spell the directory once.
//!
//! ## The name collides with `<org>/Assets`, and that is tolerable
//!
//! An org root already has an `Assets` area — `files_domain::layout`'s
//! name for the org's loose files, a sibling of `vault/` rather than a
//! folder inside it. So a deployment now has both `<org>/Assets/` and
//! `<org>/vault/Assets/`, and the org-tree browser shows the word twice.
//!
//! Kept anyway, for two reasons. The org already uses "assets" to mean
//! *the things you will want later*, which is exactly what ADR 0004
//! means by the tier — a second word would be a second concept where
//! there is one. And the two are never confusable by address: one is
//! reached through the files lanes by `RootId`, the other through
//! `VaultSync` by `vault_id` + path, and no call site can take one for
//! the other. Flagged here so nobody rediscovers it as a bug.
//!
//! # How an asset is recognised
//!
//! Two facts, answering different questions:
//!
//! - **Frontmatter** [`TYPE_KEY`]`: `[`TYPE_ASSET`] plus [`KIND_KEY`] —
//!   *what this file is*. It travels with the file, so a chart dragged
//!   out of `Assets/` into `Notes/` is still a chart and still parses
//!   as one. A person's filing mistake is not data loss.
//! - **The `Assets/` prefix** ([`is_asset_path`]) — *where the tier
//!   keeps them*. It is what listing, walking and excluding key on,
//!   because those operations hold a path and have not opened the file.
//!
//! ## A song and its default arrangement share a basename
//!
//! Deliberately: `song:opening-night` and `chart:opening-night` are the
//! song and the chart you get when you ask for "the chart" of it, and
//! naming them the same is what makes both readable. They are different
//! *nodes* — a `NodeRef` carries its kind — so nothing in the link
//! graph is ambiguous.
//!
//! What is ambiguous is a bare `[[opening-night]]` wikilink, which
//! resolves by basename. That is not a new failure mode (any vault with
//! `Projects/X.md` and `Records/X.md` has it), and the answer is the
//! same: write the kind-qualified reference — `song:opening-night` —
//! when you mean one of them in particular. The seeded chart says so in
//! its own body, which is the only place a person will meet it.
//!
//! # What the UI does with them
//!
//! Task's UI keeps assets out of the surfaces that mean "my notes" (the
//! explorer tree) and leaves them in the surfaces that mean "anything
//! in this vault" — search, tags, backlinks, the graph, `[[wikilink]]`
//! autocomplete. Being reachable by those is the entire point of the
//! move; being listed beside a journal entry is what the shelf exists
//! to prevent.

/// The vault directory that holds every asset, relative to the vault
/// root. One segment, capitalised like the rest of a hand-filed vault.
pub const ASSETS_DIR: &str = "Assets";

/// Frontmatter key naming what kind of page this is.
pub const TYPE_KEY: &str = "type";

/// The [`TYPE_KEY`] value that says "this page is an asset". Task's UI
/// keys its notes-surface exclusion on exactly this string.
pub const TYPE_ASSET: &str = "asset";

/// Frontmatter key naming which kind of asset a page is.
///
/// Deliberately **not** `resource_kind`: the tier changed, and a page
/// that still says `resource_kind` is a page the migration has not
/// reached yet — which is a fact worth being able to see from the file.
pub const KIND_KEY: &str = "asset_kind";

/// Whether a vault-relative path is in the Assets tier.
///
/// Prefix-shaped rather than frontmatter-shaped because the callers are
/// walkers and filters that hold a path and have not read the bytes.
#[must_use]
pub fn is_asset_path(rel_path: &str) -> bool {
    rel_path
        .trim_start_matches('/')
        .strip_prefix(ASSETS_DIR)
        .is_some_and(|rest| rest.starts_with('/'))
}

impl crate::PageMeta {
    /// Whether this page is an Assets-tier item, and therefore belongs
    /// on the shelf rather than in a list of notes.
    ///
    /// Either fact is enough. The frontmatter is authoritative about
    /// *what the file is*; the path is authoritative about *where the
    /// tier keeps it*. Accepting both means an asset a person filed by
    /// hand and an asset whose frontmatter a person mangled are each
    /// still recognisable, and a UI filter never has to decide which
    /// half to trust.
    #[must_use]
    pub fn is_asset(&self) -> bool {
        self.page_type.eq_ignore_ascii_case(TYPE_ASSET) || is_asset_path(&self.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prefix_is_a_directory_and_not_a_name() {
        assert!(is_asset_path("Assets/Charts/doxology.md"));
        assert!(is_asset_path("/Assets/Charts/doxology.md"));
        assert!(
            !is_asset_path("Assets.md"),
            "a note *named* Assets is not the tier"
        );
        assert!(
            !is_asset_path("Notes/Assets/thing.md"),
            "the tier is at the vault root, not wherever the word appears"
        );
    }

    fn page(path: &str, page_type: &str) -> crate::PageMeta {
        crate::PageMeta {
            path: path.into(),
            basename: String::new(),
            title: String::new(),
            page_type: page_type.into(),
            folder: String::new(),
            tags: Vec::new(),
            icon: String::new(),
            sha256: String::new(),
            aliases: Vec::new(),
        }
    }

    /// Either half recognises an asset — the filing or the declaration.
    #[test]
    fn an_asset_is_recognised_by_frontmatter_or_by_shelf() {
        assert!(page("Assets/Charts/d.md", "asset").is_asset());
        assert!(
            page("Notes/d.md", "asset").is_asset(),
            "a chart filed somewhere else is still a chart"
        );
        assert!(
            page("Assets/Charts/d.md", "").is_asset(),
            "a file on the shelf is on the shelf"
        );
        assert!(!page("Notes/journal.md", "").is_asset());
    }
}
