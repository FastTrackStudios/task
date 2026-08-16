//! The Files placement layer end to end over an in-process
//! `architect::LocalServer` — the spec's Testing Decisions primary seam
//! ("the established idiom … the session facade's memory-link bootstrap
//! tests are the prior art"), the same one `files`' own
//! `tests/rpc_surface.rs` uses.
//!
//! All three lanes are mounted on one router here, exactly as a
//! deployment mounts them on three (operator on the server lane, org on
//! each org's, agent wherever agents connect) — the split is about who
//! can reach what, and this file asserts that split from the outside:
//! nothing below reaches into `StorageCore` to check private state.
//!
//! Covers every acceptance criterion of issue #262:
//!
//! 1. an operator registers a location; an org without a grant cannot
//!    place anything on it;
//! 2. grants enforce logical-byte quota and path prefix;
//! 3. an agent announces, is approved, and hosts a root's live tree +
//!    authoritative repo;
//! 4. a second location can hold blob replicas of the same root
//!    (placement is a separate axis from the live tree);
//!
//! plus a regression for each finding of the PR #284 review — operator
//! authorization, agent credentials, confinement before creation,
//! cross-org isolation, concurrent opens, failed-placement retry, quota
//! without an explicit refresh, and re-approval restoring health.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use architect::{LayerRouter, LocalServer, Scope};
use files_storage::core::{in_server_announcement, server_volume};
use files_storage::proto::service::org::StorageServiceStreamSource as _;
use files_storage::{
    AgentAnnouncement, AgentCredential, AgentDirective, AgentHosting, AgentStatus, AnnouncedVolume,
    AuthorizeFuture, CapabilityClass, DirectiveOutcome, GrantSpec, InServerAgent, LocationKind,
    OperatorAuth, PlacementStatus, StorageAdminBackend, StorageAdminServiceClient,
    StorageAgentBackend, StorageAgentServiceClient, StorageBackend, StorageCore, StorageError,
    StorageEvent, StorageServiceClient, StorageServiceStreamClient, storage_admin_layer,
    storage_agent_layer, storage_agent_stream_layer, storage_service_layer,
    storage_service_stream_layer,
};
use uuid::Uuid;

const ORG: &str = "acme";
/// The only session token the harness's operator gate accepts.
const OPERATOR: &str = "operator-session";

/// Stand-in for the server's `HomeOrgOperator`: accepts exactly one
/// token, so a test can prove the lane is closed to everyone else.
struct FakeOperatorAuth;

impl OperatorAuth for FakeOperatorAuth {
    fn authorize<'a>(&'a self, session_token: &'a str) -> AuthorizeFuture<'a> {
        let ok = session_token == OPERATOR;
        Box::pin(async move {
            if ok {
                Ok(())
            } else {
                Err(StorageError::Unauthorized("bad operator session".into()))
            }
        })
    }
}

/// One deployment: a registry, an in-server agent, and all three lanes on
/// one router.
struct Harness {
    _dir: tempfile::TempDir,
    agent_id: Uuid,
    agent: Arc<InServerAgent>,
    /// The in-server agent's enrollment secret, as handed back by
    /// `announce`.
    credential: AgentCredential,
    core: Arc<StorageCore>,
    scope: Arc<Scope>,
    admin: StorageAdminServiceClient,
    org: StorageServiceClient,
    agents: StorageAgentServiceClient,
    org_backend: StorageBackend,
    local: LocalServer,
}

impl Harness {
    /// Build the deployment and enroll the in-server agent with the given
    /// volumes (pending approval).
    async fn with_volumes(volumes: Vec<AnnouncedVolume>) -> Self {
        let dir = tempfile::tempdir().expect("deployment tempdir");

        let core = StorageCore::open(dir.path().join("storage")).expect("registry");
        let agent_id = Uuid::new_v4();
        let agent = Arc::new(InServerAgent::new(agent_id));
        core.register_local_agent(agent.clone());

        let org_backend = StorageBackend::new(core.clone(), ORG);
        let router = LayerRouter::new()
            .merge(storage_admin_layer(StorageAdminBackend::new(
                core.clone(),
                Arc::new(FakeOperatorAuth),
            )))
            .merge(storage_agent_layer(StorageAgentBackend::new(core.clone())))
            .merge(storage_agent_stream_layer(StorageAgentBackend::new(
                core.clone(),
            )))
            .merge(storage_service_layer(org_backend.clone()))
            .merge(storage_service_stream_layer(org_backend.clone()));

        let scope = Scope::new();
        let local = LocalServer::serve(router, scope.clone());
        let admin: StorageAdminServiceClient = local.establish().await.expect("admin client");
        let org: StorageServiceClient = local.establish().await.expect("org client");
        let agents: StorageAgentServiceClient = local.establish().await.expect("agent client");

        let enrollment = agents
            .announce(in_server_announcement(
                agent_id,
                "task-server",
                None,
                volumes,
            ))
            .await
            .expect("announce rpc");
        let credential = AgentCredential {
            agent_id,
            token: enrollment
                .token
                .expect("first enrollment mints the agent's secret"),
        };

        Self {
            _dir: dir,
            agent_id,
            agent,
            credential,
            core,
            scope,
            admin,
            org,
            agents,
            org_backend,
            local,
        }
    }

    /// A volume directory on disk, announced with the given capabilities.
    fn volume_spec(root: &Path, key: &str, capabilities: Vec<CapabilityClass>) -> AnnouncedVolume {
        let path = root.join(key);
        std::fs::create_dir_all(&path).unwrap();
        AnnouncedVolume {
            key: key.to_string(),
            name: format!("{key} volume"),
            kind: LocationKind::ServerVolume,
            root_path: path.to_str().unwrap().to_string(),
            capabilities,
            capacity_bytes: None,
        }
    }

