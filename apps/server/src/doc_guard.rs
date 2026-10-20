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

impl GuardedVaultDocs {
    /// Refuse a replica whose history shares nothing with the served
    /// document — the server's document was recreated (a fresh data
    /// root, a reseeded demo) under the same id. Merging the two would
    /// put every line in twice; refusing with
    /// [`vault_doc::FOREIGN_LINEAGE`] lets the device keep its copy and
    /// decide (`vault_doc::Replica::reset`).
    async fn same_lineage(&self, doc_id: Uuid, from: &[u8]) -> Result<(), SyncError> {
        same_lineage(&self.registry, doc_id, from).await
    }
}

/// Refuse an attach whose history shares nothing with `registry`'s
/// document of `doc_id` — see [`GuardedVaultDocs::same_lineage`]. The
/// peer document lane (`crate::doc_peer`) applies the same rule to a
/// node's history.
pub(crate) async fn same_lineage(
    registry: &DocRegistry,
    doc_id: Uuid,
    from: &[u8],
) -> Result<(), SyncError> {
    if from.is_empty() {
        return Ok(());
    }
    let Ok(client) = crdt::loro::VersionVector::decode(from) else {
        return Ok(()); // the registry reports a bad vector itself
    };
    let Ok(doc) = registry.doc(doc_id).await else {
        return Ok(()); // unknown doc: the registry's refusal
    };
    if vault_doc::is_foreign_lineage(&client, &doc.loro().oplog_vv()) {
        architect_telemetry::wide::set("collab.sync", "foreign_lineage");
        return Err(SyncError::BadVersion(format!(
            "{}: this copy of {doc_id} is not a version of this node's",
            vault_doc::FOREIGN_LINEAGE
        )));
    }
    Ok(())
}

impl DocSync for GuardedVaultDocs {
    // t[impl offline.doc.access] — every attach (each reconnect of each
    // device, offline-created documents' first sync included) is checked
    // against the file's write guard for the caller on this request, and
    // a replica from another lineage is refused rather than merged
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        if let Err(e) = self.admit(doc_id) {
            architect_telemetry::wide::set("collab.sync", "refused");
            return Err(e);
        }
        self.same_lineage(doc_id, &from).await?;
        architect_telemetry::wide::set("collab.sync", "attached");
        self.registry.sync(doc_id, from, up, down).await
    }
}
