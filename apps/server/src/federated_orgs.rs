//! Subscribing to a source on **another server** — the transport half.
//!
//! [`wiki_live::subscriptions_backend::LocalOrgs`] resolves a source by
//! looking for an org of that domain's name on this data root, which is
//! the arrangement `admin seed` produces and the only one that federated
//! before this module. A publisher on another server is not on this
//! disk, so that resolver answers `None` — an orphan, correctly, because
//! there is genuinely nothing here to read.
//!
//! [`FederatedOrgs`] is the same resolver with one more place to look:
//! the org's own peer table (`wiki_live::source_peers`), which says where
//! a domain is and holds the secret that reads one source there. When it
//! answers, the refresh reads over the wire through
//! [`wiki_live::source::Source::Remote`] and nothing above it changes —
//! `materialize::refresh` has been generic over its source all along.
//!
//! # Why this lives in the server and not in `wiki-live`
//!
//! `wiki-live` does not depend on `vox` and should not start: it is the
//! disk-backed implementation of the wiki feature, and the split that
//! keeps the files lane honest is the same one — `RemoteFiles` is a port
//! in `files`, and `IrohRemotes`, which dials, is the transport beside
//! it. `Upstream` and `SourceVault` are the ports here. This is the
//! `IrohRemotes` of the wiki lane, and it is in the crate that already
//! owns dialling, serving and endpoints.
//!
//! # One bridge, on the rare path
//!
//! `SourceVault` is sync because the refresh engine is; the generated
//! `SubscriptionsClient` is async. Something has to bridge, and the
//! choice is *which side*: a refresh's disk work is the common path and
//! belongs on a blocking thread, its wire work is the rare one. So the
//! engine stays sync — dispatched onto `spawn_blocking` by
//! `architect/dispatch-tokio`, where blocking is what the thread is for —
//! and [`RemoteSource::block_on`] carries the wire across.
//!
//! Inverting it was considered and is worse: an async engine would put
//! every local refresh's tree scan on the reactor, or else hop through
//! `spawn_blocking` once per file to get off it again, to spare one call
//! on the path that leaves the building.
//!
//! # What crosses, and why the line is size rather than kind
//!
//! A wiki and an asset **shelf** cross, both through the vault engine —
//! whose manifest turns out to be content-agnostic: the walk hashes every
//! file it meets, markdown or not. So a shelf's documents ride the same
//! two read calls a wiki's pages do, bounded by
//! `materialize::REMOTE_FILE_LIMIT`, and what the bound leaves behind is
//! reported rather than attempted.
//!
//! A **Resource** crosses on the same terms, and it has to: the tier
//! holds scripture *and* the app libraries — `resources/patches/`,
//! `resources/samples/` — which ADR 0003 put there precisely because a
//! subscription could carry them. Signal sharing a rig with another
//! organisation is that sentence.
//!
//! A **project** is refused, and that is the same rule reaching its
//! conclusion rather than a missing walker: a project *is* its media, and
//! ADR 0003's rule is that subscribing moves names and not gigabytes.
//! Bytes at that size cross as a File Root — offered, accepted, pulled in
//! chunks by the lane that owns resumption and renditions — which
//! `tests/integration/tests/it/remote_assets.rs` proves end to end.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use architect::iroh_link::{self, iroh};
use tokio::sync::Mutex;
use vault_proto::{FileBytes, Manifest, VaultSyncError};
use wiki_live::source::{Source, SourceVault};
use wiki_live::source_peers::SourcePeers;
use wiki_live::subscriptions_backend::{Admission, LocalOrgs, Upstream};
use wiki_proto::WikiError;
use wiki_proto::service::subscriptions::SubscriptionsClient;
use wiki_proto::subscription::{SourceKind, Subscription};