    /// Approve the in-server agent and return its first location.
    async fn approve(&self) -> files_storage::StorageLocationInfo {
        self.admin
            .approve_agent(OPERATOR.to_string(), self.agent_id, true)
            .await
            .expect("approve_agent rpc");
        self.admin
            .list_locations(OPERATOR.to_string())
            .await
            .expect("list_locations rpc")
            .into_iter()
            .next()
            .expect("approval registered a location")
    }

    async fn grant(&self, location_id: Uuid, capabilities: Vec<CapabilityClass>, quota: u64) {
        self.admin
            .issue_grant(
                OPERATOR.to_string(),
                GrantSpec {
                    org: ORG.to_string(),
                    location_id,
                    capabilities,
                    quota_bytes: quota,
                    path_prefix: "orgs/acme".to_string(),
                },
            )
            .await
            .expect("issue_grant rpc");
    }

    async fn close(self) {
        self.agent.shutdown().await;
        drop(self.admin);
        drop(self.org);
        drop(self.agents);
        drop(self.org_backend);
        self.scope.close().await;
        drop(self.local);
        drop(self.core);
    }
}

/// Write a file into a hosted live tree through the agent's own
/// authoritative repo — the only handle allowed on that store — so the
/// root has real content to measure and replicate. Stands in for the
/// cadence engine (issue #260), which is what writes checkpoints for
/// real.
fn checkpoint_into(agent: &InServerAgent, live_tree: &Path, name: &str, content: &[u8]) {
    use jj_lib::repo::Repo as _;
    use files_store::version::checkpoint::{Change, checkpoint};

    let repo = agent.repo_at_head(live_tree).expect("authoritative repo");
    let parent = repo
        .view()
        .heads()
        .iter()
        .next()
        .cloned()
        .unwrap_or_else(|| repo.store().root_commit_id().clone());
    let path = jj_lib::repo_path::RepoPathBuf::from_internal_string(name).unwrap();
    pollster::block_on(checkpoint(
        &repo,
        parent,
        vec![Change::Write {
            path,
            content: content.to_vec(),
        }],
        "test content",
    ))
    .expect("checkpoint");
}

/// The application error behind a failed RPC call. Every lane's errors
/// arrive wrapped in vox's transport envelope; a transport-level failure
/// in these tests is a bug in the harness, not an expected outcome.
fn app_err<T: std::fmt::Debug>(result: Result<T, vox::VoxError<StorageError>>) -> StorageError {
    match result {
        Err(vox::VoxError::User(err)) => *err,
        other => panic!("expected an application error, got {other:?}"),
    }
}

/// The canonicalized granted prefix on a location — what a live tree's
/// path must start with (the agent resolves symlinks, so a textual
/// comparison against the un-canonicalized path can differ, e.g. under
/// macOS's `/var` → `/private/var`).
fn prefix_of(location: &files_storage::StorageLocationInfo) -> PathBuf {
    let raw = Path::new(&location.root_path).join("orgs/acme");
    raw.canonicalize().unwrap_or(raw)
}

async fn next_event(rx: &mut vox::Rx<StorageEvent>) -> StorageEvent {
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for a StorageEvent")
        .expect("event channel errored")
        .expect("event stream closed early");
    let mut copied = None;
    let _ = frame.map(|ev| copied = Some(ev));
    copied.expect("SelfRef::map ran")
}

async fn next_directive(rx: &mut vox::Rx<AgentDirective>) -> AgentDirective {
    let frame = tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("timed out waiting for an AgentDirective")
        .expect("directive channel errored")
        .expect("directive stream closed early");
    let mut copied = None;
    let _ = frame.map(|d| copied = Some(d));
    copied.expect("SelfRef::map ran")
}

