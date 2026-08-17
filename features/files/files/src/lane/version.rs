//! `VersionService` — history, divergence, restore and the advisory
//! occupancy signal.
//!
//! Two surfaces that look unrelated live in one lane because they are the
//! two ends of a single mechanism: nothing blocks a write, so concurrent
//! edits *will* collide, and what makes that tolerable is being told
//! beforehand (occupancy) and losing nothing afterwards (divergence).
//! Splitting them would let one be implemented without the other, which
//! is precisely the combination that loses work.
//!
//! ## Versions are addressed by `VersionId`, the store speaks hex
//!
//! The store's identity for a saved state is a jj `CommitId` — 32 bytes of
//! hex, not a UUID — while this surface addresses versions with
//! [`VersionId`]. Rather than mint and persist a side table mapping one to
//! the other (a second authority over the store's contents, which
//! `files-domain`'s cadence journal deliberately refuses to be), the id is
//! *derived* from the commit hex: see [`version_id_of`]. It is stable,
//! needs no storage, survives a rebuild of the registry, and is the same
//! trick `crate::entity` already uses to give a vault page a stable id.
//!
//! The cost is that resolving a `VersionId` back to a commit means
//! scanning the candidates it could name — a path's chain for
//! [`VersionService::restore`], a root's divergent heads for
//! [`VersionService::resolve_divergence`]. Both sets are small and both
//! were being read anyway to validate the request.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use chrono::{TimeDelta, Utc};
use files_proto::error::FilesFault;
use files_proto::id::{PrincipalId, RootId, SnapshotId, VersionId};
use files_proto::model::{ChainEntry, CheckpointInfo, DivergenceInfo, SnapshotInfo};
use files_proto::path::RootPath;
use files_proto::service::version::{Occupancy, Resolution, VersionService};
use files_proto::{DivergenceChoice, FilesError, FilesService};

use crate::backend::FilesBackend;

/// How long a hold survives without a heartbeat.
///
/// Short on purpose. `files.concurrency.advisory-lock` says the signal
/// "expires on its own when a client vanishes", and a client that vanishes
/// does so silently — a shut laptop sends no goodbye. The only thing that
/// bounds how long a phantom "Sam has this open" is shown is this number,
/// so it is measured against a heartbeat interval (call `hold` on a timer)
/// rather than against a working session.
const OCCUPANCY_TTL: TimeDelta = TimeDelta::seconds(90);

/// The stable [`VersionId`] naming the saved state at a store commit.
///
/// UUIDv5 over the commit hex: a pure function of the identity the store
/// already assigns, so two processes, two replicas and two rebuilds of the
/// registry all agree without coordinating. Callers holding a
/// [`ChainEntry`] or a `DivergenceSide` — both of which carry the hex —
/// derive the id the same way rather than being handed one.
///
/// Delegates to [`VersionId::from_commit_hex`], which is the single
/// definition shared with every other lane. This wrapper survives only
/// because callers already import it.
#[must_use]
pub fn version_id_of(commit_hex: &str) -> VersionId {
    VersionId::from_commit_hex(commit_hex)
}

/// The principal this process speaks for.
///
/// A placeholder, and knowingly so: no method on this trait carries a
/// caller, and `FilesBackend` holds no session — the dispatcher that will
/// supply one is not built. Minting one id per process at least makes the
/// signal *correct within a process* (a client's own holds are its own,
/// two clients on one server are not yet distinguishable) instead of
/// pretending to an attribution we cannot make. Everything else here is
/// written against `PrincipalId`, so threading a real one through is a
/// change to this function and its callers, not to the table.
fn this_principal() -> PrincipalId {
    static ME: OnceLock<PrincipalId> = OnceLock::new();
    *ME.get_or_init(PrincipalId::generate)
}

/// Who has what open, keyed by `(root, path)` then by principal.
///
/// In memory and process-local, because that is what an advisory signal
/// is: persisting it would outlive the client whose open file it
/// describes, and a hold that survives a server restart is exactly the
/// stranded lock the spec forbids. Losing the table on restart costs a
/// round of heartbeats, nothing more.
#[derive(Debug, Default)]
struct Holds(Mutex<HashMap<(RootId, String), HashMap<PrincipalId, Occupancy>>>);

/// The process-wide table. Not on `FilesBackend`: the backend is cloned
/// per call and this is per process, and the alternative — a field —
/// belongs to `backend.rs`, which this lane does not own.
fn holds() -> &'static Holds {
    static HOLDS: OnceLock<Holds> = OnceLock::new();
    HOLDS.get_or_init(Holds::default)
}

