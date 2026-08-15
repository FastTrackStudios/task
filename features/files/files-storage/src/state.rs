//! The deployment-scoped registry: agents, locations, grants,
//! placements, outstanding directives and per-agent secrets, persisted as
//! one JSON document (`<dir>/storage.json`).
//!
//! Deployment-scoped is the load-bearing word (glossary "Storage
//! Location"): ONE of these serves every org in the deployment, which is
//! exactly why an org's reach into it is mediated by grants rather than
//! by having its own registry. It is also why the lookups below are
//! **org-scoped on both the read and the write side** — a root id from
//! one org must never resolve to another org's placement (PR #284
//! review).
//!
//! # Locking and durability
//!
//! [`Registry::write`] mutates and serializes under the state lock, then
//! persists **after dropping it**, ordered by a sequence number so a
//! slower writer can never overwrite a newer snapshot. Nothing holds the
//! state lock across a syscall, so a reader never waits on disk.
//! [`Registry::write_volatile`] skips persistence entirely — for fields
//! like an agent's `last_seen`, where rewriting the whole document per
//! heartbeat would be pure write amplification and nothing durable
//! changed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::{DateTime, Duration, Utc};
use files_storage_proto::{
    AgentDirective, AgentInfo, DirectiveKind, RootPlacement, StorageError, StorageGrantInfo,
    StorageLocationInfo,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{Result, io};

/// How long a directive an agent never acknowledged stays outstanding.
/// Without this, a periodic `refresh_usage` against an offline agent
/// appends one entry per tick forever — inflating the document, every
/// serialization of it, and every `pending_directives` reply.
const OUTSTANDING_TTL_HOURS: i64 = 24;

/// A directive handed to an agent that has not reported back yet. The
/// wire directive says nothing about *where* the result lands (the agent
/// does not need to know); the coordinator keeps that here so an
/// incoming outcome can be applied to the right placement.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Outstanding {
    pub directive: AgentDirective,
    /// The location the directive's result belongs to — the live tree's
    /// for hosting/measuring, the replica's for replication.
    pub location_id: Uuid,
    pub issued_at: DateTime<Utc>,
}

impl Outstanding {
    /// What makes two directives "the same work": re-issuing a measure
    /// for a root an offline agent never answered replaces the old one
    /// rather than queueing beside it.
    fn dedupe_key(&self) -> (Uuid, Uuid, u8, Uuid) {
        let (root_id, kind) = match &self.directive.kind {
            DirectiveKind::HostLiveTree { root_id, .. } => (*root_id, 0),
            DirectiveKind::ReplicateBlobs { root_id, .. } => (*root_id, 1),
            DirectiveKind::MeasureLiveTree { root_id, .. } => (*root_id, 2),
        };
        (self.directive.agent_id, root_id, kind, self.location_id)
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    #[serde(default)]
    pub agents: Vec<AgentInfo>,
    #[serde(default)]
    pub locations: Vec<StorageLocationInfo>,
    #[serde(default)]
    pub grants: Vec<StorageGrantInfo>,
    #[serde(default)]
    pub placements: Vec<RootPlacement>,
    #[serde(default)]
    pub outstanding: Vec<Outstanding>,
    /// blake3 of each agent's enrollment secret, keyed by agent id. The
    /// secret itself is transmitted exactly once (in the enrollment
    /// reply) and never stored, so this file leaks no credential even if
    /// it is read.
    #[serde(default)]
    pub agent_secrets: HashMap<Uuid, String>,
}

impl State {
    pub fn agent(&self, id: Uuid) -> Option<&AgentInfo> {
        self.agents.iter().find(|a| a.id == id)
    }

    pub fn agent_mut(&mut self, id: Uuid) -> Option<&mut AgentInfo> {
        self.agents.iter_mut().find(|a| a.id == id)
    }

    pub fn location(&self, id: Uuid) -> Option<&StorageLocationInfo> {
        self.locations.iter().find(|l| l.id == id)
    }

    pub fn location_for_volume(&self, agent_id: Uuid, key: &str) -> Option<&StorageLocationInfo> {
        self.locations
            .iter()
            .find(|l| l.agent_id == agent_id && l.volume_key == key)
    }

