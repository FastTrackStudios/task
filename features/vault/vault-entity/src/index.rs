//! The vault's read index — `vault.index.*`, `storage.query.no-scan`.
//!
//! # What this replaces
//!
//! `VaultEntityStore::get_by_uuid` resolved through `find` → `list` →
//! `scan`: a lookup by id iterated every page in the vault and re-parsed
//! every page of that type, discarding all but one. The vault spec's
//! preamble names that as the behaviour its rules exist to replace, and
//! this is the replacement.
//!
//! Two structures, and they answer different rules:
//!
//! - **`parsed`** — a page's typed model, keyed by the *content* it was
//!   parsed from. `vault.index.parse-once`: "the cache is keyed by
//!   content, not by clock, so an unchanged file is never re-parsed and a
//!   changed one is never served stale". A clock-keyed cache — mtime, a
//!   TTL, a generation counter — gets both halves of that wrong on a
//!   filesystem with second-granularity timestamps or a clock that moved.
//! - **`by_id`** — id to path, per entity type. `vault.index.lookup`:
//!   resolving one page costs one hash lookup and one parse, so the cost
//!   is proportional to the result rather than to the vault, and adding
//!   ten thousand pages of an unrelated type changes neither.
//!
//! # Type-erased, because one vault holds every type
//!
//! A vault carries projects, tasks, goals, milestones and the rest, and
//! `VaultEntityStore<E>` is one view over it per type. Several such views
//! share one `Vault` through `from_shared`, so they have to share one
//! index too — otherwise each type keeps its own copy of the same pages
//! and "parsed once" becomes "parsed once per type that looked".
//!
//! So models are held as `Arc<dyn Any + Send + Sync>` and downcast on
//! the way out. The alternative — an index per type — is the same map
//! with a worse hit rate.
//!
//! # Incremental by construction
//!
//! [`PageIndex::forget`] drops one path. Nothing here walks the vault,
//! and nothing invalidates more than the page it was told about, which
//! is `vault.index.incremental`: "a change to one file re-indexes that
//! file, and no edit triggers a full rescan".

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use uuid::Uuid;

/// A page's parsed models, and the content they came from.
#[derive(Default)]
struct Parsed {
    /// Hash of the page's raw bytes when these were parsed.
    ///
    /// A hash rather than the bytes: holding a second copy of every page
    /// would double the vault's memory to save a comparison that is
    /// already fast.
    content: u64,
    /// One model per entity type that has read this page. Usually one —
    /// a page is a project or a task, not both — but the map costs
    /// nothing when it holds one entry and keeps `matches` from being
    /// consulted twice.
    models: HashMap<TypeId, Arc<dyn Any + Send + Sync>>,
}

/// Parsed models and identity lookups over one vault.
///
/// Shared by every [`crate::store::VaultEntityStore`] over the same
/// pages — see the module docs on why it is type-erased.
#[derive(Default)]
pub struct PageIndex {
    parsed: HashMap<String, Parsed>,
    /// entity type → id → the page that declares it.
    by_id: HashMap<TypeId, HashMap<Uuid, String>>,
}

/// The content key a page is cached against.
#[must_use]
pub fn content_key(raw: &str) -> u64 {
    let mut h = DefaultHasher::new();
    raw.hash(&mut h);
    h.finish()
}

impl PageIndex {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The cached model for `path`, if it was parsed from this content.
    ///
    /// `None` when the page has not been read, was read as another type,
    /// or has changed since — and the last of those is the whole point:
    /// a changed page misses, so it is never served stale.
    pub fn cached<M: Any + Send + Sync>(&self, path: &str, raw: &str) -> Option<Arc<M>> {
        let entry = self.parsed.get(path)?;
        if entry.content != content_key(raw) {
            return None;
        }
        entry
            .models
            .get(&TypeId::of::<M>())
            .and_then(|m| Arc::clone(m).downcast::<M>().ok())
    }

    /// Remember a parse, and the id it resolves under.
    ///
    /// Replacing a page's entry drops the models parsed from its old
    /// content, which is what keeps the cache bounded by the vault
    /// rather than by how many times it has been edited.
    pub fn remember<M: Any + Send + Sync>(
        &mut self,
        path: &str,
        raw: &str,
        id: Uuid,
        model: Arc<M>,
    ) {
        let key = content_key(raw);
        let entry = self.parsed.entry(path.to_owned()).or_default();
        if entry.content != key {
            entry.content = key;
            entry.models.clear();
        }
        entry.models.insert(TypeId::of::<M>(), model);
        self.by_id
            .entry(TypeId::of::<M>())
            .or_default()
            .insert(id, path.to_owned());
    }