impl Holds {
    /// Record or refresh one principal's hold and return it.
    fn touch(&self, root_id: RootId, path: &RootPath, principal: PrincipalId) -> Occupancy {
        let now = Utc::now();
        let mut guard = self.0.lock().expect("occupancy lock poisoned");
        let per_path = guard
            .entry((root_id, path.as_str().to_string()))
            .or_default();
        per_path.retain(|_, o| o.expires_at > now);
        let entry = per_path.entry(principal).or_insert_with(|| Occupancy {
            root_id,
            path: path.clone(),
            principal,
            display_name: "this client".to_string(),
            // `since` is set once and never refreshed: "open since 09:12"
            // is the useful fact, and a heartbeat that reset it would
            // report every held file as opened a moment ago.
            since: now,
            expires_at: now + OCCUPANCY_TTL,
        });
        entry.expires_at = now + OCCUPANCY_TTL;
        entry.clone()
    }

    /// Live holds on a path, oldest first. Expiry is evaluated on read as
    /// well as on write, so a table nobody touches still tells the truth.
    fn live(&self, root_id: RootId, path: &RootPath) -> Vec<Occupancy> {
        let now = Utc::now();
        let mut guard = self.0.lock().expect("occupancy lock poisoned");
        let key = (root_id, path.as_str().to_string());
        let Some(per_path) = guard.get_mut(&key) else {
            return Vec::new();
        };
        per_path.retain(|_, o| o.expires_at > now);
        let mut out: Vec<Occupancy> = per_path.values().cloned().collect();
        if out.is_empty() {
            guard.remove(&key);
        }
        out.sort_by_key(|o| o.since);
        out
    }
}

/// The v1 error surface onto the v2 one.
///
/// The legacy methods this lane delegates to answer `FilesError`, whose
/// four `String` variants predate every typed fault, so this can only be
/// as precise as the prose it is handed. The one distinction worth
/// recovering — an unknown root — is made *before* delegating, by
/// [`FilesBackend::known_root`], which is why `NotFound` collapses to
/// `Invalid` here rather than being guessed at from the message.
fn fault(err: FilesError) -> FilesFault {
    match err {
        FilesError::NotFound(m) | FilesError::BadRequest(m) => FilesFault::Invalid(m),
        FilesError::AlreadyExists(m) => FilesFault::AlreadyRoot(m),
        FilesError::Io(m) => FilesFault::Io(m),
    }
}

impl FilesBackend {
    /// Fail fast on an unknown root, so every method on this lane answers
    /// the typed `RootNotFound` rather than the legacy prose a delegated
    /// call would produce.
    fn known_root(&self, root_id: RootId) -> Result<(), FilesFault> {
        self.registry_get(root_id.get())
            .map(|_| ())
            .ok_or(FilesFault::RootNotFound(root_id))
    }
}

impl VersionService for FilesBackend {
    // t[impl files.version.unit] — the chain addresses the path the caller
    // opened and follows jj's recorded renames through it
    async fn chain(&self, root_id: RootId, path: RootPath) -> Result<Vec<ChainEntry>, FilesFault> {
        self.known_root(root_id)?;
        FilesService::chain(self, root_id.get(), path.as_str().to_string())
            .await
            .map_err(fault)
    }

    // t[impl files.version.cadence] — the explicit half; quiescence and
    // save points are the cadence engine's
    async fn checkpoint(
        &self,
        root_id: RootId,
        description: Option<String>,
    ) -> Result<CheckpointInfo, FilesFault> {
        self.known_root(root_id)?;
        FilesService::checkpoint_now(self, root_id.get(), description)
            .await
            .map_err(fault)
    }

    // t[impl files.version.cadence] — snapshots are not versions, which is
    // why they are listed by their own verb and expire on their own
    async fn snapshots(
        &self,
        root_id: RootId,
        limit: Option<u32>,
    ) -> Result<Vec<SnapshotInfo>, FilesFault> {
        self.known_root(root_id)?;
        let mut all = FilesService::snapshots(self, root_id.get())
            .await
            .map_err(fault)?;
        // Newest first already; truncating keeps the newest, which is the
        // half a recovery UI shows.
        if let Some(limit) = limit {
            all.truncate(limit as usize);
        }
        Ok(all)
    }

    // t[impl files.concurrency.advisory-lock] — publishes the fact and
    // nothing else: no gate, no write refused, no release
    async fn hold(&self, root_id: RootId, path: RootPath) -> Result<Occupancy, FilesFault> {
        self.known_root(root_id)?;
        // Deliberately not checked against the tree: a hold is taken as a
        // file is being opened, and making it depend on a scan would put
        // the one call a client makes on a timer behind disk I/O.
        Ok(holds().touch(root_id, &path, this_principal()))
    }

