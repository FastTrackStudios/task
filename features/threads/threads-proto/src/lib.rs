//! `threads-proto` — conversations & topics wire contract.
//!
//! A **universal conversation primitive**: a [`Thread`] is a topic
//! anchored to any host entity via `(entity_type, entity_id)` — a
//! task, a project today; a forge issue, an email thread, a chat room
//! tomorrow — and a [`Message`] is one entry in it. The anchor is the
//! same polymorphic pair the old threads feature used, so "enable
//! threads on X" is just a new `entity_type` string, no schema change.
//!
//! ## Storage / DB portability
//!
//! Both entities use `#[derive(architect::Entity)]`, so each gets the
//! wasm-clean wire struct, `<Name>Create`/`<Name>Update`/`<Name>List`,
//! the `<Name>Repo` `#[vox::service]` trait, and (with `--features
//! server`) the SeaORM `Model`/`Entity`/`Column`/`Relation`/`ActiveModel`
//! plus `<Name>RepoStorage<C>`. Persistence is SeaORM, so swapping the
//! database (sqlite → postgres) is a connection-URL + migration change,
//! not an app-code change.
//!
//! [`ThreadsService`] carries the anchored reads (threads by
//! `(entity_type, entity_id)`, messages by `thread_id`) that the
//! architect-emitted generic `Repo::list` filter can't yet express,
//! plus the convenience writes the UI / CLI / agents drive.
//!
//! ## Provenance
//!
//! Thread- and message-level `source_*` fields (+ `Message::original_text`)
//! record where an ingested conversation came from and preserve the
//! verbatim original — the trace-back-to-source the comms vision needs.
//! `"native"` for things created in-app.

pub mod error;
pub mod message;
pub mod service;
pub mod thread;

pub use error::ThreadsError;
pub use message::Message;
pub use service::{CreateThreadRequest, PostMessageRequest, ThreadsService};
pub use thread::Thread;

/// The architect-generated vox client for [`ThreadsService`] — what the
/// app calls via `establish_for::<ThreadsServiceClient>(slug)`.
#[cfg(feature = "vox")]
pub use service::ThreadsServiceClient;