/// How long a dial may take before the publisher counts as unreachable.
///
/// The same five seconds `files::IrohRemotes` allows, chosen against
/// discovery rather than against patience, and for the same reason: an
/// unreachable source is an orphan — the copy on disk goes on resolving
/// (`wiki.life.orphan`) — and a surface cannot report an orphan it is
/// still waiting for.
const DIAL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The endpoint this deployment dials peers from, and the connections it
/// has open to them.
///
/// Late-bound, because the endpoint is bound *after* `AppState` — the
/// same ordering `attach_peering` deals with on the files lane, and the
/// reason that one takes `&mut`. A resolver built before the endpoint
/// exists holds this and resolves nothing remote until it is filled, at
/// which point every clone of it can dial: an `Arc` to one cell rather
/// than a copy per clone.
#[derive(Debug, Default)]
pub struct Dialler {
    endpoint: OnceLock<iroh::Endpoint>,
    /// One live connection per peer. A vox connection is a stream on a
    /// QUIC connection, so a refresh fetching thirty pages opens thirty
    /// streams on one handshake instead of thirty handshakes.
    pool: Mutex<HashMap<iroh::EndpointId, iroh::endpoint::Connection>>,
}

impl Dialler {
    /// Give this deployment its endpoint. Calling twice keeps the first
    /// — an endpoint is an identity, and swapping it under a live pool
    /// would leave connections dialled from somewhere the peer no longer
    /// admits.
    pub fn attach(&self, endpoint: iroh::Endpoint) {
        let _ = self.endpoint.set(endpoint);
    }

    /// Open a subscriptions lane on `peer`.
    ///
    /// By bare id: the endpoint resolves it through whatever address
    /// lookup it was bound with — n0's DNS in a deployment, a seeded
    /// book where there is nothing to discover.
    async fn lane(&self, peer: &str) -> Result<SubscriptionsClient, VaultSyncError> {
        let endpoint = self.endpoint.get().ok_or_else(|| {
            // Not an outage: this process has no endpoint at all, so no
            // peer is reachable and none will become reachable by
            // retrying. Said rather than reported as a missing file.
            VaultSyncError::Io(
                "this server dials no peers: its iroh endpoint was never attached, so a \
                 source on another server cannot be reached from here"
                    .to_owned(),
            )
        })?;
        let id: iroh::EndpointId = peer
            .parse()
            .map_err(|e| VaultSyncError::Io(format!("`{peer}` is not an endpoint id: {e}")))?;

        // The pooled connection first, a fresh one if it has died. A
        // stale entry — the peer restarted, the NAT rebound — surfaces at
        // `open_bi`, so it costs one extra round trip rather than an
        // error the caller has to interpret.
        let link = match self.pooled(id).await {
            Some(connection) => match Self::open(connection).await {
                Ok(link) => link,
                Err(_) => {
                    self.pool.lock().await.remove(&id);
                    Self::open(self.redial(endpoint, id).await?).await?
                }
            },
            None => Self::open(self.redial(endpoint, id).await?).await?,
        };
        architect::vox::initiator_on(link)
            .establish()
            .await
            .map_err(|e| VaultSyncError::Io(format!("establish on {peer}: {e}")))
    }

    async fn pooled(&self, id: iroh::EndpointId) -> Option<iroh::endpoint::Connection> {
        self.pool.lock().await.get(&id).cloned()
    }

    async fn redial(
        &self,
        endpoint: &iroh::Endpoint,
        id: iroh::EndpointId,
    ) -> Result<iroh::endpoint::Connection, VaultSyncError> {
        let dialled =
            tokio::time::timeout(DIAL_TIMEOUT, endpoint.connect(id, iroh_link::VOX_ALPN)).await;
        let connection = match dialled {
            Ok(Ok(connection)) => connection,
            Ok(Err(e)) => return Err(VaultSyncError::Io(format!("dial {id}: {e}"))),
            Err(_elapsed) => {
                return Err(VaultSyncError::Io(format!(
                    "{id} did not answer within {}s; the copy already held still resolves",
                    DIAL_TIMEOUT.as_secs()
                )));
            }
        };
        self.pool.lock().await.insert(id, connection.clone());
        Ok(connection)
    }

    async fn open(
        connection: iroh::endpoint::Connection,
    ) -> Result<iroh_link::IrohLink, VaultSyncError> {
        let (send, recv) = connection
            .open_bi()
            .await
            .map_err(|e| VaultSyncError::Io(format!("open stream: {e}")))?;
        Ok(iroh_link::IrohLink::new(connection, send, recv))
    }
}

