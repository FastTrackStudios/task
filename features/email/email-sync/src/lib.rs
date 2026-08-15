//! Per-account sync engine.
//!
//! One [`SyncEngine`] per account drives an
//! `email_proto::EmailSync` backend, reconciles its state with
//! the local `email-store` cache, and broadcasts merged
//! [`SyncEvent`]s to subscribers (typically the UI).
//!
//! Why this is a separate crate: pimalaya splits `neverest`
//! (sync engine) and `mirador` (watcher) out of `himalaya` (the
//! client) — sync isn't baked into the backend's request
//! surface; it's a layer above. Same model here: any
//! `EmailSync` impl (imap / maildir / jmap / nextcloud) can be
//! wrapped in this engine without changing the backend.
//!
//! Phase 1 (this iteration):
//! - Periodic [`SyncEngine::run_cycle`] — list folders, fetch
//!   envelopes for each, diff against the previous snapshot,
//!   emit `NewMessage` / `FolderListChanged` events.
//! - Public [`SyncHandle`] that owns the spawned task; drop
//!   it (or call `stop`) to cancel cleanly.
//! - No `email-store` writes yet — that lands when the store
//!   grows a Maildir walker. The engine instead keeps an
//!   in-memory snapshot for diff calculations, which is enough
//!   to drive a live UI.
//!
//! Phase 2 (next):
//! - Forward backend `subscribe()` events through the same
//!   broadcast channel (currently we only generate events from
//!   poll diffs).
//! - Write fetched messages into `email-store`'s Maildir +
//!   `SQLite` index.
//! - Outbound queue for drafts + pending flag changes when the
//!   backend is offline.

#![cfg(not(target_arch = "wasm32"))]

mod engine;
mod event;
mod snapshot;

pub use engine::{SyncEngine, SyncHandle, SyncOptions};
pub use event::SyncEvent;
pub use snapshot::Snapshot;