/// Acceptance criteria 1 and 3: an agent announces, the operator
/// approves and thereby registers its volume, and only then — and only
/// with a grant — can an org place a root's live tree on it, repo and
/// all.
#[tokio::test(flavor = "multi_thread")]
async fn agent_approval_grant_then_hosting() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees, CapabilityClass::Blobs],
    )])
    .await;
    let root_id = Uuid::new_v4();

    // Announced, not approved: nothing is a location yet.
    let announced = h
        .admin
        .list_agents(OPERATOR.to_string())
        .await
        .expect("list_agents rpc");
    assert_eq!(announced.len(), 1);
    assert_eq!(announced[0].status, AgentStatus::Pending);
    assert_eq!(announced[0].hosting, AgentHosting::InServer);
    assert!(
        h.admin
            .list_locations(OPERATOR.to_string())
            .await
            .expect("list_locations rpc")
            .is_empty(),
        "an unapproved agent's volumes are not locations"
    );

    // The operator approves — that is what registers the location.
    let location = h.approve().await;

    // Criterion 1: an org with no grant sees nothing and can place
    // nothing — even naming the location id directly.
    assert!(
        h.org
            .list_locations()
            .await
            .expect("org list_locations rpc")
            .is_empty(),
        "an ungranted location is invisible to the org lane"
    );
    let err_ungranted = app_err(
        h.org
            .place_root(root_id, location.id, "mix-session".to_string())
            .await,
    );
    assert!(
        matches!(err_ungranted, StorageError::NotGranted(_)),
        "placing without a grant must be refused: {err_ungranted:?}"
    );

    // Subscribe to the org's events before the grant lands.
    let stream: StorageServiceStreamClient = h.local.establish().await.expect("stream client");
    let (tx, mut rx) = vox::channel::<StorageEvent>();
    let subscription = tokio::spawn(async move {
        stream
            .events(tx)
            .await
            .expect("subscribe to storage events");
    });
    let hub = h.org_backend.events_hub().clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        while hub.subscriber_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("subscriber sink never reached the org hub");

    // The operator admits the org: live trees only, 1 MiB, own prefix.
    h.grant(location.id, vec![CapabilityClass::LiveTrees], 1024 * 1024)
        .await;
    let grant = h.org.list_grants().await.expect("list_grants rpc")[0].clone();
    assert_eq!(grant.used_bytes, 0);
    match next_event(&mut rx).await {
        StorageEvent::GrantIssued(g) => assert_eq!(g.id, grant.id),
        other => panic!("expected GrantIssued, got {other:?}"),
    }
    assert_eq!(
        h.org.list_locations().await.expect("org locations").len(),
        1,
        "a granted location becomes visible"
    );

    // Criterion 3: placement hosts the live tree AND its authoritative
    // repo, under the grant's prefix.
    let placement = h
        .org
        .place_root(root_id, location.id, "mix-session".to_string())
        .await
        .expect("place_root rpc");
    assert_eq!(placement.status, PlacementStatus::Hosted);
    let live_tree = placement.live_tree.clone().expect("live tree bound");
    assert_eq!(live_tree.location_id, location.id);
    assert!(
        live_tree.repo_initialized,
        "the hosting agent initialized the authoritative repo"
    );
    let tree_path = PathBuf::from(&live_tree.absolute_path);
    assert!(tree_path.is_dir(), "the live tree exists on the volume");
    assert!(
        tree_path.join(".fts-files").join("store").exists(),
        "the authoritative version-store repo lives with the live tree"
    );
    assert!(
        tree_path.starts_with(prefix_of(&location)),
        "the live tree sits under the grant's path prefix: {tree_path:?}"
    );
    match next_event(&mut rx).await {
        StorageEvent::PlacementChanged(p) => assert_eq!(p.root_id, root_id),
        other => panic!("expected PlacementChanged, got {other:?}"),
    }

    // A second live tree for the same root is refused — a root's live
    // tree sits wholly on one location.
    let err_again = app_err(
        h.org
            .place_root(root_id, location.id, "mix-session-2".to_string())
            .await,
    );
    assert!(
        matches!(err_again, StorageError::AlreadyExists(_)),
        "a root already has its live tree: {err_again:?}"
    );

    // Revoking admission closes the lane again without touching data.
    h.admin
        .revoke_grant(OPERATOR.to_string(), grant.id)
        .await
        .expect("revoke_grant rpc");
    let err_revoked = app_err(
        h.org
            .place_root(Uuid::new_v4(), location.id, "other".to_string())
            .await,
    );
    assert!(
        matches!(err_revoked, StorageError::NotGranted(_)),
        "a revoked grant places nothing"
    );
    assert!(
        tree_path.is_dir(),
        "revoking a grant never deletes what was already placed"
    );

    subscription.abort();
    h.close().await;
}

/// Acceptance criterion 2: the two grant terms with teeth — the path
/// prefix confines every path an org supplies, and the logical-byte
/// quota refuses growth past it.
///
/// Regression for review finding 7: the quota must bite **without the
/// org first calling `refresh_usage`**. Nothing below refreshes.
#[tokio::test(flavor = "multi_thread")]
async fn grants_enforce_prefix_and_quota() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees, CapabilityClass::Blobs],
    )])
    .await;
    let location = h.approve().await;
    // A deliberately tiny quota: 4 KiB of logical bytes.
    h.grant(location.id, vec![CapabilityClass::LiveTrees], 4096)
        .await;

    // Path prefix: nothing an org sends resolves outside its subtree.
    for escape in ["../elsewhere", "/etc", "a/../../../elsewhere", ""] {
        let err_attempt = app_err(
            h.org
                .place_root(Uuid::new_v4(), location.id, escape.to_string())
                .await,
        );
        assert!(
            matches!(err_attempt, StorageError::BadRequest(_)),
            "{escape:?} must not escape the grant's prefix: {err_attempt:?}"
        );
    }
    assert!(
        !dir.path().join("primary").join("elsewhere").exists(),
        "a refused placement creates nothing outside the prefix"
    );

    // Quota: place a root, fill it past the quota, and watch the next
    // placement be refused — with no `refresh_usage` call anywhere.
    let first = Uuid::new_v4();
    let placement = h
        .org
        .place_root(first, location.id, "big-session".to_string())
        .await
        .expect("place_root rpc");
    let live_tree = PathBuf::from(&placement.live_tree.unwrap().absolute_path);
    assert_eq!(
        placement.logical_bytes, 0,
        "a fresh live tree references nothing yet"
    );

    let payload = vec![b'a'; 8192];
    checkpoint_into(&h.agent, &live_tree, "session.wav", &payload);

    let usage = h.org.usage(location.id).await.expect("usage rpc");
    assert_eq!(usage.quota_bytes, 4096);
    assert!(
        usage.used_bytes >= payload.len() as u64,
        "usage re-measures the tree rather than reporting a stale zero: {usage:?}"
    );
    assert_eq!(usage.placements, 1);
    let grants = h.org.list_grants().await.expect("list_grants rpc");
    assert_eq!(
        grants[0].used_bytes, usage.used_bytes,
        "a grant's used_bytes is derived from placements, never a separate counter"
    );

    let err_refused = app_err(
        h.org
            .place_root(Uuid::new_v4(), location.id, "another-session".to_string())
            .await,
    );
    assert!(
        matches!(err_refused, StorageError::QuotaExceeded(_)),
        "a grant with no headroom takes no new placements: {err_refused:?}"
    );

    h.close().await;
}