    /// The grant admitting `org` onto `location` — the single gate every
    /// placement passes through.
    pub fn grant(&self, org: &str, location_id: Uuid) -> Option<&StorageGrantInfo> {
        self.grants
            .iter()
            .find(|g| g.org == org && g.location_id == location_id)
    }

    pub fn placement(&self, org: &str, root_id: Uuid) -> Option<&RootPlacement> {
        self.placements
            .iter()
            .find(|p| p.org == org && p.root_id == root_id)
    }

    /// The mutable counterpart, scoped by org for the same reason the
    /// read side is: a placement belongs to exactly one org, and a root
    /// id another org happens to name must not resolve to it. Scoping
    /// only the read was enough to let org B overwrite org A's live-tree
    /// binding (PR #284 review).
    pub fn placement_mut(&mut self, org: &str, root_id: Uuid) -> Option<&mut RootPlacement> {
        self.placements
            .iter_mut()
            .find(|p| p.org == org && p.root_id == root_id)
    }

    /// Queue a directive, replacing any equivalent one still outstanding
    /// and dropping anything past its TTL.
    pub fn enqueue(&mut self, outstanding: Outstanding) {
        let cutoff = Utc::now() - Duration::hours(OUTSTANDING_TTL_HOURS);
        let key = outstanding.dedupe_key();
        self.outstanding
            .retain(|o| o.issued_at >= cutoff && o.dedupe_key() != key);
        self.outstanding.push(outstanding);
    }
}

/// The registry file plus the in-memory state it holds.
#[derive(Debug)]
pub struct Registry {
    path: PathBuf,
    state: Mutex<State>,
    /// Monotonic snapshot counter; `persisted` is the highest one that
    /// has reached disk. Together they keep a slow writer from clobbering
    /// a newer document once persistence moved outside the state lock.
    seq: AtomicU64,
    persisted: Mutex<u64>,
}

impl Registry {
    pub fn open(dir: &Path) -> Result<Self> {
        std::fs::create_dir_all(dir).map_err(|e| io("create registry dir", e))?;
        let path = dir.join("storage.json");
        let state = if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| io("read registry", e))?;
            serde_json::from_slice(&bytes).map_err(|e| io("parse registry", e))?
        } else {
            State::default()
        };
        Ok(Self {
            path,
            state: Mutex::new(state),
            seq: AtomicU64::new(0),
            persisted: Mutex::new(0),
        })
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().expect("storage registry lock poisoned")
    }

    /// Read-only access.
    pub fn read<T>(&self, f: impl FnOnce(&State) -> T) -> T {
        f(&self.lock())
    }

    /// Mutate, then persist. `f` runs under the state lock and its
    /// snapshot is serialized there too — but the write and rename
    /// happen after the lock is released. A failed `f` persists nothing.
    pub fn write<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        let (out, bytes, seq) = {
            let mut state = self.lock();
            let out = f(&mut state)?;
            let bytes =
                serde_json::to_vec_pretty(&*state).map_err(|e| io("serialize registry", e))?;
            let seq = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
            (out, bytes, seq)
        };
        self.persist(seq, &bytes)?;
        Ok(out)
    }

    /// Mutate in memory only — for fields whose loss on restart costs
    /// nothing (an agent's `last_seen` is re-established by its next
    /// heartbeat). Keeps a heartbeat from rewriting the whole document.
    pub fn write_volatile<T>(&self, f: impl FnOnce(&mut State) -> Result<T>) -> Result<T> {
        f(&mut self.lock())
    }

    fn persist(&self, seq: u64, bytes: &[u8]) -> Result<()> {
        let mut persisted = self.persisted.lock().expect("registry io lock poisoned");
        if *persisted >= seq {
            // A newer snapshot already reached disk; ours is stale by
            // construction (it was serialized from an older state).
            return Ok(());
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, bytes).map_err(|e| io("write registry", e))?;
        std::fs::rename(&tmp, &self.path).map_err(|e| io("rename registry", e))?;
        *persisted = seq;
        Ok(())
    }

    pub fn require_location(&self, id: Uuid) -> Result<StorageLocationInfo> {
        self.read(|s| s.location(id).cloned())
            .ok_or_else(|| StorageError::NotFound(format!("storage location {id}")))
    }
}
