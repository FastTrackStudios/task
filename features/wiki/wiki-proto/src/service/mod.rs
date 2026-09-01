//! Per-capability traits for the wiki feature. Same shape
//! as `agent-proto::service` — no umbrella trait; callers
//! express what they need via bounds:
//!
//! ```ignore
//! fn run_ingest<W: Schema + Catalog + Ingest + RawLayer>(wiki: &W, ...) { /* ... */ }
//! ```
//!
//! Each trait is `#[architect::rpc]`-decorated and emits
//! its own async client + dispatcher + descriptor.
//! Backends implement only what they support — federation
//! peers may only impl `Catalog + Search`; the local
//! `wiki-live` implements most of them.

pub mod catalog;
pub mod events;
pub mod federation;
pub mod graph;
pub mod ingest;
pub mod lint;
pub mod multimodal;
pub mod pages;
pub mod raw_layer;
pub mod registry;
pub mod research;
pub mod review;
pub mod schema;
pub mod search;
pub mod subscriptions;
pub mod watcher;

pub use catalog::Catalog;
pub use events::Events;
pub use federation::Federation;
pub use graph::Graph;
pub use ingest::Ingest;
pub use lint::Lint;
pub use multimodal::Multimodal;
pub use pages::Pages;
pub use raw_layer::RawLayer;
pub use registry::{Registry, WikiSummary};
pub use research::Research;
pub use review::Review;
pub use schema::Schema;
pub use search::Search;
pub use subscriptions::{HeldSubscription, RefreshReport, Subscriptions};
pub use watcher::Watcher;
