//! Documents between nodes (`offline.peers`): the vault's half of
//! `crdt::peer`.
//!
//! Every native install runs this crate's `AppState` (a server, a laptop,
//! a phone), so every one of them is a node, and a host is only the node
//! that stays up. The lane, the exchange and the lineage refusal are
//! architect's (`crdt::peer`); what is left here is the vault's:
//!
//! - **Admission** ([`host`]): the router is served over the org's iroh
//!   endpoint with the caller's proved endpoint id as its identity, the
//!   gate lets a host reach `docs/replica` and nothing else of the vault
//!   (`files::peer::HostEngine`), and [`keeps_content`] checks the rest:
//!   the caller is an admitted host that keeps content, the org's own
//!   devices, never a federated server holding structure.
//! - **Path mapping** ([`VaultDocs`]): a document is a vault file, named
//!   `<vault_id>/<path>`, its id the one both nodes derive from the pair
//!   (`vault_proto::collab_doc_id`), its digest the file's sha.
//! - **Pulling** ([`pull_peer`]): once per admitted peer inside the
//!   device sweep (`device_sync::sweep`): list what the peer holds and
//!   exchange each document whose file differs from ours
//!   (`crdt::peer::exchange`) through this node's own registry, so what
//!   arrives is persisted, relayed to anyone with the document open here,
//!   and written to the file by the write-behind, which is also what
//!   tells the task stream (`task_sync`) a task arrived.
//!
//! One span per peer per sweep, `doc_peer/pull`, with the counts:
//! `doc_peer.listed`, `doc_peer.exchanged`, `doc_peer.pulled`,
//! `doc_peer.foreign`, `doc_peer.failed`, and `doc_peer.outcome`
//! (`ok` | `unreachable`). Never a path.

use std::collections::HashMap;
use std::time::Duration;

use architect::iroh_link::{self, iroh};
use architect_permissions::Principal;
use architect_telemetry::wide;
use crdt::peer::{
    DocAt, DocCatalog, DocPeerClient, Exchanged, HeldDoc, PeerLane, PullReport, SelfLane,
    SessionEnd,
};
use crdt::sync::SyncError;
use files_domain::{Content, HostId};
use tracing::Instrument;
use uuid::Uuid;
use vault_proto::collab_doc_id;

use crate::OrgAppState;

/// A dial that has not answered in this long is a peer that is not there
/// this pass.
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

/// One document's exchange. Generous (a long history over a slow link)
/// but bounded, so one stuck session cannot stall the sweep.
const EXCHANGE_TIMEOUT: Duration = Duration::from_secs(60);

/// The `DocPeer` lane over an org's vault documents.
pub type DocPeerHost = crdt::peer::DocPeerHost<VaultDocs>;

/// The org's `DocPeer` lane: its vault documents, to the hosts that keep
/// its content.
#[must_use]
pub fn host(org: &OrgAppState) -> DocPeerHost {
    let files = org.files.clone();
    crdt::peer::DocPeerHost::new(
        org.vault_collab.registry().clone(),
        VaultDocs(org.vault_sync.clone()),
        move |endpoint| keeps_content(&files, endpoint),
    )
}

/// The peer at `endpoint` is admitted to this org and keeps content. The
/// gate has already let a host through to `docs/replica`; this is the
/// half it cannot see (the admission's hosting) and the half that holds
/// when the gate only observes.
fn keeps_content(files: &files::FilesBackend, endpoint: &str) -> bool {
    files
        .admits(&HostId(endpoint.to_owned()))
        .is_some_and(|h| h.content != Content::None)
}

/// An org's vault files as `crdt::peer` documents.
#[derive(Clone)]
pub struct VaultDocs(pub vault::Backend);

impl DocCatalog for VaultDocs {
    async fn held(&self) -> Result<Vec<HeldDoc>, SyncError> {
        let sync = self.0.clone();
        tokio::task::spawn_blocking(move || held(&sync))
            .await
            .map_err(|e| SyncError::Internal(format!("list: {e}")))
    }

    /// Registered here if it was not (a document this node has never
    /// been asked for, or one it does not hold yet: the session then
    /// makes it).
    async fn resolve(&self, at: &DocAt) -> Result<Uuid, SyncError> {
        let (vault_id, path) = route(at).ok_or_else(|| {
            SyncError::Internal(format!("`{}` is not a vault document of that id", at.name))
        })?;
        self.0
            .register_collab(vault_id, path)
            .map_err(|e| SyncError::Internal(e.to_string()))
    }
}

