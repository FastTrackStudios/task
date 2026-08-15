//! Typed vox service clients for the shell's own services.
//!
//! The connection machinery — endpoint resolution, the per-org cached
//! connection root, the cancel-safe browser dial, and the generic
//! [`establish_for`] / [`establish_server`] entry points — lives in
//! [`task_ui_core::vox_clients`]; it is re-exported here so every
//! `crate::vox_clients::…` call site keeps working. This module only
//! adds the handful of typed wrappers the shell reaches for often
//! enough to name.
//!
//! Feature UI crates declare their own wrappers over the same
//! [`establish_for`] rather than growing this list — that's what keeps
//! their protos out of the shell's graph.

pub use task_ui_core::vox_clients::{
    RootLane, caller_for, drop_cached_connections, establish_for, establish_server,
};

/// An org's `TaskServiceClient` — a view over the org's shared caller.
pub async fn task_client(slug: &str) -> Result<task_proto::TaskServiceClient, String> {
    establish_for::<task_proto::TaskServiceClient>(slug).await
}

/// An org's `VaultSyncClient` — backs the `/vault` route (manifest,
/// read, conditional write over the same long-lived socket).
pub async fn vault_client(slug: &str) -> Result<vault_proto::VaultSyncClient, String> {
    establish_for::<vault_proto::VaultSyncClient>(slug).await
}

/// An org's `ShareServiceClient` — share-link CRUD for the note Share
/// panel + the Links registry.
pub async fn share_client(slug: &str) -> Result<share_proto::ShareServiceClient, String> {
    establish_for::<share_proto::ShareServiceClient>(slug).await
}

/// An org's `VaultGraphClient` — link-graph reads (backlinks /
/// links / orphans / unresolved / deadends / tags) for the vault
/// page's backlinks panel and the editor's tag candidates.
pub async fn vault_graph_client(slug: &str) -> Result<vault_proto::VaultGraphClient, String> {
    establish_for::<vault_proto::VaultGraphClient>(slug).await
}
