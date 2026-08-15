//! Server-side threads impl.
//!
//! Sits on the SeaORM `Model`/`Entity` items emitted by
//! `#[derive(architect::Entity)]` on `threads-proto`'s [`Thread`] +
//! [`Message`], and adds the [`Store`] that implements
//! [`threads_proto::ThreadsService`] — the anchored reads
//! (`list_threads` by `(entity_type, entity_id)`, `list_messages` by
//! `thread_id`) plus the provenance-stamped writes. Persistence is
//! SeaORM, so swapping the database is a connection + migration change.
//!
//! [`Thread`]: threads_proto::Thread
//! [`Message`]: threads_proto::Message

pub mod entity;
pub mod error;
pub mod migrations;
pub mod store;

pub use error::ThreadsDbError;
pub use migrations::Migrator;
pub use store::Store;
