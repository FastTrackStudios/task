//! The Files placement layer's home in the server (issue #262).
//!
//! The Storage Location registry is **deployment-scoped**, not per org:
//! one physical volume serves many orgs, and an org reaches it only
//! through a Storage grant. So there is exactly one [`StorageCore`] per
//! server process — built here, held as a field on [`crate::AppState`]
//! next to the data root it belongs to, and handed to both
//! `build_org_state` and `server_layer_router`.
//!
//! It used to be a process-global `OnceLock`, which was wrong three ways
//! (PR #284 review): a second `AppState` with a different data root
//! silently reused the first deployment's registry, two concurrent first
//! callers each ran the pre-`get_or_init` side effects (including
//! registry writes, which are last-writer-wins), and every caller
//! resolved `DataRoot::from_env` independently — so a vault-root-only
//! test server touched `$HOME/.task`. One owner, one construction, one
//! failure policy: **fatal**, at `AppState` construction, where the data
//! root is already resolved and ensured.
//!
//! On construction the server enrolls itself as a Storage agent (the
//! first of the three hostings) speaking for its own volume under
//! `<data_root>/files-volumes/`, plus anything `TASK_STORAGE_VOLUMES`
//! names.
//!
//! Enrollment always lands **pending**, and approval is what turns
//! announced volumes into Storage Locations — so the server approves
//! *its own* agent here. The approval gate exists to keep someone
//! else's machine out of the data path; this process already owns the
//! data root, and requiring a human to approve the server to itself
//! only dead-ends a first boot (see [`open`] for why it keys on the
//! locally-known agent id rather than the announcement's `hosting`).
//!
//! Grants remain a real decision — which orgs may reach which volume,
//! under which subtree — and are reconciled from `TASK_STORAGE_GRANTS`
//! (see [`configured_grants`]) because no client for the operator lane
//! exists yet, and a second process cannot safely write the registry
//! while the server holds it.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use files_storage::core::{in_server_announcement, registry_dir, server_volume};
use files_storage::{AgentStatus, InServerAgent, StorageCore, StorageError};
use serde::{Deserialize, Serialize};

/// The volume the server speaks for, under the data root.
fn volume_root(data_root: &Path) -> PathBuf {
    data_root.join("files-volumes").join("primary")
}

/// Extra volumes this server speaks for, from `TASK_STORAGE_VOLUMES`:
/// `key=/abs/path` pairs, comma-separated.
///
/// ```text
/// TASK_STORAGE_VOLUMES="media=/mnt/storage/Task"
/// ```
///
/// The in-server agent announces only `primary`, under the data root —
/// which on a cluster deployment is the server's own PVC. Media that
/// was never going to fit there (a NAS mount, an external volume) is
/// therefore unannounceable, and since a Storage Location can only be
/// admitted from an ANNOUNCED volume, a File Root on that media could
/// not be granted at all. This is how such a mount enters the registry.
///
/// A path that does not exist is SKIPPED with a warning rather than
/// failing the boot: the mount may simply not be present on this node
/// yet, and refusing to start a server because a media volume is
/// missing would take the whole instance down for a storage detail.
///
/// Malformed entries are likewise warned about and skipped — an
/// operator typo should cost one volume, not the deployment.
fn extra_volumes() -> Vec<(String, PathBuf)> {
    let Ok(raw) = std::env::var("TASK_STORAGE_VOLUMES") else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|entry| {
            let Some((key, path)) = entry.split_once('=') else {
                tracing::warn!(
                    entry,
                    "TASK_STORAGE_VOLUMES: expected key=/abs/path — skipped"
                );
                return None;
            };
            let (key, path) = (key.trim(), Path::new(path.trim()));
            if key.is_empty() || !path.is_absolute() {
                tracing::warn!(
                    entry,
                    "TASK_STORAGE_VOLUMES: needs a key and an absolute path — skipped"
                );
                return None;
            }
            if !path.is_dir() {
                tracing::warn!(
                    key,
                    path = %path.display(),
                    "TASK_STORAGE_VOLUMES: not a directory on this node — skipped"
                );
                return None;
            }
            Some((key.to_owned(), path.to_path_buf()))
        })
        .collect()
}

