//! The coordinator: one deployment-scoped object holding the registry,
//! the org-lane event hubs, the agent-directive hub, and every rule that
//! governs placement. The three RPC lanes ([`crate::admin`],
//! [`crate::org`], [`crate::agent_lane`]) are thin shells over this.
//!
//! The rules, all of them enforced here rather than at any lane's edge:
//!
//! - An org reaches a location **only** through a grant. No grant is
//!   indistinguishable from no location — an org lane never learns that a
//!   location it wasn't admitted to exists.
//! - A grant's **capability subset** gates each axis: `LiveTrees` to host
//!   a live tree, `Blobs` to hold replicas. A blob-only location can
//!   never hold a live tree, whoever asks.
//! - A grant's **path prefix** is the org's subtree. The boundary travels
//!   with the directive and the *agent* enforces it before creating
//!   anything (see [`crate::agent`]); the coordinator re-checks the path
//!   the agent reports back as a post-condition.
//! - A grant's **logical-byte quota** bounds growth: usage is re-measured
//!   from the authoritative repos before every placement that could add
//!   bytes, and the projected addition must fit — not merely "the quota
//!   isn't exhausted yet".
//! - An agent's volumes become locations only once the operator
//!   **approves** it, and an agent proves who it is with a secret, never
//!   with its (public) id. The coordinator never becomes the data path:
//!   it issues directives, and agents move bytes.
//! - Every placement lookup is **org-scoped on both sides**: a root id
//!   from one org can never resolve to another org's placement.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::Utc;
use files_storage_proto::{
    AgentAnnouncement, AgentCredential, AgentDirective, AgentEnrollment, AgentHosting, AgentInfo,
    AgentStatus, AnnouncedVolume, BlobReplica, CapabilityClass, ConfinedPath, DirectiveKind,
    DirectiveOutcome, GrantSpec, GrantUsage, LiveTreeBinding, LocationHealth, PlacementStatus,
    RootPlacement, StorageError, StorageEvent, StorageGrantInfo, StorageLocationInfo, VolumeHealth,
};
use uuid::Uuid;

use crate::agent::LocalAgent;
use crate::error::{Result, path as path_err};
use crate::state::{Outstanding, Registry, State};

/// Sub-directory of a blob-capable location's granted prefix that holds
/// replicas, one chunk store per root. Kept out of the way of any live
/// tree the same grant may also host.
const REPLICA_DIR: &str = "blobs";

pub struct StorageCore {
    registry: Registry,
    /// Agents living in this process (the in-server hosting, and in tests
    /// a fake). A directive for one of these is executed inline; anything
    /// else waits on the wire protocol.
    local_agents: Mutex<HashMap<Uuid, Arc<dyn LocalAgent>>>,
    /// One hub per org — an org's subscribers must never see another
    /// org's grants or placements, and `#[subscribe]` hands out a
    /// `&PubSub`, so the per-org backend holds a clone of its own hub.
    org_hubs: Mutex<HashMap<String, architect::PubSub<StorageEvent>>>,
    /// The agent lane's single directive hub. Directives carry their
    /// `agent_id` and agents filter client-side (root CLAUDE.md's
    /// `#[subscribe]` idiom).
    directives: architect::PubSub<AgentDirective>,
}

impl std::fmt::Debug for StorageCore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StorageCore").finish_non_exhaustive()
    }
}