/// One source on another server, as something a refresh can read.
///
/// Holds no connection: a refresh is minutes apart at best, and a lane
/// held open across that is a lane that has died. What it holds is the
/// three facts a call needs — which peer, which source, which secret —
/// and the pool that turns the first of them into a stream.
pub struct RemoteSource {
    dialler: Arc<Dialler>,
    /// The publisher's endpoint id, from the org's peer table.
    peer: String,
    kind: SourceKind,
    /// The slug **as the publisher names it**. A subscriber may hold the
    /// copy under any local name; what crosses the wire is the
    /// publisher's.
    slug: String,
    secret: String,
    /// The runtime to drive the wire on. Captured where this is built —
    /// inside the server's runtime — because the thread that calls in is
    /// a blocking one, and a blocking thread has no runtime of its own.
    runtime: tokio::runtime::Handle,
}

impl RemoteSource {
    /// Run one call to the publisher, from a thread that may block.
    ///
    /// The invariant: a caller must be off the reactor, which every
    /// dispatched `SubscriptionsBackend` method is. Misuse fails loudly
    /// rather than hanging — a single-threaded runtime is refused here
    /// with the reason (its only thread is the one that would park), and
    /// a reactor thread on a multi-threaded runtime gets tokio's own
    /// panic, which the dispatcher turns into a failed call.
    fn block_on<T>(
        &self,
        call: impl std::future::Future<Output = Result<T, VaultSyncError>>,
    ) -> Result<T, VaultSyncError> {
        if matches!(
            tokio::runtime::Handle::try_current().map(|h| h.runtime_flavor()),
            Ok(tokio::runtime::RuntimeFlavor::CurrentThread)
        ) {
            return Err(VaultSyncError::Io(
                "a remote wiki source was read on a single-threaded runtime's only thread: \
                 this bridge blocks on the wire, so it has to be called from a thread that \
                 may block (`spawn_blocking`, which is where a dispatched backend method \
                 runs)"
                    .to_owned(),
            ));
        }
        self.runtime.block_on(call)
    }

    async fn lane(&self) -> Result<SubscriptionsClient, VaultSyncError> {
        self.dialler.lane(&self.peer).await
    }
}

/// The publisher's refusals, as the refresh's own error type.
///
/// `NotFound` survives as `NotFound` deliberately: a refresh treats a
/// file that has gone away between the listing and the fetch differently
/// from a source that will not answer, and flattening both into `Io`
/// would make a race look like an outage.
fn wire_err(e: architect::vox::VoxError<WikiError>) -> VaultSyncError {
    match e {
        architect::vox::VoxError::User(user) => match *user {
            WikiError::NotFound(_) => VaultSyncError::NotFound,
            other => VaultSyncError::Io(other.to_string()),
        },
        other => VaultSyncError::Io(other.to_string()),
    }
}

impl SourceVault for RemoteSource {
    /// `vault_id` is the subscriber's name for the copy; the publisher is
    /// asked about its own slug, which is what the grant was minted for.
    fn manifest(&self, _vault_id: &str) -> Result<Manifest, VaultSyncError> {
        self.block_on(async {
            let lane = self.lane().await?;
            lane.source_manifest(self.kind, self.slug.clone(), self.secret.clone())
                .await
                .map_err(wire_err)
        })
    }

    fn get_file(&self, _vault_id: &str, path: &str) -> Result<FileBytes, VaultSyncError> {
        self.block_on(async {
            let lane = self.lane().await?;
            lane.source_file(
                self.kind,
                self.slug.clone(),
                path.to_owned(),
                self.secret.clone(),
            )
            .await
            .map_err(wire_err)
        })
    }
}

/// [`LocalOrgs`], plus the orgs this one has been granted access to
/// elsewhere.
///
/// t[impl wiki.subscribe.federated] — a source on another server
/// subscribes through the same surface as one in the same org: same id
/// shape, same staleness reporting, same resolution. What differs is
/// where the two read calls land, and nothing above this knows. Holds for
/// a wiki and for an asset shelf; a project and a Resource say plainly
/// which route they take instead (see the module docs).
///
/// Local first, always. A domain that names an org on this data root is
/// served from disk — no dial, no secret, and the publisher's own
/// visibility check in the same process. Only a domain this disk does not
/// hold is looked up in the peer table.
pub struct FederatedOrgs {
    local: LocalOrgs,
    /// The subscriber's own org root: where its peer table lives. One
    /// resolver per org, because a grant is one org's to hold — the
    /// server is not a broker that lends its peers to its other orgs.
    org_root: PathBuf,
    dialler: Arc<Dialler>,
    runtime: tokio::runtime::Handle,
}

