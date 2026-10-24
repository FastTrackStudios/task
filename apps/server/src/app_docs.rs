//! App documents per org: architect's `crdt::docs`, served here.
//!
//! An app (Studio first) keeps its data as Loro docs: a project, a
//! setlist, a rig. It registers each one in its org's catalog (the
//! personal org for the person's own things) and syncs it over the
//! org's one `DocSync` lane. The server stores and relays the bytes and
//! the catalog row and never reads a doc: `kind` is the app's word, not
//! one Task interprets.
//!
//! - Storage: `<org>/app_docs.sqlite` (`OrgRoot::app_docs_db`). It holds
//!   the catalog (`crdt_doc_meta`) and the bytes (`crdt_doc`,
//!   `crdt_update`), migrated by `crdt_seaorm::Migrator`.
//! - Access: the org's permits gate (`permits::APP_DOCS`): members read
//!   and write, and a personal org has only its owner. The host's
//!   per-doc authorizer is left at its default; sharing a doc beyond its
//!   org is a later grant table consulted there.
//! - Content: [`OrgDocSync`] sends a catalogued id to the host, and
//!   anything else to the vault's registry.

use std::time::Duration;

use crdt::docs::DocsHost;
use crdt::sync::{DocSync, SyncDown, SyncError};
use uuid::Uuid;

/// Fold a doc's update log into one snapshot every this many updates.
const COMPACT_EVERY: u32 = 64;
/// Close a doc nobody has attached to for this long.
const IDLE: Duration = Duration::from_secs(15 * 60);

// t[impl offline.app-docs.replica] — the catalog and bytes in the org's
// own database, compacted and evicted when idle
// t[impl offline.app-docs.tombstone] — `DocsHost` keeps deletes as
// tombstones and stops serving the content
/// The org's app-docs host over its database (already migrated).
pub async fn open(db: sea_orm::DatabaseConnection) -> eyre::Result<DocsHost> {
    let bytes = crdt_seaorm::SeaOrmPersistence::new(db.clone());
    let registry = crdt::DocRegistry::new(move |doc_id| {
        let bytes = bytes.clone();
        Box::pin(async move { crdt::CrdtDoc::open(doc_id, bytes).await })
    })
    .with_compaction(COMPACT_EVERY)
    .with_idle_eviction(IDLE);
    DocsHost::open(crdt_seaorm::SeaOrmDocsStore::new(db), registry)
        .await
        .map_err(|e| eyre::eyre!("open app docs: {e}"))
}

/// The org's `DocSync` below the live sessions: app docs to their host,
/// every other id to the vault's registry (its own admission refuses an
/// id it never registered).
#[derive(Clone)]
pub struct OrgDocSync {
    pub docs: DocsHost,
    pub vault: vault_collab::VaultCollab,
}

impl DocSync for OrgDocSync {
    // t[impl offline.app-docs.create] — a catalogued id syncs only once
    // registered; the host admits nothing else
    async fn sync(
        &self,
        doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        if self.docs.admits(doc_id) {
            architect_telemetry::wide::set("doc.lane", "app-docs");
            self.docs.sync(doc_id, from, up, down).await
        } else {
            self.vault.sync(doc_id, from, up, down).await
        }
    }
}
