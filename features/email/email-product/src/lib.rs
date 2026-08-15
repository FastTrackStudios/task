//! `email-product` — the product layer over the synced mailbox.
//!
//! Implements `email_proto::EmailProduct` (the outbox / staged-send
//! surface) over per-account `email_store::Store` sqlite handles,
//! plus the server-side delivery poller that turns `Approved`
//! entries into real `EmailSync::send` calls with exponential
//! backoff.
//!
//! ## Shape
//!
//! - The backend holds one `Store` per account (same `index.db`
//!   the sync engine uses) behind a `std::sync::Mutex` — methods
//!   run on the blocking pool (`TokioBlockingDispatcher`), so a
//!   short mutex hold is fine.
//! - Delivery goes through the mounted `EmailSync` backend
//!   (`Arc<dyn EmailSync>`) — no second SMTP path; the maildir
//!   backend's `send` (submit + Sent-copy + `NewMessage` event)
//!   is the single way out.
//! - Events are published on the *same* `EmailChange` hub the
//!   `EmailSync` `changes` stream serves (the hub is cloned from
//!   the sync backend at construction), publish-after-write.

#![cfg(not(target_arch = "wasm32"))]

mod backend;
mod triage;

pub use backend::{ProductAccount, ProductBackend};
pub use triage::{
    ContactLookup, DerivationEngine, DerivationInput, HeuristicEngine, NoContacts,
};