/// Acceptance criterion 4: blob placement is a separate axis. A second
/// location that cannot host a live tree at all still holds a full blob
/// replica of the same root.
#[tokio::test(flavor = "multi_thread")]
async fn second_location_holds_blob_replicas() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![
        Harness::volume_spec(
            dir.path(),
            "primary",
            vec![CapabilityClass::LiveTrees, CapabilityClass::Blobs],
        ),
        // Blobs only — an archive volume that can never hold a live tree.
        Harness::volume_spec(dir.path(), "archive", vec![CapabilityClass::Blobs]),
    ])
    .await;
    h.approve().await;
    let locations = h
        .admin
        .list_locations(OPERATOR.to_string())
        .await
        .expect("locations");
    let primary = locations
        .iter()
        .find(|l| l.volume_key == "primary")
        .expect("primary registered")
        .clone();
    let archive = locations
        .iter()
        .find(|l| l.volume_key == "archive")
        .expect("archive registered")
        .clone();

    // Primary carries both classes on purpose: the reason a replica may
    // not live on the live tree's own location is that one location
    // holds one copy, not that the grant fell short.
    h.grant(
        primary.id,
        vec![CapabilityClass::LiveTrees, CapabilityClass::Blobs],
        1024 * 1024,
    )
    .await;
    h.grant(archive.id, vec![CapabilityClass::Blobs], 1024 * 1024)
        .await;

    // A grant may not exceed its location's own capabilities.
    let err_over_grant = app_err(
        h.admin
            .issue_grant(
                OPERATOR.to_string(),
                GrantSpec {
                    org: "other".to_string(),
                    location_id: archive.id,
                    capabilities: vec![CapabilityClass::LiveTrees],
                    quota_bytes: 1,
                    path_prefix: "orgs/other".to_string(),
                },
            )
            .await,
    );
    assert!(
        matches!(err_over_grant, StorageError::CapabilityDenied(_)),
        "a grant cannot offer what its location cannot do: {err_over_grant:?}"
    );

    let root_id = Uuid::new_v4();
    let placement = h
        .org
        .place_root(root_id, primary.id, "video-project".to_string())
        .await
        .expect("place_root rpc");
    let live_tree = PathBuf::from(&placement.live_tree.unwrap().absolute_path);
    checkpoint_into(&h.agent, &live_tree, "cut-01.mov", &vec![b'v'; 40_000]);
    checkpoint_into(&h.agent, &live_tree, "notes.txt", b"client feedback");
    let measured = h.org.refresh_usage(root_id).await.expect("refresh_usage");

    // The live tree's location cannot be its own replica…
    assert!(
        matches!(
            app_err(h.org.add_blob_replica(root_id, primary.id).await),
            StorageError::BadRequest(_)
        ),
        "a replica must live somewhere other than the live tree"
    );
    // …and the blob-only location cannot host a live tree.
    assert!(
        matches!(
            app_err(
                h.org
                    .place_root(Uuid::new_v4(), archive.id, "nope".to_string())
                    .await
            ),
            StorageError::CapabilityDenied(_)
        ),
        "a blob-only grant hosts no live tree"
    );

    // The replica itself.
    let replicated = h
        .org
        .add_blob_replica(root_id, archive.id)
        .await
        .expect("add_blob_replica rpc");
    assert_eq!(replicated.replicas.len(), 1);
    let replica = replicated.replicas[0].clone();
    assert_eq!(replica.location_id, archive.id);
    assert!(replica.synced_at.is_some(), "the replica synced");
    assert_eq!(
        replica.files_present, 2,
        "both saved files reached the replica: {replica:?}"
    );
    assert_eq!(
        replica.logical_bytes, measured.logical_bytes,
        "the replica holds the same logical bytes as the live tree"
    );
    let replica_path = PathBuf::from(&replica.absolute_path);
    assert!(
        replica_path.is_dir(),
        "the replica's chunk store is on the archive volume"
    );
    let archive_prefix = {
        let raw = Path::new(&archive.root_path).join("orgs/acme");
        raw.canonicalize().unwrap_or(raw)
    };
    assert!(
        replica_path.starts_with(archive_prefix),
        "the replica sits under the grant's prefix too: {replica_path:?}"
    );

    // Both axes charge their own location, independently.
    let primary_usage = h.org.usage(primary.id).await.expect("primary usage");
    let archive_usage = h.org.usage(archive.id).await.expect("archive usage");
    assert_eq!(primary_usage.used_bytes, measured.logical_bytes);
    assert_eq!(archive_usage.used_bytes, measured.logical_bytes);
    assert_eq!(archive_usage.placements, 1);

    // The live tree is untouched by replication — one root, one live
    // tree, N blob copies.
    let after = h.org.placement(root_id).await.expect("placement rpc");
    assert_eq!(
        after.live_tree.as_ref().unwrap().location_id,
        primary.id,
        "replication never moves the live tree"
    );

    h.close().await;
}

