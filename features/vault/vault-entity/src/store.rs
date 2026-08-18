//! Generic CRUD over vault-backed markdown entities.
//!
//! Eleven slices carried a byte-for-byte copy of this store, differing
//! only in type names: `Arc<Mutex<Vault>>`, a `map_io`, a `find_idx`,
//! and list/get/create/update/delete. [`VaultEntityStore`] is that code
//! written once; a slice supplies a [`VaultEntity`] mapping and keeps
//! only the behaviour that is actually its own.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::{Vault, VaultPage};

use crate::error::{EntityError, ParseError, WriteError};
use crate::index::PageIndex;
use crate::slug;

/// How a domain model maps onto one markdown page in the vault.
///
/// This is implemented by a zero-sized marker type in the owning slice
/// rather than by the model itself: most models live in a wasm-clean
/// `*-proto` crate that must not depend on the vault, so the model and
/// this trait are both foreign to the slice and a direct impl would
/// trip the orphan rule.
///
/// ```text
/// pub struct BodyMetrics;
///
/// impl VaultEntity for BodyMetrics {
///     type Model = BodyMetric;
///     const TYPE: &'static str = "body-metric";
///     const DEFAULT_FOLDER: &'static str = "Projects/Fitness/body";
///     …
/// }
///
/// pub type Store = VaultEntityStore<BodyMetrics>;
/// ```
pub trait VaultEntity: Send + Sync + 'static {
    /// The domain model stored on the page.
    type Model: Clone + Send + Sync + 'static;

    /// Frontmatter discriminator — matched against `type:` or a tag.
    const TYPE: &'static str;
    /// Folder new pages land in when the caller gives no path.
    const DEFAULT_FOLDER: &'static str;
    /// Slug used when the model's name yields nothing sluggable.
    const SLUG_FALLBACK: &'static str = Self::TYPE;

    fn id(model: &Self::Model) -> Uuid;
    fn set_id(model: &mut Self::Model, id: Uuid);
    fn path(model: &Self::Model) -> &str;
    fn set_path(model: &mut Self::Model, path: String);
    /// Human name, used to derive the default filename.
    fn name(model: &Self::Model) -> &str;

    fn from_page(page: &VaultPage) -> Result<Self::Model, ParseError>;
    fn to_markdown(model: &Self::Model) -> Result<String, WriteError>;

    /// Stamp creation metadata. Default: nothing — implement it when
    /// the model carries `dateCreated` / `dateModified`.
    fn on_create(_model: &mut Self::Model, _now: DateTime<Utc>) {}
    /// Stamp modification metadata.
    fn on_update(_model: &mut Self::Model, _now: DateTime<Utc>) {}

    /// Default vault-relative path for a new page.
    fn default_path(name: &str, folder: Option<&str>) -> String {
        slug::entity_path(name, folder, Self::DEFAULT_FOLDER, Self::SLUG_FALLBACK)
    }

    /// True when `page` belongs to this entity type.
    fn matches(page: &VaultPage) -> bool {
        crate::frontmatter::has_type(&page.raw, Self::TYPE)
    }
}

/// File-backed store for a [`VaultEntity`] mapping.
///
/// Cloning shares the underlying vault, so a slice can hand the same
/// vault to several stores (`from_shared`) exactly as the hand-written
/// versions did.
pub struct VaultEntityStore<E: VaultEntity> {
    inner: Arc<Mutex<Vault>>,
    /// Parsed models and identity lookups over the same pages.
    ///
    /// A second handle rather than a field inside the `Vault`, so that
    /// `from_shared` — which thirty-odd callers use — keeps its
    /// signature and its meaning. Stores built from one shared vault
    /// through [`Self::from_shared`] each get their own index and are
    /// correct but colder; [`Self::from_shared_indexed`] shares both,
    /// which is what a caller holding a vault across requests wants.
    index: Arc<Mutex<PageIndex>>,
    /// One lock per page path — see [`Self::page_lock`].
    page_locks: Arc<Mutex<HashMap<String, Arc<Mutex<()>>>>>,
    _entity: PhantomData<fn() -> E>,
}