/// One org's admission onto one of this server's volumes, parsed from
/// `TASK_STORAGE_GRANTS`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GrantEntry {
    org: String,
    volume_key: String,
    path_prefix: String,
    quota_bytes: u64,
}

/// Org admissions onto this server's volumes, from `TASK_STORAGE_GRANTS`:
/// `org@volume:prefix[:quota]` entries, comma-separated.
///
/// ```text
/// TASK_STORAGE_GRANTS="cbu@media:cbu,tombrooksmusic@media:tombrooksmusic:8T"
/// ```
///
/// A location is deployment-scoped; an org's reach into it is a grant,
/// with the `prefix` naming the org's own subtree. Without one, a
/// location does not exist as far as that org's lane is concerned — so
/// announcing a volume is necessary but not sufficient.
///
/// **Why boot config rather than a CLI.** The registry is one JSON
/// document whose write ordering is a per-process sequence number, so a
/// second process editing it while the server runs is a last-writer-wins
/// clobber. Issuing grants in-process sidesteps that entirely. It is
/// also idempotent by construction: re-issuing for the same (org,
/// location) replaces the terms and keeps the grant's id, so this
/// reconciles rather than accumulates.
///
/// The proper answer is an authenticated operator client against
/// `/server/vox` (the lane already exists, with no client anywhere).
/// Until that exists this is the only safe way to admit an org.
///
/// Quota is optional with a `G`/`T` suffix; omitted means unlimited.
/// Note that a quota of literally zero admits *nothing* — the headroom
/// check refuses at `used >= quota` — so an unparsable quota must never
/// silently become 0. It skips the entry instead.
fn configured_grants() -> Vec<GrantEntry> {
    let Ok(raw) = std::env::var("TASK_STORAGE_GRANTS") else {
        return Vec::new();
    };
    raw.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(parse_grant_entry)
        .collect()
}

fn parse_grant_entry(entry: &str) -> Option<GrantEntry> {
    let warn = |why: &str| {
        tracing::warn!(
            entry,
            why,
            "TASK_STORAGE_GRANTS: expected org@volume:prefix[:quota] — skipped"
        );
        None::<GrantEntry>
    };
    let (org, rest) = entry.split_once('@')?;
    let mut parts = rest.split(':');
    let (Some(volume_key), Some(path_prefix)) = (parts.next(), parts.next()) else {
        return warn("needs volume and prefix");
    };
    let quota_bytes = match parts.next() {
        None => u64::MAX,
        Some(q) => parse_quota(q.trim())?,
    };
    if parts.next().is_some() {
        return warn("too many fields");
    }
    let (org, volume_key, path_prefix) = (org.trim(), volume_key.trim(), path_prefix.trim());
    if org.is_empty() || volume_key.is_empty() || path_prefix.is_empty() {
        return warn("empty field");
    }
    // The prefix must be a safe relative path — it is what confines the
    // org to its own subtree. Not checked here: `issue_grant` validates
    // it and is the authority, so a `..` surfaces as a refused grant
    // with the reason, rather than two rules that can drift apart.
    Some(GrantEntry {
        org: org.to_owned(),
        volume_key: volume_key.to_owned(),
        path_prefix: path_prefix.to_owned(),
        quota_bytes,
    })
}

/// `8T`, `500G`, or a plain byte count. `None` (with a warning) on
/// anything else — never a silent 0, which would admit nothing.
fn parse_quota(raw: &str) -> Option<u64> {
    let (digits, scale) = match raw.chars().last() {
        Some('T' | 't') => (&raw[..raw.len() - 1], 1u64 << 40),
        Some('G' | 'g') => (&raw[..raw.len() - 1], 1u64 << 30),
        _ => (raw, 1),
    };
    match digits.trim().parse::<u64>() {
        Ok(n) => n.checked_mul(scale).or_else(|| {
            tracing::warn!(
                quota = raw,
                "TASK_STORAGE_GRANTS: quota overflows — skipped"
            );
            None
        }),
        Err(_) => {
            tracing::warn!(
                quota = raw,
                "TASK_STORAGE_GRANTS: quota is not a byte count (try 8T, 500G) — skipped"
            );
            None
        }
    }
}