/// The storage-agent protocol as the other two hostings will speak it: a
/// remote agent enrolls, is approved, receives its directive over the
/// `#[subscribe]` stream, and reports the outcome — which is what flips
/// the placement to hosted.
///
/// Regression for review finding 2: the directive can only be completed
/// by the agent it was issued to, and *proving* that takes the agent's
/// secret, not its (public) id.
#[tokio::test(flavor = "multi_thread")]
async fn remote_agent_receives_directives_and_reports_back() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees],
    )])
    .await;

    // A second agent, deliberately NOT registered in-process — this is
    // the desktop/standalone hosting from the coordinator's view.
    let remote_id = Uuid::new_v4();
    let remote_root = dir.path().join("remote-drive");
    std::fs::create_dir_all(&remote_root).unwrap();
    let enrollment = h
        .agents
        .announce(AgentAnnouncement {
            agent_id: remote_id,
            token: None,
            hosting: AgentHosting::Standalone,
            label: "nas".to_string(),
            volumes: vec![AnnouncedVolume {
                key: "bulk".to_string(),
                name: "NAS bulk".to_string(),
                kind: LocationKind::ServerVolume,
                root_path: remote_root.to_str().unwrap().to_string(),
                capabilities: vec![CapabilityClass::LiveTrees],
                capacity_bytes: Some(1 << 40),
            }],
        })
        .await
        .expect("announce rpc");
    assert_eq!(enrollment.agent.status, AgentStatus::Pending);
    let remote = AgentCredential {
        agent_id: remote_id,
        token: enrollment.token.expect("enrollment mints a secret"),
    };

    // A pending agent's volume cannot be granted, because it is not a
    // location at all.
    let err_premature = app_err(
        h.admin
            .issue_grant(
                OPERATOR.to_string(),
                GrantSpec {
                    org: ORG.to_string(),
                    location_id: Uuid::new_v4(),
                    capabilities: vec![CapabilityClass::LiveTrees],
                    quota_bytes: 1024,
                    path_prefix: "orgs/acme".to_string(),
                },
            )
            .await,
    );
    assert!(
        matches!(err_premature, StorageError::NotFound(_)),
        "no location exists for an unapproved agent: {err_premature:?}"
    );
    assert!(
        matches!(
            app_err(
                h.admin
                    .register_location(OPERATOR.to_string(), remote_id, "bulk".to_string())
                    .await
            ),
            StorageError::AgentNotApproved(_)
        ),
        "an unapproved agent's volume cannot be registered"
    );

    h.admin
        .approve_agent(OPERATOR.to_string(), remote_id, true)
        .await
        .expect("approve_agent rpc");
    let location = h
        .admin
        .list_locations(OPERATOR.to_string())
        .await
        .expect("locations")
        .into_iter()
        .find(|l| l.agent_id == remote_id)
        .expect("the remote agent's volume is now a location");
    h.grant(location.id, vec![CapabilityClass::LiveTrees], 1 << 20)
        .await;

    // The agent subscribes to its directive stream, as an agent does on
    // connect.
    let stream: files_storage::StorageAgentServiceStreamClient =
        h.local.establish().await.expect("agent stream client");
    let (tx, mut rx) = vox::channel::<AgentDirective>();
    let subscription = tokio::spawn(async move {
        stream
            .directives(tx)
            .await
            .expect("subscribe to directives");
    });
    let hub = h.core.directives_hub().clone();
    tokio::time::timeout(Duration::from_secs(10), async {
        while hub.subscriber_count() == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("subscriber sink never reached the directive hub");

    let root_id = Uuid::new_v4();
    let placement = h
        .org
        .place_root(root_id, location.id, "remote-session".to_string())
        .await
        .expect("place_root rpc");
    assert_eq!(
        placement.status,
        PlacementStatus::Pending,
        "a remote agent's placement waits for the agent"
    );

    let directive = next_directive(&mut rx).await;
    assert_eq!(directive.agent_id, remote_id);
    let target = match &directive.kind {
        files_storage::DirectiveKind::HostLiveTree { target, .. } => target.clone(),
        other => panic!("expected HostLiveTree, got {other:?}"),
    };
    // Finding 3: the boundary travels WITH the directive, so a remote
    // hosting can enforce the grant's prefix itself.
    assert!(
        target.boundary.starts_with(remote_root.to_str().unwrap()),
        "the directive names a boundary on the agent's own volume"
    );
    assert_eq!(target.relative, "remote-session");

    let outstanding = h
        .agents
        .pending_directives(remote.clone())
        .await
        .expect("pending_directives rpc");
    assert_eq!(outstanding.len(), 1, "catch-up read sees the same work");
    assert_eq!(outstanding[0].id, directive.id);

    // Another agent may not answer for this one — and its own id is not
    // enough to try.
    let hosted_outcome = || DirectiveOutcome::Hosted {
        repo_initialized: true,
        absolute_path: format!("{}/remote-session", target.boundary),
    };
    assert!(
        matches!(
            app_err(
                h.agents
                    .complete_directive(h.credential.clone(), directive.id, hosted_outcome())
                    .await
            ),
            StorageError::Unauthorized(_)
        ),
        "a directive can only be completed by the agent it was issued to"
    );
    assert!(
        matches!(
            app_err(
                h.agents
                    .complete_directive(
                        AgentCredential {
                            agent_id: remote_id,
                            token: "guessed".to_string(),
                        },
                        directive.id,
                        hosted_outcome(),
                    )
                    .await
            ),
            StorageError::Unauthorized(_)
        ),
        "knowing the agent id is not knowing its secret"
    );

    h.agents
        .complete_directive(remote.clone(), directive.id, hosted_outcome())
        .await
        .expect("complete_directive rpc");

    let hosted = h.org.placement(root_id).await.expect("placement rpc");
    assert_eq!(hosted.status, PlacementStatus::Hosted);
    assert!(hosted.live_tree.unwrap().repo_initialized);
    assert!(
        h.agents
            .pending_directives(remote)
            .await
            .expect("pending_directives rpc")
            .is_empty(),
        "a completed directive stops being outstanding"
    );

    subscription.abort();
    h.close().await;
}

/// Review finding 1: every operator-lane method authorizes its caller.
/// `/server/vox` has no gate in front of it, so an unauthenticated
/// caller reaching `issue_grant` or `approve_agent` would own the
/// deployment.
#[tokio::test(flavor = "multi_thread")]
async fn operator_lane_refuses_an_unauthorized_session() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees],
    )])
    .await;

    for bad in ["", "not-the-operator"] {
        assert!(
            matches!(
                app_err(h.admin.list_agents(bad.to_string()).await),
                StorageError::Unauthorized(_)
            ),
            "list_agents must not answer {bad:?}"
        );
        assert!(
            matches!(
                app_err(h.admin.list_locations(bad.to_string()).await),
                StorageError::Unauthorized(_)
            ),
            "list_locations must not answer {bad:?}"
        );
        assert!(
            matches!(
                app_err(
                    h.admin
                        .approve_agent(bad.to_string(), h.agent_id, true)
                        .await
                ),
                StorageError::Unauthorized(_)
            ),
            "approve_agent must refuse {bad:?}"
        );
        assert!(
            matches!(
                app_err(
                    h.admin
                        .issue_grant(
                            bad.to_string(),
                            GrantSpec {
                                org: ORG.to_string(),
                                location_id: Uuid::new_v4(),
                                capabilities: vec![CapabilityClass::LiveTrees],
                                quota_bytes: u64::MAX,
                                path_prefix: "orgs/acme".to_string(),
                            },
                        )
                        .await
                ),
                StorageError::Unauthorized(_)
            ),
            "issue_grant must refuse {bad:?}"
        );
        assert!(
            matches!(
                app_err(h.admin.revoke_grant(bad.to_string(), Uuid::new_v4()).await),
                StorageError::Unauthorized(_)
            ),
            "revoke_grant must refuse {bad:?}"
        );
        assert!(
            matches!(
                app_err(
                    h.admin
                        .register_location(bad.to_string(), h.agent_id, "primary".to_string())
                        .await
                ),
                StorageError::Unauthorized(_)
            ),
            "register_location must refuse {bad:?}"
        );
        assert!(
            matches!(
                app_err(h.admin.list_grants(bad.to_string(), None).await),
                StorageError::Unauthorized(_)
            ),
            "list_grants must refuse {bad:?}"
        );
    }

    // None of that changed anything: the agent is still pending, so
    // nothing is placeable.
    assert_eq!(
        h.admin
            .list_agents(OPERATOR.to_string())
            .await
            .expect("list_agents rpc")[0]
            .status,
        AgentStatus::Pending,
        "a refused approve_agent must not have approved anything"
    );
    assert!(
        h.admin
            .list_grants(OPERATOR.to_string(), None)
            .await
            .expect("list_grants rpc")
            .is_empty(),
        "a refused issue_grant must not have admitted anyone"
    );

    h.close().await;
}

