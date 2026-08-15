//! Generic CRUD over vault-backed markdown entities.
//!
//! Eleven slices carried a byte-for-byte copy of this store, differing
//! only in type names: `Arc<Mutex<Vault>>`, a `map_io`, a `find_idx`,
//! and list/get/create/update/delete. [`VaultEntityStore`] is that code
//! written once; a slice supplies a [`VaultEntity`] mapping and keeps
//! only the behaviour that is actually its own.

use std::marker::PhantomData;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use uuid::Uuid;
use vault::{Vault, VaultPage};

use crate::error::{EntityError, ParseError, WriteError};
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
    _entity: PhantomData<fn() -> E>,
}

impl<E: VaultEntity> Clone for VaultEntityStore<E> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
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
            _entity: PhantomData,
        }
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
    pub fn list(&self) -> Vec<E::Model> {
        Self::scan(&self.lock())
    }

    /// Free-standing scan, for callers holding their own vault.
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
    pub fn find(&self, pred: impl Fn(&E::Model) -> bool) -> Option<E::Model> {
        self.list().into_iter().find(|m| pred(m))
    }

    pub fn get(&self, id: &str) -> Result<E::Model, EntityError> {
        let uuid = Uuid::parse_str(id).map_err(EntityError::bad_id)?;
        self.get_by_uuid(uuid)
            .ok_or_else(|| EntityError::NotFound(id.to_string()))
    }

    pub fn get_by_uuid(&self, id: Uuid) -> Option<E::Model> {
        self.find(|m| E::id(m) == id)
    }

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
        Ok(model)
    }

    pub fn update(&self, mut model: E::Model) -> Result<E::Model, EntityError> {
        let mut vault = self.lock();
        let idx = Self::index_of(&vault, E::id(&model))
            .ok_or_else(|| EntityError::NotFound(E::id(&model).to_string()))?;

        // The page on disk owns its path; an update never moves a file.
        let path = vault.pages[idx].rel_path.clone();
        E::set_path(&mut model, path.clone());
        E::on_update(&mut model, Utc::now());

        let body = E::to_markdown(&model)?;
        vault.pages[idx].raw = body;
        vault::save_page(&mut vault, &path).map_err(EntityError::io)?;
        Ok(model)
    }

    pub fn delete(&self, id: &str) -> Result<(), EntityError> {
        let uuid = Uuid::parse_str(id).map_err(EntityError::bad_id)?;
        let mut vault = self.lock();
        let idx =
            Self::index_of(&vault, uuid).ok_or_else(|| EntityError::NotFound(id.to_string()))?;
        let path = vault.pages[idx].rel_path.clone();
        vault::delete_page(&mut vault, &path).map_err(EntityError::io)?;
        Ok(())
    }

    fn index_of(vault: &Vault, id: Uuid) -> Option<usize> {
        vault
            .pages
            .iter()
            .position(|p| E::matches(p) && E::from_page(p).map(|m| E::id(&m) == id).unwrap_or(false))
    }
}