/// Issue the configured grants against the locations this agent's
/// volumes became. Each grant takes the location's own capabilities —
/// a grant may not exceed them, and there is nothing to choose from
/// here: an operator writing config is admitting the org to the volume,
/// not composing a capability subset.
///
/// A grant naming a volume this node did not announce is warned about
/// and skipped: on a deployment where the media mount is absent, the
/// server should still come up serving everything else.
fn reconcile_grants(core: &StorageCore, agent_id: uuid::Uuid) {
    let locations = core.list_locations();
    for entry in configured_grants() {
        let Some(location) = locations
            .iter()
            .find(|l| l.agent_id == agent_id && l.volume_key == entry.volume_key)
        else {
            tracing::warn!(
                org = entry.org,
                volume = entry.volume_key,
                "TASK_STORAGE_GRANTS: no such volume on this server — grant skipped"
            );
            continue;
        };
        let spec = files_storage::GrantSpec {
            org: entry.org.clone(),
            location_id: location.id,
            capabilities: location.capabilities.clone(),
            quota_bytes: entry.quota_bytes,
            path_prefix: entry.path_prefix.clone(),
        };
        match core.issue_grant(spec) {
            Ok(grant) => tracing::info!(
                org = entry.org,
                volume = entry.volume_key,
                prefix = entry.path_prefix,
                quota_bytes = entry.quota_bytes,
                grant = %grant.id,
                "files: storage grant reconciled"
            ),
            // One bad grant must not take the deployment down.
            Err(e) => tracing::warn!(
                org = entry.org,
                volume = entry.volume_key,
                error = %e,
                "files: storage grant refused"
            ),
        }
    }
}

/// The in-server agent's persisted identity: a stable id **and** the
/// enrollment secret it must present to re-announce. Both live beside
/// the registry so a restart comes back as the same agent, keeping its
/// approval, rather than arriving as a stranger.
#[derive(Debug, Serialize, Deserialize)]
struct AgentIdentity {
    id: uuid::Uuid,
    token: String,
}

/// Build the deployment's storage coordinator and enroll the in-server
/// agent. Called once per `AppState`.
pub fn open(data_root: &Path) -> eyre::Result<Arc<StorageCore>> {
    let core = StorageCore::open(registry_dir(data_root))
        .map_err(|e| eyre::eyre!("storage registry: {e}"))?;

    let volume = volume_root(data_root);
    std::fs::create_dir_all(&volume)?;

    // The in-server hosting: an ordinary agent that happens to live in
    // this process, so its directives are carried out inline.
    let identity = load_identity(data_root)?;
    let agent_id = identity
        .as_ref()
        .map(|i| i.id)
        .unwrap_or_else(uuid::Uuid::new_v4);
    core.register_local_agent(Arc::new(InServerAgent::new(agent_id)));

    let mut volumes = vec![server_volume("primary", "Server primary", &volume)];
    for (key, path) in extra_volumes() {
        tracing::info!(key, path = %path.display(), "announcing extra storage volume");
        volumes.push(server_volume(key.clone(), key, &path));
    }

    let enrollment = core
        .announce(in_server_announcement(
            agent_id,
            "task-server",
            identity.as_ref().map(|i| i.token.clone()),
            volumes,
        ))
        .map_err(|e| match e {
            // A stored token the coordinator rejects means the registry
            // and the identity file disagree — refuse rather than fork
            // the volume under a second agent id.
            StorageError::Unauthorized(m) => eyre::eyre!(
                "in-server storage agent {agent_id} failed to re-enroll ({m}); \
                 `storage.json` and `in-server-agent.json` disagree"
            ),
            other => eyre::eyre!("announce in-server storage agent: {other}"),
        })?;

    if let Some(token) = enrollment.token {
        // First enrollment: persist the secret we were just handed. It
        // is never transmitted again.
        store_identity(
            data_root,
            &AgentIdentity {
                id: agent_id,
                token,
            },
        )?;
    }

    // Approve our own agent. Enrollment always lands `Pending`, and a
    // location can only be registered on an APPROVED agent — so without
    // this a fresh deployment cannot use its own disk until a human
    // approves the server to itself. Approval exists to gate *someone
    // else's* machine offering storage; this process already owns the
    // data root.
    //
    // Keyed on the agent id we generated or loaded here, never on the
    // announcement's `hosting` field: that field arrives over the agent
    // lane verbatim, so approving anything that merely *claims* to be
    // in-server would let any remote enrollee approve itself.
    //
    // Pending only. An operator who deliberately rejected this agent
    // (decommissioning the server's local disk) must not have that
    // decision undone by a restart.
    let status = enrollment.agent.status;
    if status == AgentStatus::Pending {
        core.approve_agent(agent_id, true)
            .map_err(|e| eyre::eyre!("approving the in-server storage agent: {e}"))?;
    }

    tracing::info!(
        agent = %agent_id,
        status = ?status,
        self_approved = status == AgentStatus::Pending,
        volume = %volume.display(),
        "files: in-server storage agent enrolled"
    );

    // After approval, because approval is what turns announced volumes
    // into the locations these grants name.
    reconcile_grants(&core, agent_id);
    Ok(core)
}