/// Review finding 2: an agent id is public, so re-announcing under
/// someone else's id must fail — otherwise an attacker rewrites an
/// approved agent's volume list (with a `root_path` of its choosing) and
/// inherits the approval.
#[tokio::test(flavor = "multi_thread")]
async fn agent_id_alone_cannot_hijack_an_approved_agent() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees],
    )])
    .await;
    let location = h.approve().await;
    let original_root = location.root_path.clone();

    // The id is right there in the org-visible location record.
    let hijack = |token: Option<String>| AgentAnnouncement {
        agent_id: location.agent_id,
        token,
        hosting: AgentHosting::Standalone,
        label: "attacker".to_string(),
        volumes: vec![AnnouncedVolume {
            key: "primary".to_string(),
            name: "anywhere".to_string(),
            kind: LocationKind::ServerVolume,
            root_path: "/".to_string(),
            capabilities: vec![CapabilityClass::LiveTrees, CapabilityClass::Blobs],
            capacity_bytes: None,
        }],
    };

    for attempt in [None, Some("guessed-secret".to_string())] {
        assert!(
            matches!(
                app_err(h.agents.announce(hijack(attempt.clone())).await),
                StorageError::Unauthorized(_)
            ),
            "re-announcing a known agent needs its secret (attempt: {attempt:?})"
        );
    }

    // Nothing moved: the volume, the approval and the location are as
    // they were.
    let agent = h
        .admin
        .list_agents(OPERATOR.to_string())
        .await
        .expect("list_agents rpc")
        .into_iter()
        .find(|a| a.id == location.agent_id)
        .expect("agent still there");
    assert_eq!(agent.label, "task-server", "the label was not rewritten");
    assert_eq!(agent.volumes[0].root_path, original_root);
    assert_eq!(agent.status, AgentStatus::Approved);
    let locations = h
        .admin
        .list_locations(OPERATOR.to_string())
        .await
        .expect("list_locations rpc");
    assert_eq!(locations.len(), 1);
    assert_eq!(locations[0].root_path, original_root);

    // The real agent, with its secret, still re-announces fine.
    let re = h
        .agents
        .announce(in_server_announcement(
            h.agent_id,
            "task-server",
            Some(h.credential.token.clone()),
            vec![Harness::volume_spec(
                dir.path(),
                "primary",
                vec![CapabilityClass::LiveTrees],
            )],
        ))
        .await
        .expect("legitimate re-announce");
    assert_eq!(re.agent.status, AgentStatus::Approved, "approval survives");
    assert!(
        re.token.is_none(),
        "the secret is minted once and never re-issued"
    );

    // And the credential gates the other three methods too.
    let forged = AgentCredential {
        agent_id: h.agent_id,
        token: "guessed".to_string(),
    };
    assert!(matches!(
        app_err(h.agents.heartbeat(forged.clone(), vec![]).await),
        StorageError::Unauthorized(_)
    ));
    assert!(matches!(
        app_err(h.agents.pending_directives(forged).await),
        StorageError::Unauthorized(_)
    ));

    h.close().await;
}

