//! The contacts capability trait. Same shape as
//! `recall_proto::service::*` — a single `#[architect::rpc]` trait that
//! emits its own async client / dispatcher / descriptor.

pub mod contacts;

pub use contacts::Contacts;
