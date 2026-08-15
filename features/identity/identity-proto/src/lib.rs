//! `identity-proto` — the identity-locker wire contract.
//!
//! One [`LinkedServer`] row per home-org user per linked remote server.
//! It holds the encrypted session token (`token_ciphertext`, an
//! `auth::crypto::encrypt_secret()` envelope — never plaintext) that
//! lets the home server act on the user's behalf against another
//! server they've linked.
//!
//! ## Storage
//!
//! [`LinkedServer`] uses `#[derive(architect::Entity)]`, so it gets the
//! wasm-clean wire struct, `LinkedServerCreate` / `LinkedServerUpdate`
//! / `LinkedServerList`, the `LinkedServerRepo` service trait, and
//! (with `--features server`) the SeaORM
//! `Model`/`Entity`/`Column`/`Relation`/`ActiveModel` plus
//! `LinkedServerRepoStorage<C>`.
//!
//! ## RPC
//!
//! [`IdentityService`] (in [`service`]) exposes the home org's
//! locker over vox, mounted at the server-level `/server/vox`
//! endpoint and gated by a home-org session token.

pub mod linked_server;
pub mod service;

pub use linked_server::LinkedServer;
pub use service::{
    IdentityService, IdentityServiceError, LinkServerRequest, LinkView, ProfileSyncReport,
    ProfileView, SyncProfileRequest,
};

#[cfg(feature = "vox")]
pub use service::{
    IdentityServiceClient, identity_service_rpc_service_descriptor as identity_descriptor,
    serve as serve_identity,
};