/// Review finding 3: a symlink planted inside the grant's prefix must be
/// refused **before** anything is created through it. The old ordering
/// dispatched first and checked afterwards, so the escape was detected
/// only once the directories — and a whole version store — already
/// existed outside the boundary.
#[tokio::test(flavor = "multi_thread")]
#[cfg(unix)]
async fn a_symlink_inside_the_prefix_creates_nothing_outside_it() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees],
    )])
    .await;
    let location = h.approve().await;
    h.grant(location.id, vec![CapabilityClass::LiveTrees], 1 << 20)
        .await;

    // Somewhere the org must never reach, and a link to it inside the
    // org's own granted subtree.
    let elsewhere = dir.path().join("another-orgs-data");
    std::fs::create_dir_all(&elsewhere).unwrap();
    let prefix = Path::new(&location.root_path).join("orgs/acme");
    std::fs::create_dir_all(&prefix).unwrap();
    std::os::unix::fs::symlink(&elsewhere, prefix.join("link")).unwrap();

    let root_id = Uuid::new_v4();
    let err = app_err(
        h.org
            .place_root(root_id, location.id, "link/escaped".to_string())
            .await,
    );
    assert!(
        matches!(err, StorageError::BadRequest(_) | StorageError::Io(_)),
        "an escape through a symlink must fail: {err:?}"
    );
    assert!(
        !elsewhere.join("escaped").exists(),
        "nothing may be created outside the boundary — not a directory, not a repo"
    );
    assert_eq!(
        std::fs::read_dir(&elsewhere).unwrap().count(),
        0,
        "the target of the symlink is untouched"
    );

    // And the failure did not wedge the root (finding 6): it can still
    // be placed somewhere legitimate.
    let ok = h
        .org
        .place_root(root_id, location.id, "legitimate".to_string())
        .await
        .expect("a failed placement must not wedge the root");
    assert_eq!(ok.status, PlacementStatus::Hosted);

    h.close().await;
}

/// Review finding 4: a root id belongs to an org. Org B naming org A's
/// root id must not resolve to — let alone overwrite — A's placement.
#[tokio::test(flavor = "multi_thread")]
async fn one_orgs_root_id_cannot_touch_anothers_placement() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees],
    )])
    .await;
    let location = h.approve().await;
    h.grant(location.id, vec![CapabilityClass::LiveTrees], 1 << 20)
        .await;

    // A second org, with its own grant on the same shared volume — its
    // own prefix, as a shared volume implies.
    let other_backend = StorageBackend::new(h.core.clone(), "rival");
    let other_scope = Scope::new();
    let other_server = LocalServer::serve(
        LayerRouter::new().merge(storage_service_layer(other_backend.clone())),
        other_scope.clone(),
    );
    let rival: StorageServiceClient = other_server.establish().await.expect("rival client");
    h.admin
        .issue_grant(
            OPERATOR.to_string(),
            GrantSpec {
                org: "rival".to_string(),
                location_id: location.id,
                capabilities: vec![CapabilityClass::LiveTrees],
                quota_bytes: 1 << 20,
                path_prefix: "orgs/rival".to_string(),
            },
        )
        .await
        .expect("issue_grant rpc");

    let shared_root_id = Uuid::new_v4();
    let mine = h
        .org
        .place_root(shared_root_id, location.id, "my-session".to_string())
        .await
        .expect("place_root rpc");
    let my_tree = mine.live_tree.clone().unwrap().absolute_path;

    // The rival places "the same" root id. It gets its OWN placement
    // under its OWN prefix, and mine is untouched.
    let theirs = rival
        .place_root(shared_root_id, location.id, "their-session".to_string())
        .await
        .expect("the rival's own placement");
    let their_tree = theirs.live_tree.clone().unwrap().absolute_path;
    assert_ne!(my_tree, their_tree);
    assert!(their_tree.contains("orgs/rival"));

    let mine_after = h
        .org
        .placement(shared_root_id)
        .await
        .expect("placement rpc");
    assert_eq!(
        mine_after.live_tree.unwrap().absolute_path,
        my_tree,
        "another org's placement must not rebind mine"
    );
    assert_eq!(mine_after.org, ORG);
    assert!(
        PathBuf::from(&my_tree).is_dir(),
        "my live tree is still where it was"
    );

    // Usage is per org too, even on one shared location.
    let mine_usage = h.org.usage(location.id).await.expect("usage rpc");
    assert_eq!(mine_usage.placements, 1, "I see only my own placement");

    other_scope.close().await;
    h.close().await;
}

/// Review finding 5: concurrent directives for one live tree must not
/// each open the version store. A second `VersionStoreBackend` on one
/// directory is the shape that wedged `files`' restart test.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_directives_on_one_live_tree_do_not_double_open() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees, CapabilityClass::Blobs],
    )])
    .await;
    let location = h.approve().await;
    h.grant(
        location.id,
        vec![CapabilityClass::LiveTrees, CapabilityClass::Blobs],
        1 << 30,
    )
    .await;

    let root_id = Uuid::new_v4();
    let placement = h
        .org
        .place_root(root_id, location.id, "busy-session".to_string())
        .await
        .expect("place_root rpc");
    let live_tree = PathBuf::from(&placement.live_tree.unwrap().absolute_path);
    checkpoint_into(&h.agent, &live_tree, "take.wav", &vec![b'x'; 20_000]);

    // Eight measurements at once, all against the same store.
    let mut tasks = Vec::new();
    for _ in 0..8 {
        let org = h.org.clone();
        tasks.push(tokio::spawn(async move {
            org.refresh_usage(root_id).await.expect("refresh_usage rpc")
        }));
    }
    let mut sizes = std::collections::HashSet::new();
    for task in tasks {
        let placement = tokio::time::timeout(Duration::from_secs(30), task)
            .await
            .expect("a concurrent measurement hung — the store was opened twice")
            .expect("measurement task panicked");
        sizes.insert(placement.logical_bytes);
    }
    assert_eq!(
        sizes.len(),
        1,
        "every concurrent measurement saw the same store: {sizes:?}"
    );
    assert!(sizes.into_iter().next().unwrap() >= 20_000);

    h.close().await;
}

