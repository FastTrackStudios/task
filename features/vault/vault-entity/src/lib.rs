//! `vault-entity` — the shared half of a vault-backed entity slice.
//!
//! Most Task feature slices store their domain objects as markdown
//! pages with YAML frontmatter inside a [`vault::Vault`]. Each slice
//! had grown its own copy of the same four things: a frontmatter
//! splitter, a set of lenient YAML readers, a `slugify`, and a
//! `Store` with list/get/create/update/delete over `Arc<Mutex<Vault>>`.
//! The copies were mechanical — and had begun to drift, most visibly in
//! `slugify`, where two spellings of the same name produced two
//! different on-disk paths.
//!
//! This crate is that shared half, written once:
//!
//! - [`frontmatter`] — split, type-discriminate, and re-emit pages
//! - [`yaml`] — lenient accessors over a frontmatter mapping
//! - [`slug`] — one slug rule and the default-path helper
//! - [`store`] — [`VaultEntityStore<T>`](store::VaultEntityStore), generic CRUD
//! - [`error`] — one error vocabulary, convertible into a slice's own
//!
//! A slice implements [`VaultEntity`] for its model and keeps only what
//! is genuinely its own: the parse/serialize field mapping and any
//! domain operations beyond CRUD.
//!
//! ```text
//! impl VaultEntity for BodyMetric {
//!     const TYPE: &'static str = "body-metric";
//!     const DEFAULT_FOLDER: &'static str = "Projects/Fitness/body";
//!
//!     fn id(&self) -> Uuid { self.id }
//!     fn set_id(&mut self, id: Uuid) { self.id = id; }
//!     fn path(&self) -> &str { &self.path }
//!     fn set_path(&mut self, p: String) { self.path = p; }
//!     fn name(&self) -> &str { &self.name }
//!
//!     fn from_page(page: &VaultPage) -> Result<Self, ParseError> { … }
//!     fn to_markdown(&self) -> Result<String, WriteError> { … }
//! }
//! ```

//! ## Features
//!
//! - `store` (default) — [`VaultEntity`] and [`VaultEntityStore`], the
//!   generic CRUD half. Requires `vault`, which reaches `notify`/`mio`
//!   and so is native-only.
//! - No features — just the readers and writers (`frontmatter`, `yaml`,
//!   `slug`, `error`). This is what a slice whose `parse` module is
//!   wasm-visible should depend on: `vault-entity = { workspace = true,
//!   default-features = false }`.

pub mod index;
pub mod error;
pub mod frontmatter;
pub mod slug;
pub mod yaml;

#[cfg(feature = "store")]
pub mod store;

pub use error::{EntityError, ParseError, WriteError};
pub use slug::{entity_path, slugify};

#[cfg(feature = "store")]
pub use store::{VaultEntity, VaultEntityStore};