    // t[impl files.concurrency.advisory-lock]
    async fn occupancy(
        &self,
        root_id: RootId,
        path: RootPath,
    ) -> Result<Vec<Occupancy>, FilesFault> {
        self.known_root(root_id)?;
        // Every live holder, the caller's own included: without a caller
        // identity on this surface the server cannot say which one is
        // "else", and silently omitting a row would be the one failure
        // mode this signal exists to prevent.
        Ok(holds().live(root_id, &path))
    }

    // t[impl files.version.keep-both] — the listing side: both states are
    // already saved by the time anyone is asked about them
    async fn divergences(&self, root_id: RootId) -> Result<Vec<DivergenceInfo>, FilesFault> {
        self.known_root(root_id)?;
        FilesService::divergences(self, root_id.get())
            .await
            .map_err(fault)
    }

    // t[impl files.version.keep-both] — a human picks; nothing is merged
    async fn resolve_divergence(
        &self,
        root_id: RootId,
        version: VersionId,
        resolution: Resolution,
    ) -> Result<DivergenceInfo, FilesFault> {
        self.known_root(root_id)?;
        let info = FilesService::divergences(self, root_id.get())
            .await
            .map_err(fault)?
            .into_iter()
            .find(|d| {
                d.sides
                    .iter()
                    .any(|s| version_id_of(&s.commit_id) == version)
            })
            .ok_or(FilesFault::VersionNotFound(version))?;

        // "Mine" is the journal line — the head the live tree is sitting
        // on — and "theirs" is what arrived beside it. That is fixed by
        // the listing order rather than by which side `version` named,
        // because a caller that names the other side and asks to keep
        // mine means the local one either way.
        let choice = match resolution {
            Resolution::KeepMine => DivergenceChoice::Pick {
                commit_id: info.sides[0].commit_id.clone(),
            },
            Resolution::KeepTheirs => match &info.sides[1..] {
                [one] => DivergenceChoice::Pick {
                    commit_id: one.commit_id.clone(),
                },
                // Three-way divergence has no single "theirs". Naming the
                // side is the caller's job, and `KeepMine` against the
                // other version does it.
                _ => {
                    return Err(FilesFault::invalid(format!(
                        "{}: keep-theirs is ambiguous across {} sides — name the side to keep",
                        info.path,
                        info.sides.len()
                    )));
                }
            },
            // The resolver mints collision-free `<stem> (divergent n)`
            // names for the sides it lands beside the original. Honouring
            // caller-chosen names needs a rename it does not perform, and
            // accepting them only to write different ones would be worse
            // than refusing: the caller would believe the file it asked
            // for exists.
            Resolution::KeepBoth { mine, theirs } if !mine.is_empty() || !theirs.is_empty() => {
                return Err(FilesFault::Internal(
                    "not yet implemented: keep-both under caller-chosen names; pass empty \
                     names to accept the server's `(divergent n)` naming"
                        .into(),
                ));
            }
            Resolution::KeepBoth { .. } => DivergenceChoice::KeepBoth,
        };

        FilesService::resolve_divergence(self, root_id.get(), info.path.clone(), choice)
            .await
            .map_err(fault)?;
        // The divergence as it stood, which is what was settled — reading
        // it back would return nothing, the root now having one head.
        Ok(info)
    }

    // t[impl files.version.restore] — copy-forward then certify: the old
    // state becomes a new version and every prior one is still there
    async fn restore(
        &self,
        root_id: RootId,
        path: RootPath,
        version: VersionId,
    ) -> Result<ChainEntry, FilesFault> {
        self.known_root(root_id)?;
        let before = FilesService::chain(self, root_id.get(), path.as_str().to_string())
            .await
            .map_err(fault)?;
        let target = before
            .iter()
            .find(|e| version_id_of(&e.commit_id) == version)
            .ok_or(FilesFault::VersionNotFound(version))?
            .clone();

        if target.path != path.as_str() {
            // The chain followed a `CopyHistory` back past a rename, so
            // the state asked for lived under another name. Copy-forward
            // restores content to the path it occupied in that commit,
            // which would resurrect the old name beside the current file
            // rather than restore the file the caller named.
            return Err(FilesFault::Internal(format!(
                "not yet implemented: restoring across a recorded rename ({} was {} at that \
                 version)",
                path, target.path
            )));
        }

        FilesService::copy_forward(
            self,
            root_id.get(),
            target.commit_id.clone(),
            vec![target.path.clone()],
        )
        .await
        .map_err(fault)?;
        // A copy into the live tree is not yet a version; the checkpoint
        // is what makes the restore itself part of history.
        FilesService::checkpoint_now(
            self,
            root_id.get(),
            Some(format!("restore {} to {}", path, &target.commit_id)),
        )
        .await
        .map_err(fault)?;

        FilesService::chain(self, root_id.get(), path.as_str().to_string())
            .await
            .map_err(fault)?
            .into_iter()
            .next()
            .ok_or_else(|| {
                FilesFault::Internal(format!("{path}: restored, but produced no chain entry"))
            })
    }

