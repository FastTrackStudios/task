//! The recall capability trait. Same shape as
//! `inbox_proto::service::*` — a single `#[architect::rpc]` trait
//! that emits its own async client / dispatcher / descriptor.

pub mod recall;

pub use recall::Recall;
