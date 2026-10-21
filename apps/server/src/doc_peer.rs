//! Documents between nodes (`offline.peers`), the node's two halves.
//!
//! Every native install runs this crate's `AppState` (a server, a laptop,
//! a phone), so every one of them is a node, and a host is only the node
//! that stays up. The vault's documents move between any two of them by
//! the same exchange — no node is the place documents come from.
//!
//! - **Serving**: [`DocPeerHost`] is the `DocPeer` lane, mounted on the
//!   org router. The router is served over the org's iroh endpoint with
//!   the caller's proved endpoint id as its identity, the gate lets a
//!   host reach `docs/replica` and nothing else of the vault
//!   (`files::peer::HostEngine`), and this checks the rest: the caller is
//!   an admitted host that keeps content — the org's own devices, never a
//!   federated server holding structure.
//! - **Pulling**: [`pull_peer`] runs inside the device sweep
//!   (`device_sync::sweep`), once per admitted peer: list what the peer
//!   holds, and exchange each document whose file differs from ours
//!   (`vault_doc::peer::exchange`). The exchange goes through this node's
//!   own registry as an ordinary client session, so what arrives is
//!   persisted, relayed to anyone with the document open here, and
//!   written to the file by the write-behind — which is also what tells
//!   the task stream (`task_sync`) a task arrived.
//!
//! One span per peer per sweep, `doc_peer/pull`, with the counts:
//! `doc_peer.listed`, `doc_peer.exchanged`, `doc_peer.pulled`,
//! `doc_peer.foreign`, `doc_peer.failed`, and `doc_peer.outcome`
//! (`ok` | `unreachable` | `refused`). Never a path.

use std::collections::HashMap;
use std::time::Duration;

use architect::iroh_link::{self, iroh};
use architect::{LayerRouter, LocalServer, Scope};
use architect_permissions::Principal;
use architect_telemetry::wide;
use crdt::registry::DocRegistry;
use crdt::sync::{DocSync, DocSyncClient, SyncDown, SyncError};
use files_domain::{Content, HostId};
use tracing::Instrument;
use vault_doc::SessionEnd;
use vault_doc::peer::{DocAt, DocEntry, DocPeer, DocPeerClient, Exchanged, PeerDoc};
use vault_proto::collab_doc_id;

use crate::OrgAppState;

/// A dial that has not answered in this long is a peer that is not there
/// this pass.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// One document's exchange. Generous — a long history over a slow link —
/// but bounded, so one stuck session cannot stall the sweep.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(60);

/// The `DocPeer` lane over an org's vault documents.
#[derive(Clone)]
pub struct DocPeerHost {
    pub registry: DocRegistry,
    pub sync: vault::Backend,
    pub files: files::FilesBackend,
}

impl DocPeerHost {
    #[must_use]
    pub fn of(org: &OrgAppState) -> Self {
        Self {
            registry: org.vault_collab.registry().clone(),
            sync: org.vault_sync.clone(),
            files: org.files.clone(),
        }
    }

    /// The caller is an admitted peer that keeps content. The gate has
    /// already let a host through to `docs/replica`; this is the half it
    /// cannot see (the admission's hosting) and the half that holds when
    /// the gate only observes.
    fn admit(&self) -> Result<(), SyncError> {
        let Some(Principal::Host { endpoint }) = architect::permissions_gate::caller() else {
            return Err(SyncError::Internal(
                "the document lane is for this org's nodes".into(),
            ));
        };
        match self.files.admits(&HostId(endpoint)) {
            Some(h) if h.content != Content::None => Ok(()),
            _ => Err(SyncError::Internal(
                "this peer keeps no content of this org".into(),
            )),
        }
    }
}

impl DocPeer for DocPeerHost {
    async fn list(&self) -> Result<Vec<DocEntry>, SyncError> {
        self.admit()?;
        let sync = self.sync.clone();
        tokio::task::spawn_blocking(move || held(&sync))
            .await
            .map_err(|e| SyncError::Internal(format!("list: {e}")))
    }

    async fn sync(
        &self,
        at: DocAt,
        from: Vec<u8>,
        up: vox::Rx<Vec<u8>>,
        down: vox::Tx<SyncDown>,
    ) -> Result<(), SyncError> {
        self.admit()?;
        let doc_id = self
            .sync
            .register_collab(&at.vault_id, &at.path)
            .map_err(|e| SyncError::Internal(e.to_string()))?;
        crate::doc_guard::same_lineage(&self.registry, doc_id, &from).await?;
        self.registry.sync(doc_id, from, up, down).await
    }
}

/// Every markdown file in every vault `sync` serves, with its sha.
fn held(sync: &vault::Backend) -> Vec<DocEntry> {
    let mut out = Vec::new();
    for vault_id in sync.vault_ids() {
        // Markdown only, filtered before hashing: every other file in
        // the vault (attachments, PDFs, audio) would be streamed through
        // SHA-256 on every peer's `list` just to drop its row.
        let Ok(manifest) = sync.manifest_where(&vault_id, |p| p.ends_with(".md")) else {
            continue;
        };
        out.extend(manifest.files.into_iter().map(|f| DocEntry {
            vault_id: vault_id.clone(),
            path: f.path,
            sha256: f.sha256,
        }));
    }
    out
}