    /// **Not implemented.**
    ///
    /// A snapshot commit hangs off the checkpoint line rather than sitting
    /// on it — that branch is exactly what keeps ephemeral captures out of
    /// every version chain. Promoting one means moving the checkpoint head
    /// onto a snapshot commit and reclassifying its journal line, which
    /// changes what the next checkpoint parents onto and what GC protects.
    /// That is a lineage decision, not a translation, and guessing at it
    /// here would quietly fold auto-snapshots into history — the one
    /// outcome `files.version.cadence` is written to prevent.
    async fn keep_snapshot(
        &self,
        root_id: RootId,
        snapshot: SnapshotId,
    ) -> Result<CheckpointInfo, FilesFault> {
        self.known_root(root_id)?;
        Err(FilesFault::Internal(format!(
            "not yet implemented: promoting snapshot {snapshot} to a durable version"
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uuid::Uuid;

    fn path(p: &str) -> RootPath {
        RootPath::parse(p).expect("valid path")
    }

    #[test]
    fn a_version_id_is_a_pure_function_of_the_commit() {
        let a = version_id_of("deadbeef");
        assert_eq!(a, version_id_of("deadbeef"), "derivation must be stable");
        assert_ne!(a, version_id_of("deadbeee"), "and injective in practice");
    }

    // t[verify files.concurrency.advisory-lock]
    #[test]
    fn a_hold_is_refreshed_rather_than_duplicated() {
        let holds = Holds::default();
        let root = RootId::new(Uuid::from_bytes([7; 16]));
        let me = PrincipalId::generate();

        let first = holds.touch(root, &path("mix.rpp"), me);
        let second = holds.touch(root, &path("mix.rpp"), me);
        assert_eq!(
            second.since, first.since,
            "the open time is the useful fact"
        );
        assert!(
            second.expires_at >= first.expires_at,
            "a heartbeat pushes the lapse out"
        );
        assert_eq!(holds.live(root, &path("mix.rpp")).len(), 1);
    }

    // t[verify files.concurrency.advisory-lock]
    #[test]
    fn a_lapsed_hold_needs_nobody_to_release_it() {
        let holds = Holds::default();
        let root = RootId::new(Uuid::from_bytes([8; 16]));
        let me = PrincipalId::generate();
        holds.touch(root, &path("stems/kick.wav"), me);

        // Age it past the TTL in place — the client vanished, and said
        // nothing on its way out.
        {
            let mut guard = holds.0.lock().unwrap();
            for o in guard.values_mut().flat_map(|m| m.values_mut()) {
                o.expires_at = Utc::now() - TimeDelta::seconds(1);
            }
        }
        assert!(
            holds.live(root, &path("stems/kick.wav")).is_empty(),
            "expiry is evaluated on read: no release, no reaper, no stranded file"
        );
    }

    #[test]
    fn holds_are_per_path_and_per_root() {
        let holds = Holds::default();
        let a = RootId::new(Uuid::from_bytes([1; 16]));
        let b = RootId::new(Uuid::from_bytes([2; 16]));
        let me = PrincipalId::generate();
        holds.touch(a, &path("mix.rpp"), me);

        assert_eq!(holds.live(a, &path("mix.rpp")).len(), 1);
        assert!(holds.live(a, &path("other.rpp")).is_empty());
        assert!(holds.live(b, &path("mix.rpp")).is_empty());
    }

    #[test]
    fn two_principals_on_one_path_both_show() {
        let holds = Holds::default();
        let root = RootId::new(Uuid::from_bytes([3; 16]));
        holds.touch(root, &path("mix.rpp"), PrincipalId::generate());
        holds.touch(root, &path("mix.rpp"), PrincipalId::generate());
        assert_eq!(
            holds.live(root, &path("mix.rpp")).len(),
            2,
            "the collision worth warning about is exactly this one"
        );
    }
}