impl FederatedOrgs {
    /// Wrap a local resolver for the org rooted at `org_root`.
    ///
    /// Must be built inside the server's runtime: the handle captured
    /// here is what a blocking thread later drives a call on.
    #[must_use]
    pub fn new(local: LocalOrgs, org_root: PathBuf, dialler: Arc<Dialler>) -> Self {
        Self {
            local,
            org_root,
            dialler,
            runtime: tokio::runtime::Handle::current(),
        }
    }

    fn peers(&self) -> SourcePeers {
        SourcePeers::open(&self.org_root)
    }

    /// The source on another server, when this org holds both halves of
    /// a grant for it.
    ///
    /// Both halves: an endpoint for the domain *and* a secret for this
    /// source. Either alone resolves nothing — a peer with no secret for
    /// what is asked would dial and be refused, which is a round trip
    /// spent learning what the table already knew.
    fn remote(&self, subscription: &Subscription) -> Option<Arc<dyn SourceVault>> {
        let peers = self.peers();
        let peer = peers.endpoint(&subscription.domain)?;
        let secret = peers.secret(&subscription.domain, subscription.kind, &subscription.slug)?;
        Some(Arc::new(RemoteSource {
            dialler: self.dialler.clone(),
            peer,
            kind: subscription.kind,
            slug: subscription.slug.clone(),
            secret,
            runtime: self.runtime.clone(),
        }))
    }
}

impl Upstream for FederatedOrgs {
    fn source(&self, subscription: &Subscription) -> Option<Source> {
        self.local
            .source(subscription)
            .or_else(|| self.remote(subscription).map(Source::Remote))
    }

    /// `Unknown` for a source this org holds a grant for; otherwise the
    /// local resolver's answer, unchanged.
    ///
    /// `Unknown` means orphan, and it is the right answer here for a
    /// reason worth stating: **only the publisher can say.** Whether VNT
    /// still admits ACME is decided by VNT's own `admits`, on every call,
    /// against the secret and the source's live visibility
    /// (`wiki.access.visibility`). A second opinion computed on this side
    /// would be a second rule for one question — and a withdrawn grant
    /// would then break a subscription instead of stopping its refresh.
    ///
    /// Why it has to be said rather than inherited: `wiki_domains` gives
    /// every example org an `<name>.test` domain whether or not that org
    /// is on *this* disk, so `LocalOrgs` recognises `vnt.test`, looks for
    /// `orgs/vnt-video/wikis/<slug>` beside its own, finds nothing, and
    /// refuses with "has no wiki" — an authoritative answer about
    /// somebody else's tree. That refusal is correct while this server is
    /// the only place a source could be. Once a grant names another
    /// server as the place, it is exactly wrong, and the grant is what
    /// tells the two cases apart.
    /// A vault is the one exception, and it is not a special case so much
    /// as the rule holding: `wiki.boundary.no-subscribe` is a fact about
    /// what a vault *is*, not about where it is, so the name is refused
    /// with its explanation on this side rather than dialled somewhere to
    /// be refused there.
    fn admits(&self, subscriber_org: &str, subscription: &Subscription) -> Admission {
        let local = self.local.admits(subscriber_org, subscription);
        let never_subscribable = matches!(subscription.slug.as_str(), "vault" | "default");
        if never_subscribable || !matches!(self.source(subscription), Some(Source::Remote(_))) {
            return local;
        }
        Admission::Unknown
    }

    /// What this data root publishes, and nothing from a peer.
    ///
    /// Discovery is a directory of what is *open* to subscription
    /// (`wiki.access.directory`); a peer's grant is not that — it names
    /// one source already offered to this org in particular, so listing
    /// it as discoverable would invite somebody to subscribe to a thing
    /// they were not offered.
    fn discover(&self, subscriber_org: &str) -> Vec<Subscription> {
        self.local.discover(subscriber_org)
    }