impl StorageCore {
    /// Open (or create) the deployment's registry under `dir`.
    pub fn open(dir: impl AsRef<Path>) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            registry: Registry::open(dir.as_ref())?,
            local_agents: Mutex::new(HashMap::new()),
            org_hubs: Mutex::new(HashMap::new()),
            directives: architect::PubSub::sliding(256),
        }))
    }

    /// Attach an in-process agent. Its directives are executed inline, so
    /// a placement onto one of its volumes is `Hosted` by the time
    /// `place_root` returns.
    pub fn register_local_agent(&self, agent: Arc<dyn LocalAgent>) {
        self.local_agents
            .lock()
            .expect("local agent map poisoned")
            .insert(agent.id(), agent);
    }

    fn local_agent(&self, id: Uuid) -> Option<Arc<dyn LocalAgent>> {
        self.local_agents
            .lock()
            .expect("local agent map poisoned")
            .get(&id)
            .cloned()
    }

    pub fn directives_hub(&self) -> &architect::PubSub<AgentDirective> {
        &self.directives
    }

    /// This org's event hub, created on first use. Cloned hubs share one
    /// subscriber list.
    pub fn org_hub(&self, org: &str) -> architect::PubSub<StorageEvent> {
        self.org_hubs
            .lock()
            .expect("org hub map poisoned")
            .entry(org.to_string())
            .or_insert_with(|| architect::PubSub::sliding(256))
            .clone()
    }

    fn publish(&self, org: &str, event: StorageEvent) {
        self.org_hub(org).publish(event);
    }

    // ── Agent identity ──────────────────────────────────────────────

    /// Mint an enrollment secret. Two v4 UUIDs' worth of CSPRNG output,
    /// which is what `uuid` gives us without adding a dependency.
    fn mint_token() -> String {
        format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
    }

    fn hash_token(token: &str) -> String {
        blake3::hash(token.as_bytes()).to_hex().to_string()
    }

    /// Confirm a credential really belongs to the agent it names. Agent
    /// ids are public (`list_agents`, every `StorageLocationInfo`, every
    /// directive), so this — not the id — is the identity check.
    fn authorize_agent(&self, credential: &AgentCredential) -> Result<()> {
        let expected = self
            .registry
            .read(|s| s.agent_secrets.get(&credential.agent_id).cloned());
        let Some(expected) = expected else {
            return Err(StorageError::Unauthorized(format!(
                "agent {} is not enrolled",
                credential.agent_id
            )));
        };
        if Self::hash_token(&credential.token) != expected {
            return Err(StorageError::Unauthorized(format!(
                "credential does not match agent {}",
                credential.agent_id
            )));
        }
        Ok(())
    }

    // ── Agent lane ──────────────────────────────────────────────────

    /// Announce (or re-announce) an agent. A new id enrolls and is handed
    /// its secret; a known id must present that secret. Re-announcing
    /// never resets an approval — which is precisely why it cannot be
    /// done with a public id alone: the volume list (and each volume's
    /// `root_path`) is what gets replaced.
    pub fn announce(&self, announcement: AgentAnnouncement) -> Result<AgentEnrollment> {
        let AgentAnnouncement {
            agent_id,
            token,
            hosting,
            label,
            volumes,
        } = announcement;
        for volume in &volumes {
            if volume.key.trim().is_empty() {
                return Err(StorageError::BadRequest(
                    "announced volume has no key".into(),
                ));
            }
            if volume.capabilities.is_empty() {
                return Err(StorageError::BadRequest(format!(
                    "volume {} announces no capability classes",
                    volume.key
                )));
            }
        }

        let known = self.registry.read(|s| s.agent(agent_id).is_some());
        if known {
            let Some(token) = token else {
                return Err(StorageError::Unauthorized(format!(
                    "agent {agent_id} is already enrolled; re-announcing requires its token"
                )));
            };
            self.authorize_agent(&AgentCredential { agent_id, token })?;
            let agent = self.registry.write(|state| {
                let agent = state
                    .agent_mut(agent_id)
                    .ok_or_else(|| StorageError::NotFound(format!("agent {agent_id}")))?;
                agent.hosting = hosting;
                agent.label = label;
                agent.volumes = volumes;
                agent.last_seen = Utc::now();
                Ok(agent.clone())
            })?;
            return Ok(AgentEnrollment { agent, token: None });
        }

        let secret = Self::mint_token();
        let hashed = Self::hash_token(&secret);
        let agent = self.registry.write(|state| {
            let info = AgentInfo {
                id: agent_id,
                hosting,
                label,
                status: AgentStatus::Pending,
                volumes,
                last_seen: Utc::now(),
            };
            state.agents.push(info.clone());
            state.agent_secrets.insert(agent_id, hashed);
            Ok(info)
        })?;
        Ok(AgentEnrollment {
            agent,
            token: Some(secret),
        })
    }

    /// Heartbeat: liveness plus per-volume health, propagated onto the
    /// volumes' registered locations.
    pub fn heartbeat(
        &self,
        credential: &AgentCredential,
        volumes: Vec<VolumeHealth>,
    ) -> Result<AgentInfo> {
        self.authorize_agent(credential)?;
        let agent_id = credential.agent_id;

        // Does anything durable actually change? `last_seen` alone does
        // not — persisting it would rewrite the whole document per beat,
        // per agent, forever (PR #284 review).
        let health_changes = self.registry.read(|state| {
            volumes
                .iter()
                .filter(|report| {
                    state
                        .location_for_volume(agent_id, &report.volume_key)
                        .is_some_and(|l| l.health != report.health)
                })
                .count()
        });

        let apply = |state: &mut State| -> Result<(AgentInfo, Vec<StorageLocationInfo>)> {
            let agent = state
                .agent_mut(agent_id)
                .ok_or_else(|| StorageError::NotFound(format!("agent {agent_id}")))?;
            agent.last_seen = Utc::now();
            let info = agent.clone();
            let mut changed = Vec::new();
            for report in &volumes {
                if let Some(location) = state
                    .locations
                    .iter_mut()
                    .find(|l| l.agent_id == agent_id && l.volume_key == report.volume_key)
                    && location.health != report.health
                {
                    location.health = report.health;
                    changed.push(location.clone());
                }
            }
            Ok((info, changed))
        };

        let (info, changed) = if health_changes == 0 {
            self.registry.write_volatile(apply)?
        } else {
            self.registry.write(apply)?
        };
        self.announce_locations(&changed);
        Ok(info)
    }

    pub fn list_agents(&self) -> Vec<AgentInfo> {
        self.registry.read(|s| s.agents.clone())
    }

    pub fn pending_directives(&self, credential: &AgentCredential) -> Result<Vec<AgentDirective>> {
        self.authorize_agent(credential)?;
        Ok(self.registry.read(|s| {
            s.outstanding
                .iter()
                .filter(|o| o.directive.agent_id == credential.agent_id)
                .map(|o| o.directive.clone())
                .collect()
        }))
    }

    // ── Operator lane ───────────────────────────────────────────────

    /// Approve (or reject) an agent. Approval is what turns announced
    /// volumes into registered Storage Locations — the step that keeps a
    /// rogue agent out of the data path — and it also brings back
    /// locations a previous rejection took offline, since nothing else
    /// would (the in-server agent has no heartbeat loop; PR #284 review).
    pub fn approve_agent(&self, agent_id: Uuid, approved: bool) -> Result<AgentInfo> {
        let (info, changed) = self.registry.write(|state| {
            let agent = state
                .agent_mut(agent_id)
                .ok_or_else(|| StorageError::NotFound(format!("agent {agent_id}")))?;
            agent.status = if approved {
                AgentStatus::Approved
            } else {
                AgentStatus::Rejected
            };
            let info = agent.clone();

            let mut changed = Vec::new();
            if approved {
                for volume in &info.volumes {
                    if state.location_for_volume(agent_id, &volume.key).is_none() {
                        let location = new_location(agent_id, volume);
                        state.locations.push(location.clone());
                        changed.push(location);
                    }
                }
                // Re-approval restores health. Without this an operator
                // who paused an agent by rejecting it could never
                // un-pause it: `require_online` would refuse every
                // placement forever, and health is persisted.
                for location in state
                    .locations
                    .iter_mut()
                    .filter(|l| l.agent_id == agent_id && l.health == LocationHealth::Offline)
                {
                    location.health = LocationHealth::Online;
                    changed.push(location.clone());
                }
            } else {
                // A revoked approval is not a delete: placements and
                // their data stay, the locations simply go offline.
                for location in state
                    .locations
                    .iter_mut()
                    .filter(|l| l.agent_id == agent_id)
                {
                    location.health = LocationHealth::Offline;
                    changed.push(location.clone());
                }
            }
            Ok((info, changed))
        })?;
        self.announce_locations(&changed);
        Ok(info)
    }

    // t[impl files.scale.capacity] — "capacity grows by registering a
    // storage location: no downtime, no migration, no path changes".
    // Admitting a volume adds room and nothing else; a path still
    // resolves to content and content to a location, so no path here
    // changes and nothing is moved
    /// Admit one announced volume of an already-approved agent.
    pub fn register_location(
        &self,
        agent_id: Uuid,
        volume_key: &str,
    ) -> Result<StorageLocationInfo> {
        self.registry.write(|state| {
            let agent = state
                .agent(agent_id)
                .ok_or_else(|| StorageError::NotFound(format!("agent {agent_id}")))?;
            if agent.status != AgentStatus::Approved {
                return Err(StorageError::AgentNotApproved(format!(
                    "agent {agent_id} is {:?}",
                    agent.status
                )));
            }
            let Some(volume) = agent.volumes.iter().find(|v| v.key == volume_key).cloned() else {
                return Err(StorageError::NotFound(format!(
                    "agent {agent_id} announced no volume {volume_key}"
                )));
            };
            if let Some(existing) = state.location_for_volume(agent_id, volume_key) {
                return Err(StorageError::AlreadyExists(format!(
                    "volume {volume_key} is already location {}",
                    existing.id
                )));
            }
            let location = new_location(agent_id, &volume);
            state.locations.push(location.clone());
            Ok(location)
        })
    }

    pub fn list_locations(&self) -> Vec<StorageLocationInfo> {
        self.registry.read(|s| s.locations.clone())
    }

    /// Admit an org onto a location. Re-issuing for the same (org,
    /// location) replaces the terms and keeps the grant's id.
    pub fn issue_grant(&self, spec: GrantSpec) -> Result<StorageGrantInfo> {
        if spec.org.trim().is_empty() {
            return Err(StorageError::BadRequest("grant has no org".into()));
        }
        if spec.capabilities.is_empty() {
            return Err(StorageError::BadRequest(
                "a grant with no capability class admits nothing".into(),
            ));
        }
        // The prefix is a path the org's whole subtree hangs off; it must
        // itself be a safe relative path, or "the org's own subtree"
        // means nothing.
        files_store::safe_relative(&spec.path_prefix).map_err(path_err)?;

        let grant = self.registry.write(|state| {
            let location = state.location(spec.location_id).cloned().ok_or_else(|| {
                StorageError::NotFound(format!("storage location {}", spec.location_id))
            })?;
            if let Some(extra) = spec
                .capabilities
                .iter()
                .find(|c| !location.capabilities.contains(c))
            {
                return Err(StorageError::CapabilityDenied(format!(
                    "location {} does not offer {extra:?}",
                    location.id
                )));
            }
            let now = Utc::now();
            if let Some(existing) = state
                .grants
                .iter_mut()
                .find(|g| g.org == spec.org && g.location_id == spec.location_id)
            {
                existing.capabilities = spec.capabilities;
                existing.quota_bytes = spec.quota_bytes;
                existing.path_prefix = spec.path_prefix;
                existing.granted_at = now;
                return Ok(existing.clone());
            }
            let grant = StorageGrantInfo {
                id: Uuid::new_v4(),
                org: spec.org,
                location_id: spec.location_id,
                capabilities: spec.capabilities,
                quota_bytes: spec.quota_bytes,
                used_bytes: 0,
                path_prefix: spec.path_prefix,
                granted_at: now,
            };
            state.grants.push(grant.clone());
            Ok(grant)
        })?;
        let grant = self.with_usage(grant);
        self.publish(&grant.org, StorageEvent::GrantIssued(grant.clone()));
        Ok(grant)
    }

    /// Withdraw an org's admission. Data already placed is left exactly
    /// where it is — revoking is an admission change, not a delete.
    pub fn revoke_grant(&self, grant_id: Uuid) -> Result<()> {
        let org = self.registry.write(|state| {
            let index = state
                .grants
                .iter()
                .position(|g| g.id == grant_id)
                .ok_or_else(|| StorageError::NotFound(format!("storage grant {grant_id}")))?;
            Ok(state.grants.remove(index).org)
        })?;
        self.publish(&org, StorageEvent::GrantRevoked(grant_id));
        Ok(())
    }

    pub fn list_grants(&self, org: Option<&str>) -> Vec<StorageGrantInfo> {
        self.registry
            .read(|s| {
                s.grants
                    .iter()
                    .filter(|g| org.is_none_or(|o| g.org == o))
                    .cloned()
                    .collect::<Vec<_>>()
            })
            .into_iter()
            .map(|g| self.with_usage(g))
            .collect()
    }

    // ── Org lane ────────────────────────────────────────────────────

    /// Directories this org may register a File Root under, outside its
    /// own org directory: `<location.root_path>/<grant.path_prefix>` for
    /// every grant carrying [`CapabilityClass::LiveTrees`].
    ///
    /// This is the boundary the Files backend confines root creation to —
    /// the same computation [`Self::place_root`] performs before binding a
    /// live tree, exposed so the check can happen at `create_root` time
    /// rather than only at placement. Keeping ONE definition of "where may
    /// this org put a tree" matters: a boundary that disagreed with
    /// placement would either register roots that can never be placed, or
    /// refuse ones that could.
    ///
    /// Health is deliberately NOT consulted. An offline location is a
    /// reason to refuse a WRITE (`place_root` calls `require_online`), not
    /// a reason to forget that the org is allowed to keep trees there — a
    /// NAS that is briefly unreachable must not make every root under it
    /// look like an escape attempt.
    ///
    /// Empty when this deployment has no locations or the org holds no
    /// grants, which is the single-machine default: only the org's own
    /// directory is permitted, exactly as before locations existed.
    pub fn live_tree_boundaries(&self, org: &str) -> Vec<PathBuf> {
        self.registry.read(|state| {
            state
                .locations
                .iter()
                .filter_map(|location| {
                    let grant = state.grant(org, location.id)?;
                    if !grant.capabilities.contains(&CapabilityClass::LiveTrees) {
                        return None;
                    }
                    // A malformed prefix (absolute, or containing `..`)
                    // yields no boundary rather than a boundary that
                    // escapes the location — the same rule `place_root`
                    // enforces via `safe_relative`.
                    let prefix = files_store::safe_relative(&grant.path_prefix).ok()?;
                    Some(Path::new(&location.root_path).join(prefix))
                })
                .collect()
        })
    }

    /// Locations this org holds a grant on — the only ones it can see.
    pub fn locations_for(&self, org: &str) -> Vec<StorageLocationInfo> {
        self.registry.read(|state| {
            state
                .locations
                .iter()
                .filter(|l| state.grant(org, l.id).is_some())
                .cloned()
                .collect()
        })
    }

    pub fn placement(&self, org: &str, root_id: Uuid) -> Result<RootPlacement> {
        self.registry
            .read(|s| s.placement(org, root_id).cloned())
            .ok_or_else(|| StorageError::NotFound(format!("placement for root {root_id}")))
    }

    pub fn list_placements(&self, org: &str) -> Vec<RootPlacement> {
        self.registry.read(|s| {
            s.placements
                .iter()
                .filter(|p| p.org == org)
                .cloned()
                .collect()
        })
    }

    pub fn usage(&self, org: &str, location_id: Uuid) -> Result<GrantUsage> {
        let grant = self.require_grant(org, location_id)?;
        self.remeasure(org, location_id);
        let (used_bytes, placements) = self.registry.read(|s| usage_of(s, org, location_id));
        Ok(GrantUsage {
            location_id,
            quota_bytes: grant.quota_bytes,
            used_bytes,
            placements,
        })
    }

    /// Bind a root's live tree to a location and have that location's
    /// agent host it.
    ///
    /// Returns `Err` unless the tree is actually hosted by the time this
    /// returns — a `Failed` placement is a failure, not an `Ok` with a
    /// status field a caller might not read (PR #284 review). A remote
    /// agent's placement legitimately returns `Pending`; only that one
    /// case resolves `Ok` without a live tree on disk.
    pub fn place_root(
        &self,
        org: &str,
        root_id: Uuid,
        location_id: Uuid,
        relative_path: &str,
    ) -> Result<RootPlacement> {
        let grant = self.require_grant(org, location_id)?;
        self.require_capability(&grant, CapabilityClass::LiveTrees, "host a live tree")?;
        let location = self.require_online(location_id)?;
        // A new live tree adds no bytes yet, but it must not be opened on
        // a grant that is already full.
        self.require_headroom(org, &grant, 0)?;

        let relative = files_store::safe_relative(relative_path).map_err(path_err)?;
        let prefix = files_store::safe_relative(&grant.path_prefix).map_err(path_err)?;
        let boundary = Path::new(&location.root_path).join(prefix);
        let target = ConfinedPath {
            boundary: files_store::to_utf8(&boundary).map_err(path_err)?,
            relative: files_store::to_utf8(&relative).map_err(path_err)?,
        };
        // What the agent is expected to resolve. The agent's own answer
        // is authoritative (it holds the filesystem); this is only used
        // to spot two roots aiming at one tree before either is created.
        let expected = files_store::to_utf8(&boundary.join(&relative)).map_err(path_err)?;

        let directive = AgentDirective {
            id: Uuid::new_v4(),
            agent_id: location.agent_id,
            kind: DirectiveKind::HostLiveTree {
                root_id,
                org: org.to_string(),
                target: target.clone(),
            },
        };

        self.registry.write(|state| {
            if let Some(existing) = state.placement(org, root_id)
                && existing.live_tree.is_some()
            {
                return Err(StorageError::AlreadyExists(format!(
                    "root {root_id} already has a live tree"
                )));
            }
            // A root's live tree sits wholly on one location, and two
            // roots never share a tree (glossary "File Root": roots never
            // overlap on disk).
            if let Some(clash) = state.placements.iter().find(|p| {
                p.root_id != root_id
                    && p.live_tree
                        .as_ref()
                        .is_some_and(|lt| lt.absolute_path == expected)
            }) {
                return Err(StorageError::AlreadyExists(format!(
                    "{expected} is already root {}'s live tree",
                    clash.root_id
                )));
            }
            let binding = LiveTreeBinding {
                location_id,
                relative_path: target.relative.clone(),
                absolute_path: expected.clone(),
                repo_initialized: false,
            };
            match state.placement_mut(org, root_id) {
                Some(existing) => {
                    existing.live_tree = Some(binding);
                    existing.status = PlacementStatus::Pending;
                    existing.failure = None;
                }
                None => state.placements.push(RootPlacement {
                    root_id,
                    org: org.to_string(),
                    status: PlacementStatus::Pending,
                    live_tree: Some(binding),
                    logical_bytes: 0,
                    replicas: Vec::new(),
                    failure: None,
                }),
            }
            Ok(())
        })?;

        let executed_locally = self.dispatch(directive, location_id)?;
        let placement = self.placement(org, root_id)?;
        if executed_locally {
            if placement.status != PlacementStatus::Hosted {
                return Err(StorageError::Io(format!(
                    "hosting root {root_id} failed: {}",
                    placement.failure.as_deref().unwrap_or("unknown reason")
                )));
            }
            // Charge what is actually there. Without this the quota gates
            // a number that is 0 forever (PR #284 review).
            self.measure_now(org, root_id)?;
        }
        self.finish(org, root_id)
    }

    /// Replicate a root's version-store blobs onto a second location —
    /// the axis independent of the live tree.
    pub fn add_blob_replica(
        &self,
        org: &str,
        root_id: Uuid,
        location_id: Uuid,
    ) -> Result<RootPlacement> {
        let grant = self.require_grant(org, location_id)?;
        self.require_capability(&grant, CapabilityClass::Blobs, "hold blob replicas")?;
        let location = self.require_online(location_id)?;

        // Measure the source first: the projection below is only
        // meaningful against a current number.
        self.measure_now(org, root_id)?;
        let placement = self.placement(org, root_id)?;
        let Some(live_tree) = placement.live_tree.clone() else {
            return Err(StorageError::BadRequest(format!(
                "root {root_id} has no live tree to replicate from"
            )));
        };
        if live_tree.location_id == location_id {
            return Err(StorageError::BadRequest(
                "a root's blob replica must live on a different location than its live tree".into(),
            ));
        }
        // The replica costs the destination grant the root's logical
        // bytes; refuse before moving any of them.
        self.require_headroom(org, &grant, placement.logical_bytes)?;

        let prefix = files_store::safe_relative(&grant.path_prefix).map_err(path_err)?;
        let boundary = Path::new(&location.root_path).join(prefix);
        let dest = ConfinedPath {
            boundary: files_store::to_utf8(&boundary).map_err(path_err)?,
            relative: files_store::to_utf8(&PathBuf::from(REPLICA_DIR).join(root_id.to_string()))
                .map_err(path_err)?,
        };
        let expected = files_store::to_utf8(&boundary.join(&dest.relative)).map_err(path_err)?;

        let directive = AgentDirective {
            id: Uuid::new_v4(),
            agent_id: location.agent_id,
            kind: DirectiveKind::ReplicateBlobs {
                root_id,
                org: org.to_string(),
                source_path: live_tree.absolute_path.clone(),
                dest,
            },
        };

        self.registry.write(|state| {
            let placement = state
                .placement_mut(org, root_id)
                .ok_or_else(|| StorageError::NotFound(format!("placement for root {root_id}")))?;
            if !placement
                .replicas
                .iter()
                .any(|r| r.location_id == location_id)
            {
                placement.replicas.push(BlobReplica {
                    location_id,
                    absolute_path: expected.clone(),
                    files_present: 0,
                    logical_bytes: 0,
                    synced_at: None,
                });
            }
            Ok(())
        })?;

        self.dispatch(directive, location_id)?;
        self.finish(org, root_id)
    }

    /// Re-measure a root's logical bytes from its authoritative repo.
    pub fn refresh_usage(&self, org: &str, root_id: Uuid) -> Result<RootPlacement> {
        self.measure_now(org, root_id)?;
        self.finish(org, root_id)
    }

    /// Issue the measure directive for one root. Local agents answer
    /// inline; a remote one answers when it reports back.
    fn measure_now(&self, org: &str, root_id: Uuid) -> Result<()> {
        let placement = self.placement(org, root_id)?;
        let Some(live_tree) = placement.live_tree.clone() else {
            return Err(StorageError::BadRequest(format!(
                "root {root_id} has no live tree to measure"
            )));
        };
        let location = self.registry.require_location(live_tree.location_id)?;
        let directive = AgentDirective {
            id: Uuid::new_v4(),
            agent_id: location.agent_id,
            kind: DirectiveKind::MeasureLiveTree {
                root_id,
                org: org.to_string(),
                live_tree_path: live_tree.absolute_path.clone(),
            },
        };
        self.dispatch(directive, live_tree.location_id)?;
        Ok(())
    }

    /// Bring every locally-measurable placement of `org` on `location`
    /// up to date, so the quota gates a current number rather than
    /// whatever was last recorded. Silent about failures: a measurement
    /// that cannot run must not turn a placement call into an error, it
    /// just leaves that root's last known size in place.
    fn remeasure(&self, org: &str, location_id: Uuid) {
        let roots: Vec<Uuid> = self.registry.read(|state| {
            state
                .placements
                .iter()
                .filter(|p| {
                    p.org == org
                        && p.live_tree
                            .as_ref()
                            .is_some_and(|lt| lt.location_id == location_id)
                })
                .map(|p| p.root_id)
                .collect()
        });
        for root_id in roots {
            let _ = self.measure_now(org, root_id);
        }
    }

    // ── Directive plumbing ──────────────────────────────────────────

    /// Publish a directive to the agent lane and either run it inline
    /// (its agent lives in this process) or queue it as outstanding (it
    /// does not). The publish happens either way: it is how a remote
    /// agent — and any observer — learns of the work. Returns whether it
    /// ran inline.
    ///
    /// Only *remote* work becomes outstanding. Inline execution is
    /// synchronous, so queueing it would be a round trip through a
    /// deduplicating list purely to take it straight back out — and with
    /// several concurrent directives for one root, the dedupe would
    /// evict an entry whose own execution was still in flight.
    fn dispatch(&self, directive: AgentDirective, location_id: Uuid) -> Result<bool> {
        self.directives.publish(directive.clone());
        if let Some(agent) = self.local_agent(directive.agent_id) {
            let outcome = agent.execute(&directive);
            self.apply(&directive, location_id, outcome)?;
            return Ok(true);
        }
        self.registry.write(|state| {
            state.enqueue(Outstanding {
                directive,
                location_id,
                issued_at: Utc::now(),
            });
            Ok(())
        })?;
        Ok(false)
    }

    /// The agent lane's entry point: a remote agent reporting work it was
    /// handed. The credential is checked first, then the directive is
    /// taken off the outstanding list — which is also what proves it was
    /// issued to this agent at all.
    pub fn complete_directive(
        &self,
        credential: &AgentCredential,
        directive_id: Uuid,
        outcome: DirectiveOutcome,
    ) -> Result<()> {
        self.authorize_agent(credential)?;
        let outstanding = self.registry.write(|state| {
            let index = state
                .outstanding
                .iter()
                .position(|o| o.directive.id == directive_id)
                .ok_or_else(|| StorageError::NotFound(format!("directive {directive_id}")))?;
            if state.outstanding[index].directive.agent_id != credential.agent_id {
                return Err(StorageError::Unauthorized(format!(
                    "directive {directive_id} belongs to another agent"
                )));
            }
            Ok(state.outstanding.remove(index))
        })?;
        self.apply(&outstanding.directive, outstanding.location_id, outcome)
    }

    /// Apply a finished directive's outcome — the one place a placement
    /// moves forward, whether the agent was local or remote.
    fn apply(
        &self,
        directive: &AgentDirective,
        location_id: Uuid,
        outcome: DirectiveOutcome,
    ) -> Result<()> {
        let (org, placement) = self.registry.write(|state| {
            let (root_id, org) = match &directive.kind {
                DirectiveKind::HostLiveTree { root_id, org, .. }
                | DirectiveKind::ReplicateBlobs { root_id, org, .. }
                | DirectiveKind::MeasureLiveTree { root_id, org, .. } => (*root_id, org.clone()),
            };
            let boundary = match &directive.kind {
                DirectiveKind::HostLiveTree { target, .. } => Some(target.boundary.clone()),
                DirectiveKind::ReplicateBlobs { dest, .. } => Some(dest.boundary.clone()),
                DirectiveKind::MeasureLiveTree { .. } => None,
            };
            // Org-scoped: the org recorded on the directive, never a
            // bare root id, so one org's outcome can never land on
            // another's placement.
            let placement = state
                .placement_mut(&org, root_id)
                .ok_or_else(|| StorageError::NotFound(format!("placement for root {root_id}")))?;

            match outcome {
                DirectiveOutcome::Hosted {
                    repo_initialized,
                    absolute_path,
                } => match verify_within(&absolute_path, boundary.as_deref()) {
                    Ok(()) => {
                        if let Some(live_tree) = placement.live_tree.as_mut() {
                            live_tree.repo_initialized = repo_initialized;
                            live_tree.absolute_path = absolute_path;
                        }
                        placement.status = PlacementStatus::Hosted;
                        placement.failure = None;
                    }
                    Err(reason) => fail(placement, reason),
                },
                DirectiveOutcome::Measured { logical_bytes, .. } => {
                    placement.logical_bytes = logical_bytes;
                }
                DirectiveOutcome::Replicated {
                    files_present,
                    logical_bytes,
                    absolute_path,
                } => match verify_within(&absolute_path, boundary.as_deref()) {
                    Ok(()) => {
                        if let Some(replica) = placement
                            .replicas
                            .iter_mut()
                            .find(|r| r.location_id == location_id)
                        {
                            replica.files_present = files_present;
                            replica.logical_bytes = logical_bytes;
                            replica.absolute_path = absolute_path;
                            replica.synced_at = Some(Utc::now());
                        }
                    }
                    Err(reason) => fail(placement, reason),
                },
                DirectiveOutcome::Failed { reason } => fail(placement, reason),
            }
            let placement = placement.clone();
            Ok((org, placement))
        })?;
        self.publish(&org, StorageEvent::PlacementChanged(placement));
        Ok(())
    }

    /// Read a placement back and announce it — every mutating org-lane
    /// call ends here so subscribers see exactly what the caller got.
    fn finish(&self, org: &str, root_id: Uuid) -> Result<RootPlacement> {
        let placement = self.placement(org, root_id)?;
        self.publish(org, StorageEvent::PlacementChanged(placement.clone()));
        Ok(placement)
    }

    fn announce_locations(&self, changed: &[StorageLocationInfo]) {
        for location in changed {
            for org in self.orgs_granted_on(location.id) {
                self.publish(&org, StorageEvent::LocationChanged(location.clone()));
            }
        }
    }

    // ── Rule helpers ────────────────────────────────────────────────

    /// The grant admitting `org` onto `location_id`. A location the org
    /// holds no grant on is reported as ungranted whether or not it
    /// exists — an org lane never learns the deployment's registry.
    fn require_grant(&self, org: &str, location_id: Uuid) -> Result<StorageGrantInfo> {
        self.registry
            .read(|s| s.grant(org, location_id).cloned())
            .map(|g| self.with_usage(g))
            .ok_or_else(|| {
                StorageError::NotGranted(format!(
                    "org {org} holds no grant on location {location_id}"
                ))
            })
    }

    fn require_capability(
        &self,
        grant: &StorageGrantInfo,
        class: CapabilityClass,
        verb: &str,
    ) -> Result<()> {
        if grant.capabilities.contains(&class) {
            return Ok(());
        }
        Err(StorageError::CapabilityDenied(format!(
            "grant {} does not carry {class:?}, so it may not {verb}",
            grant.id
        )))
    }

    fn require_online(&self, location_id: Uuid) -> Result<StorageLocationInfo> {
        let location = self.registry.require_location(location_id)?;
        if location.health == LocationHealth::Online {
            return Ok(location);
        }
        Err(StorageError::BadRequest(format!(
            "location {location_id} is {:?}; placement waits for it to come back",
            location.health
        )))
    }

    /// The quota check. Re-measures first, so what it gates is what is
    /// actually on the volume, and refuses when the *projected* total
    /// would exceed the quota rather than only once the quota is already
    /// spent — the latter admits growth instead of bounding it (PR #284
    /// review).
    fn require_headroom(
        &self,
        org: &str,
        grant: &StorageGrantInfo,
        projected_addition: u64,
    ) -> Result<()> {
        self.remeasure(org, grant.location_id);
        let (used, _) = self.registry.read(|s| usage_of(s, org, grant.location_id));
        let projected = used.saturating_add(projected_addition);
        if used >= grant.quota_bytes || projected > grant.quota_bytes {
            return Err(StorageError::QuotaExceeded(format!(
                "org {org} would hold {projected} of {} permitted logical bytes on location {} \
                 ({used} already used)",
                grant.quota_bytes, grant.location_id
            )));
        }
        Ok(())
    }

    /// Fill a grant's derived `used_bytes`. Usage is always computed from
    /// placements rather than stored, so a counter can never drift from
    /// what is actually placed.
    fn with_usage(&self, mut grant: StorageGrantInfo) -> StorageGrantInfo {
        let (used, _) = self
            .registry
            .read(|s| usage_of(s, &grant.org, grant.location_id));
        grant.used_bytes = used;
        grant
    }

    fn orgs_granted_on(&self, location_id: Uuid) -> Vec<String> {
        self.registry.read(|s| {
            s.grants
                .iter()
                .filter(|g| g.location_id == location_id)
                .map(|g| g.org.clone())
                .collect()
        })
    }
}