/// The `(vault_id, path)` `at` names: its name split at the `/` whose
/// halves derive `at.doc_id`, so a vault id holding a `/` still routes,
/// and a name that does not match its id routes nowhere.
fn route(at: &DocAt) -> Option<(&str, &str)> {
    at.name.match_indices('/').find_map(|(i, _)| {
        let (vault_id, rest) = at.name.split_at(i);
        let path = rest.strip_prefix('/')?;
        (collab_doc_id(vault_id, path) == at.doc_id).then_some((vault_id, path))
    })
}

/// Every markdown file in every vault `sync` serves, with its sha.
fn held(sync: &vault::Backend) -> Vec<HeldDoc> {
    let mut out = Vec::new();
    for vault_id in sync.vault_ids() {
        // Markdown only, filtered before hashing: every other file in
        // the vault (attachments, PDFs, audio) would be streamed through
        // SHA-256 on every peer's `list` just to drop its row.
        let Ok(manifest) = sync.manifest_where(&vault_id, |p| p.ends_with(".md")) else {
            continue;
        };
        out.extend(manifest.files.into_iter().map(|f| HeldDoc {
            doc_id: collab_doc_id(&vault_id, &f.path),
            name: format!("{vault_id}/{}", f.path),
            digest: f.sha256,
        }));
    }
    out
}

/// Pull `host`'s documents into `org` and, in the same sessions, push
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

/// `crdt::peer::pull`, with each document registered here before its
/// exchange: this node's registry admits only a document its vault
/// registered, and only the peer's listing names one this node has never
/// opened.
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

    let docs = VaultDocs(org.vault_sync.clone());
    let ours: HashMap<Uuid, String> = docs
        .held()
        .await
        .map_err(|e| format!("list here: {e}"))?
        .into_iter()
        .map(|d| (d.doc_id, d.digest))
        .collect();

    // This node's registry, as a client of itself: the exchange is then
    // an ordinary session on both sides, and the registry's authorizer
    // sees who is pulling.
    let local = SelfLane::open(
        org.vault_collab.registry().clone(),
        Principal::Service {
            name: "doc-peer".into(),
        },
    )
    .await
    .map_err(|e| format!("local registry: {e}"))?;

    let mut report = PullReport {
        listed: theirs.len(),
        ..PullReport::default()
    };
    for entry in &theirs {
        if ours.get(&entry.doc_id) == Some(&entry.digest) {
            continue;
        }
        let at = DocAt {
            doc_id: entry.doc_id,
            name: entry.name.clone(),
        };
        if docs.resolve(&at).await.is_err() {
            report.failed += 1;
            continue;
        }
        report.exchanged += 1;
        let remote = PeerLane {
            client: &peer,
            name: &entry.name,
        };
        let done = tokio::time::timeout(
            EXCHANGE_TIMEOUT,
            crdt::peer::exchange(entry.doc_id, &local, &remote),
        )
        .await;
        match done {
            Ok(Ok(Exchanged::Pulled)) => report.pulled += 1,
            Ok(Ok(Exchanged::Unchanged)) => {}
            Ok(Err(SessionEnd::ForeignLineage)) => report.foreign += 1,
            Ok(Err(_)) | Err(_) => report.failed += 1,
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn at(vault_id: &str, path: &str) -> DocAt {
        DocAt {
            doc_id: collab_doc_id(vault_id, path),
            name: format!("{vault_id}/{path}"),
        }
    }

    #[test]
    fn a_name_routes_back_to_its_file() {
        let a = at("default", "Notes/today.md");
        assert_eq!(route(&a), Some(("default", "Notes/today.md")));
        // A vault id with a `/` in it: the id picks the split.
        let b = at("assets/songs", "Charts/x.md");
        assert_eq!(route(&b), Some(("assets/songs", "Charts/x.md")));
    }

    #[test]
    fn a_name_that_is_not_its_id_routes_nowhere() {
        let mut a = at("default", "Notes/today.md");
        a.doc_id = collab_doc_id("default", "Notes/other.md");
        assert_eq!(route(&a), None);
        let bare = DocAt {
            doc_id: collab_doc_id("default", ""),
            name: "default".into(),
        };
        assert_eq!(route(&bare), None);
    }
}