    fn domain_of(&self, org: &str) -> Option<String> {
        self.local.domain_of(org)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiki_proto::subscription::Selection;

    fn subscription(domain: &str, slug: &str, kind: SourceKind) -> Subscription {
        Subscription {
            domain: domain.into(),
            slug: slug.into(),
            kind,
            title: String::new(),
            core: false,
            declined: false,
            selection: Selection::default(),
        }
    }

    /// A data root holding one org, `acme`, and a subscriber `alice`.
    fn world() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let wiki = dir.path().join("orgs/acme/wikis/theory");
        std::fs::create_dir_all(&wiki).unwrap();
        std::fs::write(wiki.join("Page.md"), "# Page\n").unwrap();
        std::fs::create_dir_all(dir.path().join("orgs/alice")).unwrap();
        dir
    }

    fn federated(dir: &tempfile::TempDir) -> FederatedOrgs {
        let domains: HashMap<String, String> = [
            ("acme.test".to_owned(), "acme".to_owned()),
            ("alice.test".to_owned(), "alice".to_owned()),
        ]
        .into_iter()
        .collect();
        FederatedOrgs::new(
            LocalOrgs::new(dir.path().to_path_buf(), domains),
            dir.path().join("orgs/alice"),
            Arc::new(Dialler::default()),
        )
    }

    #[tokio::test]
    async fn a_source_on_this_disk_is_read_from_disk_and_never_dialled() {
        let dir = world();
        let upstream = federated(&dir);
        // Even with a grant recorded for it — the peer table is the
        // second place to look, not the first.
        SourcePeers::open(&dir.path().join("orgs/alice"))
            .trust(
                "acme.test",
                "not-an-endpoint",
                SourceKind::Wiki,
                "theory",
                "s",
            )
            .unwrap();
        assert!(matches!(
            upstream.source(&subscription("acme.test", "theory", SourceKind::Wiki)),
            Some(Source::Local(_))
        ));
    }

    #[tokio::test]
    async fn a_domain_this_disk_does_not_hold_resolves_through_the_peer_table() {
        let dir = world();
        let upstream = federated(&dir);
        let sub = subscription("vnt.test", "cut", SourceKind::Wiki);
        // Nothing recorded: an orphan, which is what it was before this
        // module existed.
        assert!(upstream.source(&sub).is_none());

        SourcePeers::open(&dir.path().join("orgs/alice"))
            .trust("vnt.test", "endpoint-1", SourceKind::Wiki, "cut", "s3cret")
            .unwrap();
        assert!(matches!(upstream.source(&sub), Some(Source::Remote(_))));
    }

    #[tokio::test]
    async fn a_peer_without_a_secret_for_this_source_is_still_an_orphan() {
        let dir = world();
        let upstream = federated(&dir);
        SourcePeers::open(&dir.path().join("orgs/alice"))
            .trust("vnt.test", "endpoint-1", SourceKind::Wiki, "cut", "s3cret")
            .unwrap();
        // The peer is known and this source is not what it granted.
        assert!(
            upstream
                .source(&subscription("vnt.test", "reel", SourceKind::Wiki))
                .is_none()
        );
    }

    /// A remote source with no endpoint attached says what is wrong
    /// rather than hanging or reporting a missing file — the state a
    /// server is in before `attach_wiki_peering` runs, and the state a
    /// deployment with peering off stays in.
    ///
    /// Multi-threaded deliberately, which is the runtime the server
    /// actually has: on a single-threaded one this bridge refuses
    /// outright (see [`RemoteSource::block_on`]) and the test would be
    /// asserting the guard rather than the dial.
    #[tokio::test(flavor = "multi_thread")]
    async fn without_an_attached_endpoint_a_remote_read_says_so() {
        let dir = world();
        let upstream = federated(&dir);
        SourcePeers::open(&dir.path().join("orgs/alice"))
            .trust("vnt.test", "endpoint-1", SourceKind::Wiki, "cut", "s3cret")
            .unwrap();
        let Some(Source::Remote(vault)) =
            upstream.source(&subscription("vnt.test", "cut", SourceKind::Wiki))
        else {
            panic!("the peer table holds both halves of a grant for this source");
        };
        // Off the reactor, as a dispatched backend method runs.
        let err = tokio::task::spawn_blocking(move || vault.manifest("cut"))
            .await
            .unwrap()
            .expect_err("no endpoint, no peer");
        assert!(
            err.to_string().contains("dials no peers"),
            "unhelpful error: {err}"
        );
    }
}