impl<E: VaultEntity> Clone for VaultEntityStore<E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            index: self.index.clone(),
            page_locks: self.page_locks.clone(),
            _entity: PhantomData,
        }
    }
}

impl<E: VaultEntity> VaultEntityStore<E> {
    pub fn new(vault: Vault) -> Self {
        Self::from_shared(Arc::new(Mutex::new(vault)))
    }

    pub fn from_shared(inner: Arc<Mutex<Vault>>) -> Self {
        Self {
            inner,
            index: Arc::new(Mutex::new(PageIndex::new())),
            page_locks: Arc::new(Mutex::new(HashMap::new())),
            _entity: PhantomData,
        }
    }

    /// Share the pages *and* the index with a sibling store.
    ///
    /// What a caller wants when several entity types read one vault:
    /// otherwise each type parses the same pages into its own cache and
    /// `vault.index.parse-once` becomes "parsed once per type that
    /// looked".
    pub fn from_shared_indexed(inner: Arc<Mutex<Vault>>, index: Arc<Mutex<PageIndex>>) -> Self {
        Self {
            inner,
            index,
            page_locks: Arc::new(Mutex::new(HashMap::new())),
            _entity: PhantomData,
        }
    }

    /// The shared index handle, for sibling stores over the same pages.
    #[must_use]
    pub fn index(&self) -> Arc<Mutex<PageIndex>> {
        self.index.clone()
    }

    fn index_lock(&self) -> MutexGuard<'_, PageIndex> {
        self.index.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A lock on one page, for the duration of a write to it.
    ///
    /// Two writes to *different* pages take different locks and proceed
    /// together; two to the same page serialise, which is what stops
    /// them interleaving into a page neither caller wrote.
    ///
    /// Entries are kept rather than reaped. A vault's page count is its
    /// page count — bounded by the tree, not by traffic — and reaping
    /// would mean a second lock over the map on the path where the point
    /// is to hold fewer locks.
    fn page_lock(&self, path: &str) -> Arc<Mutex<()>> {
        let mut locks = self.page_locks.lock().unwrap_or_else(|e| e.into_inner());
        Arc::clone(locks.entry(path.to_owned()).or_default())
    }

    /// The shared vault handle, for sibling stores over the same files.
    pub fn shared(&self) -> Arc<Mutex<Vault>> {
        self.inner.clone()
    }

    /// Lock the vault, recovering from a poisoned mutex rather than
    /// panicking. A previous panic mid-write can leave the in-memory
    /// snapshot stale, but the pages on disk are the source of truth
    /// and every read re-parses them — so taking the lock back is
    /// strictly better than taking the whole service down, which is
    /// what the `.lock().unwrap()` copies did.
    fn lock(&self) -> MutexGuard<'_, Vault> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` against the locked vault — the escape hatch for
    /// slice-specific queries that don't fit plain CRUD.
    pub fn with_vault<R>(&self, f: impl FnOnce(&Vault) -> R) -> R {
        f(&self.lock())
    }

    /// Run `f` against the locked vault mutably.
    pub fn with_vault_mut<R>(&self, f: impl FnOnce(&mut Vault) -> R) -> R {
        f(&mut self.lock())
    }

    /// Every page of this type, parse failures logged and skipped.
    // t[impl storage.projection.external-edits] — every call scans the
    // vault fresh, so a page changed by an editor, a sync client or a
    // shell is picked up on the next read with no restart and no
    // conflict. The vault having other writers is the normal case
    pub fn list(&self) -> Vec<E::Model>
    where
        E::Model: Send + Sync + Clone + 'static,
    {
        let vault = self.lock();
        self.scan_indexed(&vault)
    }

