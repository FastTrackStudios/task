//! The collab sync lane, under the vault's write guard.
//!
//! A vault file is written two ways: a save (`VaultSync::put_file`, which
//! asks the backend's [`vault::WriteGuard`] itself) and a live session —
//! a client attaches to the file's CRDT doc over `DocSync` and its edits
//! reach disk through the server's write-behind. That write-behind runs as
//! the server, so it cannot be where the rule is applied; the attach is.
//!
//! `open_collab` already refuses a caller the guard refuses, but a doc id
//! is a deterministic function of `(vault_id, path)` and the registry
//! admits any id somebody has opened. So a caller who may not write could
//! still attach to a doc that an Editor opened. [`GuardedVaultDocs`]
//! closes that: it looks the doc back up to its file and asks the same
//! guard, with the caller the permissions gate resolved for this call.

use crdt::registry::DocRegistry;
use crdt::sync::{DocSync, SyncDown, SyncError};
use uuid::Uuid;

/// The vault's doc registry, refusing an attach the write guard refuses.
#[derive(Clone)]
pub struct GuardedVaultDocs {
    pub registry: DocRegistry,
    pub sync: vault::Backend,
}

impl GuardedVaultDocs {
    /// May the caller on this request task attach to `doc_id`?
    fn admit(&self, doc_id: Uuid) -> Result<(), SyncError> {
        // An id `open_collab` never registered is the registry's to
        // refuse (it is not a vault file this server knows).
        let Some((vault_id, path)) = self.sync.collab_route(doc_id) else {
            return Ok(());
        };
        let who = vault::write_guard::gate_principal();
        self.sync
            .check_write(&vault_id, &path, who.as_deref())
            .map_err(|e| SyncError::Internal(e.to_string()))
    }
}

impl DocSync for GuardedVaultDocs {
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        self.admit(doc_id)?;
        self.registry.sync(doc_id, from, up, down).await
    }
}