/// Record a failure AND release the live-tree binding, so the root can be
/// placed again. Leaving the binding in place wedged the root forever:
/// the retry guard keys on `live_tree.is_some()` and no lane has an
/// unplace verb (PR #284 review). The first failure reason wins — a
/// later post-condition must not overwrite the agent's real error.
fn fail(placement: &mut RootPlacement, reason: String) {
    placement.status = PlacementStatus::Failed;
    if placement.failure.is_none() {
        placement.failure = Some(reason);
    }
    placement.live_tree = None;
}

/// Post-condition on a path an agent reports: it must be inside the
/// boundary the directive named. The agent already enforced this before
/// creating anything; this catches an agent that lies (or drifts).
fn verify_within(reported: &str, boundary: Option<&str>) -> std::result::Result<(), String> {
    let Some(boundary) = boundary else {
        return Ok(());
    };
    let reported_path = Path::new(reported);
    if !reported_path.starts_with(Path::new(boundary)) {
        return Err(format!(
            "agent reported {reported}, which is outside the granted boundary {boundary}"
        ));
    }
    // When the path is visible from here (a local agent), resolve it too
    // — a textual prefix says nothing about symlinks.
    if reported_path.exists() {
        files_store::confine(reported_path, Path::new(boundary)).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Logical bytes an org has on one location, plus how many placements
/// (live trees + replicas) make them up.
fn usage_of(state: &State, org: &str, location_id: Uuid) -> (u64, u32) {
    let mut used = 0u64;
    let mut count = 0u32;
    for placement in state.placements.iter().filter(|p| p.org == org) {
        if placement
            .live_tree
            .as_ref()
            .is_some_and(|lt| lt.location_id == location_id)
        {
            used = used.saturating_add(placement.logical_bytes);
            count += 1;
        }
        for replica in placement
            .replicas
            .iter()
            .filter(|r| r.location_id == location_id)
        {
            used = used.saturating_add(replica.logical_bytes);
            count += 1;
        }
    }
    (used, count)
}

fn new_location(agent_id: Uuid, volume: &AnnouncedVolume) -> StorageLocationInfo {
    StorageLocationInfo {
        id: Uuid::new_v4(),
        name: volume.name.clone(),
        kind: volume.kind,
        agent_id,
        volume_key: volume.key.clone(),
        root_path: volume.root_path.clone(),
        capabilities: volume.capabilities.clone(),
        capacity_bytes: volume.capacity_bytes,
        health: LocationHealth::Online,
        registered_at: Utc::now(),
    }
}

/// The in-server agent's own announcement — the server speaking for its
/// own volumes. Approval still runs (the operator decides what the
/// deployment offers), but nothing about it is remote. `token` is the
/// enrollment secret from a previous run, or `None` on first enrollment.
#[must_use]
pub fn in_server_announcement(
    agent_id: Uuid,
    label: impl Into<String>,
    token: Option<String>,
    volumes: Vec<AnnouncedVolume>,
) -> AgentAnnouncement {
    AgentAnnouncement {
        agent_id,
        token,
        hosting: AgentHosting::InServer,
        label: label.into(),
        volumes,
    }
}

/// Convenience for the common in-server volume: a POSIX directory that
/// can do both capability classes.
#[must_use]
pub fn server_volume(
    key: impl Into<String>,
    name: impl Into<String>,
    root_path: &Path,
) -> AnnouncedVolume {
    AnnouncedVolume {
        key: key.into(),
        name: name.into(),
        kind: files_storage_proto::LocationKind::ServerVolume,
        root_path: root_path.to_string_lossy().into_owned(),
        capabilities: vec![CapabilityClass::LiveTrees, CapabilityClass::Blobs],
        capacity_bytes: None,
    }
}

/// Where the deployment's registry lives under a data root.
#[must_use]
pub fn registry_dir(data_root: &Path) -> PathBuf {
    data_root.join("storage")
}