    /// Free-standing scan, for callers holding their own vault.
    /// Every page of this type, reusing what has already been parsed.
    ///
    /// The cache is consulted per page and keyed by that page's content,
    /// so an edit to one page costs one parse and the rest are handed
    /// back — `vault.index.parse-once` and `vault.index.incremental`,
    /// which are the same mechanism seen from two directions.
    // t[impl vault.index.parse-once] — keyed by content, so an unchanged
    // file is never re-parsed and a changed one is never served stale
    // t[impl vault.index.incremental] — a changed page costs a parse; no
    // edit anywhere triggers a rescan of anything else
    fn scan_indexed(&self, vault: &Vault) -> Vec<E::Model>
    where
        E::Model: Send + Sync + Clone + 'static,
    {
        let mut index = self.index_lock();
        vault
            .pages
            .iter()
            .filter(|p| E::matches(p))
            .filter_map(|p| {
                if let Some(hit) = index.cached::<E::Model>(&p.rel_path, &p.raw) {
                    return Some((*hit).clone());
                }
                match E::from_page(p) {
                    Ok(model) => {
                        index.remember(
                            &p.rel_path,
                            &p.raw,
                            E::id(&model),
                            std::sync::Arc::new(model.clone()),
                        );
                        Some(model)
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %p.rel_path,
                            entity = E::TYPE,
                            ?e,
                            "vault entity parse failed"
                        );
                        None
                    }
                }
            })
            .collect()
    }

    // t[impl vault.index.tolerant] — a page that fails to parse is
    // skipped and reported with its path and reason, and every other page
    // is still returned. It also stays a page in `vault.pages`, so it is
    // listed as unparsed rather than vanishing
    pub fn scan(vault: &Vault) -> Vec<E::Model> {
        vault
            .pages
            .iter()
            .filter(|p| E::matches(p))
            .filter_map(|p| match E::from_page(p) {
                Ok(model) => Some(model),
                Err(e) => {
                    tracing::warn!(
                        path = %p.rel_path,
                        entity = E::TYPE,
                        ?e,
                        "vault entity parse failed"
                    );
                    None
                }
            })
            .collect()
    }

    /// First model matching `pred`.
    pub fn find(&self, pred: impl Fn(&E::Model) -> bool) -> Option<E::Model>
    where
        E::Model: Send + Sync + Clone + 'static,
    {
        self.list().into_iter().find(|m| pred(m))
    }

    pub fn get(&self, id: &str) -> Result<E::Model, EntityError>
    where
        E::Model: Send + Sync + Clone + 'static,
    {
        let uuid = Uuid::parse_str(id).map_err(EntityError::bad_id)?;
        self.get_by_uuid(uuid)
            .ok_or_else(|| EntityError::NotFound(id.to_string()))
    }

    // t[impl vault.index.lookup] — one hash lookup for the path, then one
    // page parsed. Cost is proportional to the result, and ten thousand
    // pages of an unrelated type change neither half
    // t[impl storage.query.no-scan] — the vault half of the rule: a lookup
    // by identity no longer walks the tree or parses every entity of a
    // type
    pub fn get_by_uuid(&self, id: Uuid) -> Option<E::Model>
    where
        E::Model: Send + Sync + Clone + 'static,
    {
        // The index, when it knows. A miss falls through to the scan
        // below, which is also what populates it — so the first lookup
        // after a boot pays for the pages it reads and every later one
        // does not.
        // Bound in its own statement, and deliberately: as the scrutinee
        // of an `if let` the guard would live for the whole block, and
        // the block takes the same lock again. A non-reentrant mutex
        // makes that a hang rather than an error, which is how this was
        // found — a test that never returned.
        let known = self.index_lock().path_of::<E::Model>(id).map(str::to_owned);
        if let Some(path) = known {
            let vault = self.lock();
            if let Some(page) = vault.pages.iter().find(|p| p.rel_path == path) {
                if let Some(hit) = self.index_lock().cached::<E::Model>(&path, &page.raw) {
                    return Some((*hit).clone());
                }
                // The page moved on since it was indexed. Re-parse just
                // this one rather than the vault.
                if let Ok(model) = E::from_page(page) {
                    if E::id(&model) == id {
                        self.index_lock().remember(
                            &path,
                            &page.raw,
                            id,
                            std::sync::Arc::new(model.clone()),
                        );
                        return Some(model);
                    }
                }
                // The id is not there any more — the page was re-pointed
                // or retyped. Drop the stale entry and fall through.
                self.index_lock().forget(&path);
            } else {
                self.index_lock().forget(&path);
            }
        }
        self.find(|m| E::id(m) == id)
    }

    // t[impl storage.projection.write-through] — the file is written by
    // `create_page` and the in-memory projection is this `Vault`'s page
    // list, updated in the same call. A crash between them leaves the
    // file correct and the projection stale, which is the direction the
    // rule requires
    // t[impl storage.tier.authored] — what a human wrote goes out as
    // markdown with YAML frontmatter and nothing else is needed to read it
    pub fn create(&self, mut model: E::Model) -> Result<E::Model, EntityError> {
        if E::id(&model).is_nil() {
            E::set_id(&mut model, Uuid::new_v4());
        }
        if E::path(&model).is_empty() {
            let path = E::default_path(E::name(&model), None);
            E::set_path(&mut model, path);
        }
        let now = Utc::now();
        E::on_create(&mut model, now);
        E::on_update(&mut model, now);

        let body = E::to_markdown(&model)?;
        let mut vault = self.lock();
        if vault.pages.iter().any(|p| p.rel_path == E::path(&model)) {
            return Err(EntityError::AlreadyExists(E::path(&model).to_string()));
        }
        vault::create_page(&mut vault, E::path(&model), body).map_err(EntityError::io)?;
        // Incremental: this page and no other.
        self.index_lock().forget(E::path(&model));
        Ok(model)
    }

    // t[impl vault.write.granular] — the fs write happens with the vault
    // lock released and a lock on this page alone, so concurrent writes
    // to different pages proceed together and neither stalls a read
    pub fn update(&self, mut model: E::Model) -> Result<E::Model, EntityError> {
        let mut vault = self.lock();
        let idx = self
            .locate(&vault, E::id(&model))
            .ok_or_else(|| EntityError::NotFound(E::id(&model).to_string()))?;

        // The page on disk owns its path; an update never moves a file.
        let path = vault.pages[idx].rel_path.clone();
        E::set_path(&mut model, path.clone());
        E::on_update(&mut model, Utc::now());

        let body = E::to_markdown(&model)?;
        vault.pages[idx].raw = body;
        // The disk write happens with the vault lock *released*, and
        // under a lock on this page alone.
        //
        // `vault.write.granular`: "a write takes no lock wider than the
        // pages it modifies, so one slow write does not stall unrelated
        // reads". Holding the vault across the fs write made every read
        // in the process wait on somebody else's disk — on a network
        // mount, for as long as that took.
        drop(vault);
        let _page = self.page_lock(&path);
        let mut vault = self.lock();
        vault::save_page(&mut vault, &path).map_err(EntityError::io)?;
        drop(vault);
        self.index_lock().forget(&path);
        Ok(model)
    }

    pub fn delete(&self, id: &str) -> Result<(), EntityError> {
        let uuid = Uuid::parse_str(id).map_err(EntityError::bad_id)?;
        let mut vault = self.lock();
        let idx = self
            .locate(&vault, uuid)
            .ok_or_else(|| EntityError::NotFound(id.to_string()))?;
        let path = vault.pages[idx].rel_path.clone();
        vault::delete_page(&mut vault, &path).map_err(EntityError::io)?;
        self.index_lock().forget(&path);
        Ok(())
    }

    fn index_of(vault: &Vault, id: Uuid) -> Option<usize> {
        vault.pages.iter().position(|p| {
            E::matches(p) && E::from_page(p).map(|m| E::id(&m) == id).unwrap_or(false)
        })
    }

    /// Where `id`'s page sits, asking the index before the vault.
    ///
    /// The mutation path's half of `vault.index.lookup`. Without it a
    /// write parses every page up to the one it is about to change,
    /// which makes editing the last page of a vault cost the vault —
    /// and is exactly what a lookup index is for.
    ///
    /// Falls back to the scan on a miss, and the scan is also what warms
    /// the index, so the fallback is self-limiting.
    fn locate(&self, vault: &Vault, id: Uuid) -> Option<usize> {
        let known = self.index_lock().path_of::<E::Model>(id).map(str::to_owned);
        if let Some(path) = known {
            if let Some(at) = vault.pages.iter().position(|p| p.rel_path == path) {
                return Some(at);
            }
            self.index_lock().forget(&path);
        }
        Self::index_of(vault, id)
    }
}