/// Review finding 6: a placement that fails must release the root, and
/// `place_root` must report the failure as an error rather than an `Ok`
/// carrying `status: Failed`.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_placement_releases_the_root_for_a_retry() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees],
    )])
    .await;
    let location = h.approve().await;
    h.grant(location.id, vec![CapabilityClass::LiveTrees], 1 << 20)
        .await;

    // A plain file where the live tree's parent would go: hosting cannot
    // create a directory through it.
    let prefix = Path::new(&location.root_path).join("orgs/acme");
    std::fs::create_dir_all(&prefix).unwrap();
    std::fs::write(prefix.join("blocked"), b"in the way").unwrap();

    let root_id = Uuid::new_v4();
    let failed = h
        .org
        .place_root(root_id, location.id, "blocked/session".to_string())
        .await;
    assert!(
        failed.is_err(),
        "a placement that did not host must not resolve Ok: {failed:?}"
    );

    // The placement records the failure but no longer holds the root.
    let after = h.org.placement(root_id).await.expect("placement rpc");
    assert_eq!(after.status, PlacementStatus::Failed);
    assert!(after.failure.is_some(), "the reason is kept: {after:?}");
    assert!(
        after.live_tree.is_none(),
        "a failed placement releases its binding, or the root is wedged forever"
    );

    // The retry — the whole point — succeeds.
    let retried = h
        .org
        .place_root(root_id, location.id, "session".to_string())
        .await
        .expect("retry after fixing the path");
    assert_eq!(retried.status, PlacementStatus::Hosted);
    assert!(retried.failure.is_none());
    assert!(PathBuf::from(&retried.live_tree.unwrap().absolute_path).is_dir());

    h.close().await;
}

/// Review finding 10: re-approving an agent brings its locations back
/// online. Rejection takes them down, health is persisted, and the
/// in-server agent has no heartbeat to raise them again — so without
/// this, "pause by rejecting" is a one-way door.
#[tokio::test(flavor = "multi_thread")]
async fn re_approval_brings_a_rejected_agents_locations_back_online() {
    let dir = tempfile::tempdir().expect("volume tempdir");
    let h = Harness::with_volumes(vec![Harness::volume_spec(
        dir.path(),
        "primary",
        vec![CapabilityClass::LiveTrees],
    )])
    .await;
    let location = h.approve().await;
    h.grant(location.id, vec![CapabilityClass::LiveTrees], 1 << 20)
        .await;
    h.org
        .place_root(Uuid::new_v4(), location.id, "before".to_string())
        .await
        .expect("placing works while approved");

    // The operator pauses the agent.
    h.admin
        .approve_agent(OPERATOR.to_string(), h.agent_id, false)
        .await
        .expect("reject rpc");
    let paused = h
        .admin
        .list_locations(OPERATOR.to_string())
        .await
        .expect("list_locations rpc");
    assert_eq!(paused[0].health, files_storage::LocationHealth::Offline);
    assert!(
        matches!(
            app_err(
                h.org
                    .place_root(Uuid::new_v4(), location.id, "during".to_string())
                    .await
            ),
            StorageError::BadRequest(_)
        ),
        "an offline location takes no placements"
    );

    // …and un-pauses it.
    h.admin
        .approve_agent(OPERATOR.to_string(), h.agent_id, true)
        .await
        .expect("re-approve rpc");
    let resumed = h
        .admin
        .list_locations(OPERATOR.to_string())
        .await
        .expect("list_locations rpc");
    assert_eq!(
        resumed.len(),
        1,
        "re-approval must not register a duplicate location"
    );
    assert_eq!(resumed[0].health, files_storage::LocationHealth::Online);
    h.org
        .place_root(Uuid::new_v4(), location.id, "after".to_string())
        .await
        .expect("placing works again after re-approval");

    h.close().await;
}

/// The registry is deployment-scoped and survives a restart: a fresh
/// `StorageCore` over the same directory still knows the agent, the
/// location, the grant and the placement — and the agent's secret, so it
/// can re-announce.
#[tokio::test(flavor = "multi_thread")]
async fn registry_survives_a_restart() {
    let dir = tempfile::tempdir().expect("deployment tempdir");
    let volumes = dir.path().join("volumes");
    std::fs::create_dir_all(volumes.join("primary")).unwrap();
    let agent_id = Uuid::new_v4();
    let root_id = Uuid::new_v4();

    let token = {
        let core = StorageCore::open(dir.path().join("storage")).expect("registry");
        let agent = Arc::new(InServerAgent::new(agent_id));
        core.register_local_agent(agent.clone());
        let enrollment = core
            .announce(in_server_announcement(
                agent_id,
                "task-server",
                None,
                vec![server_volume(
                    "primary",
                    "Server primary",
                    &volumes.join("primary"),
                )],
            ))
            .expect("announce");
        core.approve_agent(agent_id, true).expect("approve");
        let location = core.list_locations()[0].clone();
        core.issue_grant(GrantSpec {
            org: ORG.to_string(),
            location_id: location.id,
            capabilities: vec![CapabilityClass::LiveTrees],
            quota_bytes: 1 << 20,
            path_prefix: "orgs/acme".to_string(),
        })
        .expect("grant");
        core.place_root(ORG, root_id, location.id, "session")
            .expect("place");
        agent.shutdown().await;
        enrollment.token.expect("enrollment secret")
    };

    let core = StorageCore::open(dir.path().join("storage")).expect("reopen registry");
    assert_eq!(core.list_agents().len(), 1);
    assert_eq!(core.list_agents()[0].status, AgentStatus::Approved);
    assert_eq!(core.list_locations().len(), 1);
    assert_eq!(core.list_grants(Some(ORG)).len(), 1);
    let placement = core.placement(ORG, root_id).expect("placement survived");
    assert_eq!(placement.status, PlacementStatus::Hosted);
    assert!(
        PathBuf::from(&placement.live_tree.unwrap().absolute_path).is_dir(),
        "the live tree is still where the registry says it is"
    );

    // The agent's enrollment survived too — a restart re-announces with
    // the same secret and keeps its approval.
    let re = core
        .announce(in_server_announcement(
            agent_id,
            "task-server",
            Some(token),
            vec![server_volume(
                "primary",
                "Server primary",
                &volumes.join("primary"),
            )],
        ))
        .expect("re-announce after restart");
    assert_eq!(re.agent.status, AgentStatus::Approved);
}
