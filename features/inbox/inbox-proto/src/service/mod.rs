//! The inbox capability trait. Same shape as
//! `scheduling-proto::service::*` — a single `#[architect::rpc]`
//! trait that emits its own async client / dispatcher / descriptor.

pub mod inbox;

pub use inbox::Inbox;