/// What one pull from one peer did.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PullReport {
    /// Documents the peer holds.
    pub listed: usize,
    /// Documents whose file differed here, so were exchanged.
    pub exchanged: usize,
    /// Of those, documents the peer had history of that this node lacked.
    pub pulled: usize,
    /// Two nodes that made two different documents of one file — left
    /// as they are on both (see `vault_doc::peer::exchange`).
    pub foreign: usize,
    /// Refused, dropped or timed out — tried again next sweep.
    pub failed: usize,
}

/// Pull `host`'s documents into `org` — and, in the same sessions, push
/// ours to it. One span; see the module docs.
pub async fn pull_peer(
    org: &OrgAppState,
    endpoint: &iroh::Endpoint,
    host: &HostId,
) -> Result<PullReport, String> {
    let span = tracing::info_span!("doc_peer.pull", otel.name = "doc_peer/pull");
    async move {
        let res = pull(org, endpoint, host).await;
        match &res {
            Ok(r) => {
                wide::set("doc_peer.outcome", "ok");
                wide::set("doc_peer.listed", r.listed as i64);
                wide::set("doc_peer.exchanged", r.exchanged as i64);
                wide::set("doc_peer.pulled", r.pulled as i64);
                wide::set("doc_peer.foreign", r.foreign as i64);
                wide::set("doc_peer.failed", r.failed as i64);
                if r.foreign > 0 {
                    // Alertable: two documents of one file will not
                    // converge until somebody decides which to keep.
                    tracing::warn!(
                        foreign = r.foreign,
                        "doc_peer: documents made separately on two nodes are not merged"
                    );
                }
            }
            Err(_) => wide::set("doc_peer.outcome", "unreachable"),
        }
        res
    }
    .instrument(span)
    .await
}

async fn pull(
    org: &OrgAppState,
    endpoint: &iroh::Endpoint,
    host: &HostId,
) -> Result<PullReport, String> {
    let peer = dial(endpoint, &host.0).await?;
    let theirs = peer
        .list()
        .await
        .map_err(|e| format!("list on {}: {e:?}", host.0))?;

    let sync = org.vault_sync.clone();
    let ours: HashMap<(String, String), String> = tokio::task::spawn_blocking(move || {
        held(&sync)
            .into_iter()
            .map(|e| ((e.vault_id, e.path), e.sha256))
            .collect()
    })
    .await
    .map_err(|e| format!("list here: {e}"))?;

    // This node's registry, as a client of itself: the exchange is then
    // an ordinary session on both sides.
    let scope = Scope::new();
    let router = LayerRouter::new().with(
        crdt::sync::doc_sync_service_descriptor(),
        crdt::sync::DocSyncDispatcher::new(org.vault_collab.registry().clone()),
    );
    let local: DocSyncClient = LocalServer::serve(router, scope.clone())
        .establish()
        .await
        .map_err(|e| format!("local registry: {e}"))?;

    let mut report = PullReport {
        listed: theirs.len(),
        ..PullReport::default()
    };
    for entry in &theirs {
        let key = (entry.vault_id.clone(), entry.path.clone());
        if ours.get(&key) == Some(&entry.sha256) {
            continue;
        }
        let Ok(doc_id) = org.vault_sync.register_collab(&entry.vault_id, &entry.path) else {
            report.failed += 1;
            continue;
        };
        debug_assert_eq!(doc_id, collab_doc_id(&entry.vault_id, &entry.path));
        report.exchanged += 1;
        let remote = PeerDoc {
            client: &*peer,
            vault_id: &entry.vault_id,
            path: &entry.path,
        };
        let done = tokio::time::timeout(
            EXCHANGE_TIMEOUT,
            vault_doc::peer::exchange(doc_id, &local, &remote),
        )
        .await;
        match done {
            Ok(Ok(Exchanged::Pulled)) => report.pulled += 1,
            Ok(Ok(Exchanged::Unchanged)) => {}
            Ok(Err(SessionEnd::ForeignLineage)) => report.foreign += 1,
            Ok(Err(_)) | Err(_) => report.failed += 1,
        }
    }
    scope.close().await;
    Ok(report)
}

/// Dial `peer` from this org's endpoint and open its document lane. The
/// connection closes when the returned client drops: see
/// [`files_sync::Dialled`], which this sweep leaked without.
async fn dial(
    endpoint: &iroh::Endpoint,
    peer: &str,
) -> Result<files_sync::Dialled<DocPeerClient>, String> {
    let id: iroh::EndpointId = peer
        .parse()
        .map_err(|e| format!("not an endpoint id: {peer} ({e})"))?;
    let link = tokio::time::timeout(DIAL_TIMEOUT, iroh_link::connect(endpoint, id))
        .await
        .map_err(|_| format!("dialling {peer}: timed out"))?
        .map_err(|e| format!("dialling {peer}: {e}"))?;
    files_sync::Dialled::establish(link)
        .await
        .map_err(|e| format!("the document lane on {peer}: {e}"))
}
