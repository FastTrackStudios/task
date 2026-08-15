//! Server-side notifications impl.
//!
//! [`Store`] is the SeaORM-backed [`notify_proto::Notify`] backend
//! (per-org `notify.sqlite`, like `timer`'s); [`channel`] carries the
//! [`DeliveryChannel`] trait plus the shipped channels. The rules that
//! *produce* notifications live server-side in
//! `apps/task/server/src/notifier.rs` — this crate only stores and
//! delivers what the notifier hands it.

pub mod channel;
pub mod entity;
pub mod migrations;
pub mod store;

pub use channel::{DeliveryChannel, InApp, Webhook};
pub use migrations::Migrator;
pub use store::{NewNotification, Store};
