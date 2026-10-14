//! Documents between nodes (`offline.peers`): every native install is a
//! node, a host is just a node that stays up, and documents move between
//! any two of them the same way.
//!
//! Two pieces:
//!
//! - **[`DocPeer`]** — the lane a node serves to the peers it admits:
//!   [`DocPeer::list`] says which documents it holds (each vault file, by
//!   path, with the sha of its mirror), and [`DocPeer::sync`] is the
//!   ordinary `DocSync` session addressed by path rather than by a doc id
//!   somebody registered first. Gated on its own resource, apart from the
//!   vault lanes a person uses: a peer may exchange documents and do
//!   nothing else with the vault.
//! - **[`exchange`]** — what the pulling node runs per document: one
//!   stateless pipe between its own registry and the peer's, through an
//!   in-memory replica. Local history up to the peer, the peer's down to
//!   the local registry, each through a normal client session — so the
//!   registry on each side persists, relays to whoever has the document
//!   open, and writes the file, exactly as for a device's editor. Nothing
//!   about the pipe is remembered; the next sweep starts over.
//!
//! Both nodes sweep each other, so either pull alone would converge two
//! documents; a pull carries both directions anyway, which is what lets
//! an edit reach a peer that cannot dial back (behind a NAT, asleep).

use std::sync::Arc;

use crdt::loro::VersionVector;
use crdt::sync::{SyncDown, SyncError};
use uuid::Uuid;

use crate::replica::{Replica, SessionEnd, SharedStore, SyncLane};

/// One document a node holds: the vault it is in, its path there, and
/// the sha of the file it is mirrored to. The sha is how a puller skips
/// what already matches without opening a single document.
#[derive(Debug, Clone, PartialEq, Eq, facet::Facet)]
pub struct DocEntry {
    pub vault_id: String,
    pub path: String,
    pub sha256: String,
}

/// Which document: a vault and a path in it.
#[derive(Debug, Clone, PartialEq, Eq, facet::Facet)]
pub struct DocAt {
    pub vault_id: String,
    pub path: String,
}

/// The document lane a node serves to the nodes it admits.
#[vox::service]
pub trait DocPeer {
    /// Every document this node holds, in every vault it serves.
    async fn list(&self) -> Result<Vec<DocEntry>, SyncError>;

    /// The `DocSync` session for the document `at` names: registered here
    /// if it was not (a document this node has never been asked for, or
    /// one it does not hold yet — the session then makes it), refused when
    /// the caller's history is a different document of the same name.
    async fn sync(
        &self,
        at: DocAt,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError>;
}

/// One document on a peer's lane, as a [`SyncLane`] — the doc id the
/// replica passes is the one `(vault_id, path)` names, so it is dropped.
pub struct PeerDoc<'a> {
    pub client: &'a DocPeerClient,
    pub vault_id: &'a str,
    pub path: &'a str,
}

impl SyncLane for PeerDoc<'_> {
    async fn sync(
        &self,
        _doc_id: Uuid,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), vox::VoxError<SyncError>> {
        let at = DocAt {
            vault_id: self.vault_id.to_owned(),
            path: self.path.to_owned(),
        };
        self.client.sync(at, from, up, down).await
    }
}

/// What an [`exchange`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exchanged {
    /// The peer had history this node lacked; it is in the local
    /// registry now (and the file follows from the write-behind).
    Pulled,
    /// Nothing came down. What this node had and the peer lacked, if
    /// anything, went up.
    Unchanged,
}

// t[impl offline.peers] — a document moves between two nodes, both ways,
// through each one's own registry
/// Exchange one document between this node's registry (`local`) and a
/// peer's (`remote`). See the module docs; `doc_id` is the id both derive
/// from `(vault_id, path)`.
///
/// # Errors
///
/// How a session ended short of converging: [`SessionEnd::ForeignLineage`]
/// when the two nodes made two different documents of one file (neither
/// is merged into the other — that would put every line in twice),
/// [`SessionEnd::Refused`] when either side refused the attach,
/// [`SessionEnd::Dropped`] when a connection went.
pub async fn exchange(
    doc_id: Uuid,
    local: &impl SyncLane,
    remote: &impl SyncLane,
) -> Result<Exchanged, SessionEnd> {
    let store = SharedStore(Arc::new(crdt::in_memory::InMemoryPersistence::new()));
    let mut pipe = Replica::open(doc_id, store, None)
        .await
        .map_err(|e| SessionEnd::Refused(format!("open the pipe: {e}")))?;
    // What this node holds…
    pipe.sync_once(local, |_| {}).await?;
    let held: VersionVector = pipe.doc().loro().oplog_vv();
    // …meets what the peer holds: theirs comes down into the pipe, ours
    // goes up until the peer acknowledges all of it…
    pipe.sync_once(remote, |_| {}).await?;
    if held.includes_vv(&pipe.doc().loro().oplog_vv()) {
        return Ok(Exchanged::Unchanged);
    }
    // …and what came down goes into the local registry.
    pipe.sync_once(local, |_| {}).await?;
    Ok(Exchanged::Pulled)
}
