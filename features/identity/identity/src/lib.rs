//! Server-side identity locker.
//!
//! Sits on top of the SeaORM `Model`/`Entity` items emitted by
//! `#[derive(architect::Entity)]` on `identity-proto`'s
//! [`identity_proto::LinkedServer`]; adds the [`Store`] that holds,
//! per home-org user, the encrypted session tokens for other servers
//! they've linked. Tokens are encrypted at rest with
//! `auth::crypto::encrypt_secret` and only decrypted on read.
//!
//! No RPC service and no server wiring live here yet — those arrive in
//! later subtasks.

pub mod entity;
pub mod error;
pub mod migrations;
pub mod store;

pub use error::IdentityDbError;
pub use identity_proto::LinkedServer;
pub use migrations::Migrator;
pub use store::{LinkRecord, Store};
