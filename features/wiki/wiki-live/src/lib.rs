//! File-backed local impl of the wiki feature.
//!
//! Owns disk layout under `<vault>/Wiki/` (paths from
//! [`wiki_proto::paths`]) and persists ingest queue +
//! review + lint state as JSON in `Wiki/_state/`.
//!
//! This crate ships the **ingest slice** the
//! `agent-wiki` bridge needs end-to-end:
//!
//! - [`WikiLive::bootstrap`] — idempotently scaffold the
//!   `Wiki/` directory (schema, purpose, index, log, raw,
//!   _state, media).
//! - [`WikiLive::read_context`] — bundle `schema.md` +
//!   `purpose.md` + `index.md` for the LLM prompt.
//! - [`WikiLive::import_raw_source`] — drop bytes into
//!   `Wiki/raw/sources/`, return the
//!   [`wiki_proto::raw::RawSourceRef`].
//! - [`WikiLive::record_pages`] — atomic batch-write of
//!   `PageDraft`s (rejects paths under `raw/`).
//! - [`WikiLive::append_log`] — append an entry to
//!   `Wiki/log.md`.
//! - [`WikiLive::rebuild_index`] — walk + render
//!   `Wiki/index.md`.
//! - Queue + state: [`WikiLive::enqueue_ingest`],
//!   [`WikiLive::list_ingest`], [`WikiLive::claim_next_ingest`],
//!   [`WikiLive::record_analysis`], [`WikiLive::complete_ingest`],
//!   [`WikiLive::fail_ingest`].
//!
//! The crate does **not** yet implement the full
//! `wiki_proto::WikiService` trait. Once the proto is
//! trait-split the same way `agent-proto` is, the impls
//! become per-capability and slot in cleanly.

mod backend;
mod context;
mod error;
pub mod findings;
mod folder_import;
mod health;
mod index;
pub mod log_md;
pub mod queue;
mod raw;
pub mod reviews;
mod snapshot;
mod source_watcher;
mod state;
pub mod materialize;
pub mod subscriptions;
pub mod subscriptions_backend;
mod vault;

pub use backend::WikiBackend;
pub use context::WikiContext;
pub use error::WikiLiveError;
pub use findings::{FindingStatus, LintFinding, LintKind, LintSeverity};
pub use folder_import::ImportFolderOpts;
pub use health::WikiHealth;
pub use snapshot::RescanDiff;
pub use source_watcher::{SourceEvent, SourceWatcherGuard, WatchSourcesOpts};
pub use vault::WikiLive;