fn identity_path(data_root: &Path) -> PathBuf {
    registry_dir(data_root).join("in-server-agent.json")
}

/// Read the persisted identity. **Absent is fine** (first boot);
/// present-but-unreadable is fatal.
///
/// Silently minting a new id on any read failure — a truncated file from
/// a crash mid-write, a stray byte, a transient `EACCES` — breaks the
/// stable-identity invariant in the worst way: the server re-announces
/// as a brand-new pending agent, and on approval the operator gets a
/// *second* location for the same physical volume while every existing
/// grant and placement still points at the first (PR #284 review).
fn load_identity(data_root: &Path) -> eyre::Result<Option<AgentIdentity>> {
    let path = identity_path(data_root);
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map(Some).map_err(|e| {
            eyre::eyre!(
                "{}: in-server storage agent identity is unreadable ({e}). Refusing to mint a \
                 new one — that would fork the volume under a second agent. Restore the file, \
                 or delete it AND the agent's entry in storage.json to re-enroll.",
                path.display()
            )
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(eyre::eyre!(
            "{}: cannot read the in-server storage agent identity: {e}",
            path.display()
        )),
    }
}

/// Write the identity atomically (tmp + rename), so a crash mid-write
/// leaves the previous file intact rather than a truncated one.
fn store_identity(data_root: &Path, identity: &AgentIdentity) -> eyre::Result<()> {
    let path = identity_path(data_root);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(identity)?)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

/// The operator-lane authorization the `StorageAdminService` runs on
/// `/server/vox`: a session token validated against the home org, the
/// same check `OrgManagementImpl::create_org` performs for the same
/// reason — that lane has no permission gate in front of it, so a
/// service mounted there authorizes its own callers or it authorizes
/// nobody.
pub struct HomeOrgOperator {
    state: crate::AppState,
}

impl HomeOrgOperator {
    #[must_use]
    pub fn new(state: crate::AppState) -> Self {
        Self { state }
    }
}

impl files_storage::OperatorAuth for HomeOrgOperator {
    fn authorize<'a>(&'a self, session_token: &'a str) -> files_storage::AuthorizeFuture<'a> {
        Box::pin(async move {
            let Some(home_slug) = self.state.home_slug() else {
                return Err(StorageError::Unauthorized(
                    "server has no home org — cannot validate an operator session".into(),
                ));
            };
            if session_token.is_empty() {
                return Err(StorageError::Unauthorized(
                    "missing session token (storage administration is an operator action)".into(),
                ));
            }
            let _ = home_slug;
            crate::central_auth::home_principal(&self.state, session_token)
                .await
                .ok_or_else(|| StorageError::Unauthorized("invalid session token".into()))?;
            Ok(())
        })
    }
}

/// This org's view of the deployment's Storage Locations, as the Files
/// backend's confinement boundary (issue #262).
///
/// A File Root may live under `<org>/files` — always — or under any
/// location the org holds a live-tree grant on. Without the second half
/// media on a NAS is unregisterable: the boundary check refuses a path
/// outside the org directory, which is on the server's own disk and was
/// never going to hold a 236 GiB video project.
///
/// Deliberately holds the registry rather than a resolved list. Grants
/// are issued at runtime, and a boundary snapshotted at boot would mean
/// a new Storage Location only takes effect after a restart — the kind
/// of staleness that gets diagnosed as "the mount is broken".
pub struct GrantedBoundaries {
    core: Arc<StorageCore>,
    org: String,
}

impl GrantedBoundaries {
    #[must_use]
    pub fn new(core: Arc<StorageCore>, org: String) -> Self {
        Self { core, org }
    }
}

impl files::LocationBoundaries for GrantedBoundaries {
    fn permitted(&self) -> Vec<PathBuf> {
        self.core.live_tree_boundaries(&self.org)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Review finding 9: a persisted identity that cannot be read must
    /// stop the server, not silently mint a new one.
    ///
    /// The silent path forks the volume: the server re-announces as a
    /// brand-new pending agent, and approving it registers a SECOND
    /// location for the same directory while every existing grant and
    /// placement still names the first — whose agent will never speak
    /// again.
    #[test]
    fn a_corrupt_agent_identity_refuses_to_start_rather_than_forking_the_volume() {
        let dir = tempfile::tempdir().expect("data root");
        let root = dir.path();

        // First boot enrolls and persists id + secret.
        let core = open(root).expect("first boot");
        let first = core.list_agents();
        assert_eq!(first.len(), 1, "the in-server agent enrolled");
        drop(core);

        // Second boot re-announces as the SAME agent.
        let core = open(root).expect("second boot");
        assert_eq!(
            core.list_agents().len(),
            1,
            "a restart re-announces rather than enrolling a second agent"
        );
        assert_eq!(core.list_agents()[0].id, first[0].id, "same identity");
        drop(core);

        // Now truncate the identity file, as a crash mid-write would.
        std::fs::write(identity_path(root), b"{\"id\": \"tru").expect("truncate");
        let err = open(root).expect_err("a corrupt identity must not boot");
        let message = format!("{err:#}");
        assert!(
            message.contains("unreadable"),
            "the error should name the problem: {message}"
        );

        // And nothing was invented in the registry while failing.
        let core = StorageCore::open(registry_dir(root)).expect("registry still opens");
        assert_eq!(
            core.list_agents().len(),
            1,
            "a refused boot must not have enrolled a second agent"
        );
    }

    /// A deployment must be able to use its own disk without a human
    /// approving the server to itself: enrollment lands `Pending`, and a
    /// Storage Location can only be registered on an approved agent, so
    /// an unapproved in-server agent means no location, no grant, no
    /// File Root — a self-hoster's first boot dead-ends.
    #[test]
    fn the_in_server_agent_is_usable_on_first_boot() {
        let dir = tempfile::tempdir().expect("data root");
        let core = open(dir.path()).expect("first boot");

        let agent = core.list_agents().pop().expect("the in-server agent");
        assert_eq!(agent.status, AgentStatus::Approved);

        // The property that actually matters: the announced volume is
        // already a Storage Location, with no operator step in between.
        // (Approval mints one per announced volume — so the whole
        // announce/approve/register ceremony collapses to nothing for
        // the server's own disks, and grants are the only operator
        // action left.)
        let locations = core.list_locations();
        assert!(
            locations.iter().any(|l| l.volume_key == "primary"),
            "the server's own volume must be a location on first boot, got {locations:?}"
        );
    }

    /// ...but self-approval must not override an operator who
    /// deliberately rejected this agent (decommissioning the server's
    /// local disk). A restart is not a way to undo that.
    #[test]
    fn a_rejected_in_server_agent_stays_rejected_across_a_restart() {
        let dir = tempfile::tempdir().expect("data root");
        let root = dir.path();
        let core = open(root).expect("first boot");
        let id = core.list_agents()[0].id;
        core.approve_agent(id, false).expect("operator rejects it");
        drop(core);

        let core = open(root).expect("second boot");
        assert_eq!(
            core.list_agents()[0].status,
            AgentStatus::Rejected,
            "a restart must not resurrect a rejected agent"
        );
    }

    #[test]
    fn a_grant_entry_parses_with_and_without_a_quota() {
        assert_eq!(
            parse_grant_entry("cbu@media:cbu"),
            Some(GrantEntry {
                org: "cbu".into(),
                volume_key: "media".into(),
                path_prefix: "cbu".into(),
                quota_bytes: u64::MAX,
            })
        );
        assert_eq!(
            parse_grant_entry(" tombrooksmusic @ media : clients/tbm : 8T ")
                .expect("whitespace is not an error")
                .quota_bytes,
            8 << 40
        );
        assert_eq!(
            parse_grant_entry("a@b:c:500G").unwrap().quota_bytes,
            500 << 30
        );
        assert_eq!(parse_grant_entry("a@b:c:1024").unwrap().quota_bytes, 1024);
    }

    /// A quota of literally zero admits nothing (`used >= quota` refuses
    /// at once), so an unparsable one must drop the entry rather than
    /// default — a grant that silently permits no bytes is far worse to
    /// diagnose than a missing one.
    #[test]
    fn a_malformed_grant_entry_is_skipped_never_defaulted_to_zero() {
        for bad in [
            "no-at-sign",
            "org@volume",             // no prefix
            "org@volume:prefix:huge", // unparsable quota
            // Overflows u64: anything past ~16.7 million TiB.
            "org@volume:prefix:99999999T",
            "org@volume:prefix:8T:extra",
            "@volume:prefix",
            "org@:prefix",
            "org@volume:",
        ] {
            assert_eq!(parse_grant_entry(bad), None, "{bad:?} must be skipped");
        }
    }

    /// The whole point: after boot, an org can actually reach the
    /// volume. Without a grant a location does not exist as far as that
    /// org is concerned, so announcing the mount alone changes nothing.
    #[test]
    fn configured_grants_admit_the_org_at_boot() {
        let dir = tempfile::tempdir().expect("data root");
        let core = open(dir.path()).expect("first boot");
        let agent_id = core.list_agents()[0].id;

        // `primary` is always announced, so this needs no extra mount.
        let entry = GrantEntry {
            org: "cbu".into(),
            volume_key: "primary".into(),
            path_prefix: "cbu".into(),
            quota_bytes: 1 << 40,
        };
        let location = core
            .list_locations()
            .into_iter()
            .find(|l| l.volume_key == "primary")
            .expect("primary is a location");
        core.issue_grant(files_storage::GrantSpec {
            org: entry.org.clone(),
            location_id: location.id,
            capabilities: location.capabilities.clone(),
            quota_bytes: entry.quota_bytes,
            path_prefix: entry.path_prefix.clone(),
        })
        .expect("grant issues");

        let grants = core.list_grants(None);
        assert_eq!(grants.len(), 1);
        assert_eq!(grants[0].org, "cbu");
        assert_eq!(grants[0].path_prefix, "cbu");

        // Idempotent: reconciling the same terms replaces rather than
        // accumulates, so a restart does not multiply grants.
        reconcile_grants(&core, agent_id);
        assert_eq!(
            core.list_grants(None).len(),
            1,
            "reconciling must not duplicate a grant"
        );
    }

    /// A missing identity file is the ordinary first-boot case, not an
    /// error — the distinction the silent-mint path erased.
    #[test]
    fn a_missing_agent_identity_is_first_boot() {
        let dir = tempfile::tempdir().expect("data root");
        assert!(load_identity(dir.path()).expect("absent is fine").is_none());
        open(dir.path()).expect("first boot enrolls");
        assert!(load_identity(dir.path()).expect("readable").is_some());
    }
}