    /// Where the page declaring `id` lives, for this entity type.
    ///
    /// One hash lookup. The caller still parses that page — and only
    /// that page — which is what makes the cost proportional to the
    /// result.
    #[must_use]
    pub fn path_of<M: Any>(&self, id: Uuid) -> Option<&str> {
        self.by_id
            .get(&TypeId::of::<M>())?
            .get(&id)
            .map(String::as_str)
    }

    /// Forget one page — the incremental invalidation.
    ///
    /// Called when a page is written, deleted, or observed to have
    /// changed. Costs one map removal plus the id entries that pointed
    /// at it; nothing else in the index is touched.
    pub fn forget(&mut self, path: &str) {
        self.parsed.remove(path);
        for ids in self.by_id.values_mut() {
            ids.retain(|_, at| at != path);
        }
    }

    /// Forget a page under its new name as well as its old.
    pub fn rename(&mut self, from: &str, to: &str) {
        self.forget(from);
        self.forget(to);
    }

    /// How many pages are cached. For tests and diagnostics.
    #[must_use]
    pub fn len(&self) -> usize {
        self.parsed.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.parsed.is_empty()
    }
}

impl std::fmt::Debug for PageIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PageIndex")
            .field("pages", &self.parsed.len())
            .field("types", &self.by_id.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    struct Note(&'static str);

    #[test]
    fn a_page_parsed_from_this_content_is_reused() {
        let mut ix = PageIndex::new();
        let id = Uuid::new_v4();
        ix.remember("a.md", "hello", id, Arc::new(Note("hello")));

        let hit: Arc<Note> = ix.cached("a.md", "hello").expect("a hit");
        assert_eq!(*hit, Note("hello"));
    }

    /// The half that matters: a changed page must miss.
    ///
    /// Keyed by content and not by clock, so this holds on a filesystem
    /// with second-granularity mtimes and across a clock that moved
    /// backwards — both of which happen.
    #[test]
    fn a_changed_page_is_never_served_stale() {
        let mut ix = PageIndex::new();
        let id = Uuid::new_v4();
        ix.remember("a.md", "hello", id, Arc::new(Note("hello")));

        assert!(
            ix.cached::<Note>("a.md", "hello, again").is_none(),
            "the cache answered for content it was not parsed from"
        );
    }

    #[test]
    fn an_id_resolves_to_one_page() {
        let mut ix = PageIndex::new();
        let id = Uuid::new_v4();
        ix.remember("deep/nested/a.md", "x", id, Arc::new(Note("x")));
        assert_eq!(ix.path_of::<Note>(id), Some("deep/nested/a.md"));
        assert_eq!(ix.path_of::<Note>(Uuid::new_v4()), None);
    }

    /// Two types over one page each keep their own model.
    #[test]
    fn one_page_can_be_two_types_without_either_evicting_the_other() {
        #[derive(Debug, PartialEq)]
        struct Task(&'static str);

        let mut ix = PageIndex::new();
        let id = Uuid::new_v4();
        ix.remember("a.md", "x", id, Arc::new(Note("as a note")));
        ix.remember("a.md", "x", id, Arc::new(Task("as a task")));

        assert_eq!(
            *ix.cached::<Note>("a.md", "x").expect("note"),
            Note("as a note")
        );
        assert_eq!(
            *ix.cached::<Task>("a.md", "x").expect("task"),
            Task("as a task")
        );
    }

    /// Forgetting one page forgets exactly one page.
    #[test]
    fn invalidation_costs_one_page() {
        let mut ix = PageIndex::new();
        let (a, b) = (Uuid::new_v4(), Uuid::new_v4());
        ix.remember("a.md", "x", a, Arc::new(Note("a")));
        ix.remember("b.md", "y", b, Arc::new(Note("b")));

        ix.forget("a.md");

        assert!(ix.cached::<Note>("a.md", "x").is_none());
        assert_eq!(ix.path_of::<Note>(a), None, "the id entry went with it");
        assert!(
            ix.cached::<Note>("b.md", "y").is_some(),
            "and nothing else did"
        );
        assert_eq!(ix.path_of::<Note>(b), Some("b.md"));
    }

    /// A page re-parsed from new content drops the old model rather than
    /// accumulating one per edit.
    #[test]
    fn re_remembering_a_page_replaces_rather_than_grows() {
        let mut ix = PageIndex::new();
        let id = Uuid::new_v4();
        ix.remember("a.md", "one", id, Arc::new(Note("one")));
        ix.remember("a.md", "two", id, Arc::new(Note("two")));

        assert_eq!(ix.len(), 1);
        assert!(ix.cached::<Note>("a.md", "one").is_none());
        assert_eq!(*ix.cached::<Note>("a.md", "two").expect("hit"), Note("two"));
    }
}
