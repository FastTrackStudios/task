//! The cadence state machine: when a root gets an **auto-snapshot**,
//! when its session ends in a **Session checkpoint**, and which
//! project-file saves ride along as **save points** (all three glossary
//! terms, `apps/task/CONTEXT.md`).
//!
//! The engine knows nothing about jj, the CAS, or the filesystem. It
//! takes activity hints and a clock and answers one question —
//! *what is due right now?* — which is exactly what makes the whole
//! cadence testable without sleeping through a 30-minute quiescence
//! window. [`crate::FilesBackend`] does the storage half: it asks
//! [`CadenceEngine::take_due`], performs each capture, and reports back.
//!
//! # The cadence itself
//!
//! - Activity (a watcher hint, or [`files_proto::FilesService::hint_activity`])
//!   opens a **session** on the root if none is open, and extends it.
//!   Sessions are per-root: concurrent writers share one (glossary).
//! - While the session has uncaptured activity, an auto-snapshot falls
//!   due once [`CadenceConfig::snapshot_debounce`] (default 10 min) has
//!   passed since the last capture. That interval *is* the debounce: a
//!   recording pass writing every few seconds coalesces into one
//!   snapshot per window, not one per write.
//! - Once activity stops for [`CadenceConfig::quiescence`] (default 30
//!   min), the session ends in one certified Session checkpoint and the
//!   root's cadence state is dropped. A quiescent root is silent: no
//!   further checkpoints until someone writes again.
//! - Quiescence outranks the snapshot window — a checkpoint full-scans
//!   the live tree anyway, so taking a snapshot on the way out would
//!   capture the same bytes twice.

use std::collections::HashMap;
use std::sync::Mutex;

use chrono::{DateTime, TimeDelta, Utc};
use files_proto::SavePoint;
use uuid::Uuid;

use super::clock::Clock;
use super::filter::ActivityFilter;
use std::sync::Arc;

/// The tunables of the cadence (spec #255: "~10 min" snapshots,
/// "default 30 min" quiescence).
#[derive(Debug, Clone, Copy)]
pub struct CadenceConfig {
    /// How long uncaptured activity coalesces before an auto-snapshot
    /// falls due.
    pub snapshot_debounce: TimeDelta,
    /// How long a root must go without activity before its session ends
    /// in a Session checkpoint.
    pub quiescence: TimeDelta,
    /// How many times the certifying scan re-reads a file that changed
    /// while it was being hashed before giving up and requeueing it
    /// (see [`crate::certify`]).
    pub certify_attempts: u32,
}

impl Default for CadenceConfig {
    fn default() -> Self {
        Self {
            snapshot_debounce: TimeDelta::minutes(10),
            quiescence: TimeDelta::minutes(30),
            certify_attempts: 3,
        }
    }
}

/// What a due capture is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DueKind {
    /// An ephemeral auto-snapshot, mid-session.
    Snapshot,
    /// The session-ending, scan-certified checkpoint.
    Checkpoint,
}

/// One capture the backend should perform now.
#[derive(Debug, Clone)]
pub struct Due {
    pub root_id: Uuid,
    pub kind: DueKind,
    /// When this capture was taken — the instant its certifying scan
    /// will enumerate the live tree *from*. Activity hinted after it
    /// cannot be in this capture, which is what
    /// [`CadenceEngine::completed`] uses to decide whether the session
    /// really ended (see its doc).
    pub taken_at: DateTime<Utc>,
    /// The save points this capture carries: for a snapshot, the
    /// project-file saves since the last capture ("the nearest
    /// auto-snapshot"); for a checkpoint, every save point of the
    /// session it closes.
    pub save_points: Vec<SavePoint>,
}

/// A session [`CadenceEngine::end_session`] took out of the engine,
/// carried by the caller across the capture it was ended for so a
/// failure can put it back.
#[derive(Debug)]
pub struct EndedSession {
    root_id: Uuid,
    state: Option<RootCadence>,
}

impl EndedSession {
    /// The save points the ended session accumulated — what the capture
    /// records on the checkpoint it writes.
    #[must_use]
    pub fn save_points(&self) -> Vec<SavePoint> {
        self.state
            .as_ref()
            .map(RootCadence::all_save_points)
            .unwrap_or_default()
    }
}

#[derive(Debug)]
struct RootCadence {
    /// Last capture (session start, or the most recent auto-snapshot) —
    /// the origin the snapshot debounce window is measured from.
    last_capture_at: DateTime<Utc>,
    /// Most recent activity hint — the origin quiescence is measured
    /// from.
    last_activity_at: DateTime<Utc>,
    /// Activity has happened since `last_capture_at`.
    uncaptured_activity: bool,
    /// A capture for this root is being performed right now; nothing
    /// else falls due until it reports back.
    in_flight: bool,
    /// Save points since the last capture.
    pending_save_points: Vec<SavePoint>,
    /// Save points captured earlier in this session (already carried by
    /// a snapshot), kept so the session's checkpoint reports the whole
    /// session's saves.
    session_save_points: Vec<SavePoint>,
}

impl RootCadence {
    fn open(now: DateTime<Utc>) -> Self {
        Self {
            last_capture_at: now,
            last_activity_at: now,
            uncaptured_activity: true,
            in_flight: false,
            pending_save_points: Vec::new(),
            session_save_points: Vec::new(),
        }
    }

    fn all_save_points(&self) -> Vec<SavePoint> {
        let mut all = self.session_save_points.clone();
        all.extend(self.pending_save_points.iter().cloned());
        all
    }
}

/// Per-root cadence state plus the clock and config it runs on.
#[derive(Debug)]
pub struct CadenceEngine {
    config: CadenceConfig,
    clock: Arc<dyn Clock>,
    roots: Mutex<HashMap<Uuid, RootCadence>>,
}

impl CadenceEngine {
    #[must_use]
    pub fn new(config: CadenceConfig, clock: Arc<dyn Clock>) -> Self {
        Self {
            config,
            clock,
            roots: Mutex::new(HashMap::new()),
        }
    }

    #[must_use]
    pub fn config(&self) -> &CadenceConfig {
        &self.config
    }

    #[must_use]
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// Record activity on `root_id`. `paths` are root-relative; those
    /// the Ignore set covers are dropped before they can open a session
    /// (the whole point of the set: a `.rpp-bak` storm is not a working
    /// session). Returns how many hints survived the filter.
    ///
    /// A surviving path that names a project file for `flavor` also
    /// marks a save point.
    pub fn note_activity(
        &self,
        root_id: Uuid,
        paths: &[String],
        filter: &impl ActivityFilter,
    ) -> u32 {
        let now = self.clock.now();
        let live: Vec<&String> = paths
            .iter()
            .filter(|p| !filter.is_ignored(p))
            .collect();
        if live.is_empty() {
            return 0;
        }
        let mut roots = self.roots.lock().expect("cadence state poisoned");
        let state = roots
            .entry(root_id)
            .or_insert_with(|| RootCadence::open(now));
        state.last_activity_at = now;
        state.uncaptured_activity = true;
        for path in &live {
            if filter.is_project_file(path) {
                state.pending_save_points.push(SavePoint {
                    path: (*path).clone(),
                    at: now,
                });
            }
        }
        u32::try_from(live.len()).unwrap_or(u32::MAX)
    }

    /// Everything due as of the engine's clock, marked in-flight so a
    /// second tick (or a second driver) can't perform the same capture
    /// twice. Every returned [`Due`] must be answered with
    /// [`CadenceEngine::completed`] or [`CadenceEngine::failed`].
    pub fn take_due(&self) -> Vec<Due> {
        let now = self.clock.now();
        let mut roots = self.roots.lock().expect("cadence state poisoned");
        let mut due = Vec::new();
        for (root_id, state) in roots.iter_mut() {
            if state.in_flight {
                continue;
            }
            let kind = if now - state.last_activity_at >= self.config.quiescence {
                DueKind::Checkpoint
            } else if state.uncaptured_activity
                && now - state.last_capture_at >= self.config.snapshot_debounce
            {
                DueKind::Snapshot
            } else {
                continue;
            };
            state.in_flight = true;
            due.push(Due {
                root_id: *root_id,
                kind,
                taken_at: now,
                save_points: match kind {
                    DueKind::Snapshot => state.pending_save_points.clone(),
                    DueKind::Checkpoint => state.all_save_points(),
                },
            });
        }
        due
    }

    /// A capture succeeded. A snapshot restarts the debounce window and
    /// carries its save points into the session's history; a checkpoint
    /// ends the session — *unless* someone wrote while the capture was
    /// running.
    ///
    /// That exception is the subtlety here. A capture's certifying scan
    /// enumerates the live tree once, at [`Due::taken_at`]; a multi-GB
    /// root's scan and commit take minutes, and a hint arriving inside
    /// that window describes a write no capture has seen (the
    /// certification retries in [`crate::certify`] cover a file moving
    /// mid-hash, not a file written after enumeration). Ending the
    /// session anyway would drop the root's cadence state, and with it
    /// any chance of a quiescence checkpoint ever falling due for that
    /// write — if it was the last save of the day it would stay
    /// unversioned indefinitely (PR #283 review). So activity newer than
    /// `taken_at` keeps the session alive with its work still marked
    /// uncaptured, and the same test keeps a snapshot from clearing a
    /// flag it did not earn.
    pub fn completed(&self, due: &Due) {
        let now = self.clock.now();
        let mut roots = self.roots.lock().expect("cadence state poisoned");
        let Some(state) = roots.get_mut(&due.root_id) else {
            return;
        };
        let missed_activity = state.last_activity_at > due.taken_at;
        state.in_flight = false;
        state.last_capture_at = now;
        if !missed_activity {
            state.uncaptured_activity = false;
        }

        // Save points this capture actually recorded are consumed;
        // anything marked while it was in flight stays pending for the
        // next one. `due.save_points` was built from the session list
        // plus a prefix of the pending list, so its length says how much
        // of `pending` went in.
        let from_pending = due
            .save_points
            .len()
            .saturating_sub(state.session_save_points.len())
            .min(state.pending_save_points.len());

        match due.kind {
            DueKind::Checkpoint if !missed_activity => {
                roots.remove(&due.root_id);
            }
            DueKind::Checkpoint => {
                // The session continues from the write this checkpoint
                // could not have seen; everything it did record is now
                // durable on the commit, so none of it carries forward.
                state.pending_save_points.drain(..from_pending);
                state.session_save_points.clear();
            }
            DueKind::Snapshot => {
                state
                    .session_save_points
                    .extend(state.pending_save_points.drain(..from_pending));
            }
        }
    }

    /// A capture failed. Nothing is consumed: the same capture falls
    /// due again on the next tick.
    pub fn failed(&self, due: &Due) {
        let mut roots = self.roots.lock().expect("cadence state poisoned");
        if let Some(state) = roots.get_mut(&due.root_id) {
            state.in_flight = false;
        }
    }

    /// End `root_id`'s session out of band — what an explicit
    /// "checkpoint now" does, since it certifies the same live tree the
    /// quiescence checkpoint would have.
    ///
    /// The session is removed *optimistically*, before the capture it
    /// belongs to has been written, because the capture needs its save
    /// points to record them. If that capture then fails, the caller
    /// must hand the returned [`EndedSession`] back to
    /// [`CadenceEngine::restore_session`] — the out-of-band counterpart
    /// of [`CadenceEngine::failed`]. Dropping it instead would lose the
    /// session's save points *and* its accumulated activity, so a root
    /// whose explicit checkpoint failed would never checkpoint at
    /// quiescence either (PR #283 review).
    #[must_use]
    pub fn end_session(&self, root_id: Uuid) -> EndedSession {
        let mut roots = self.roots.lock().expect("cadence state poisoned");
        EndedSession {
            root_id,
            state: roots.remove(&root_id),
        }
    }

    /// Put back a session [`CadenceEngine::end_session`] removed, after
    /// the capture it was ended for failed. If activity has reopened a
    /// session in the meantime, the two are merged: the restored save
    /// points go back in front of the new ones, and the newer activity
    /// timestamps win.
    pub fn restore_session(&self, ended: EndedSession) {
        let Some(old) = ended.state else {
            return;
        };
        let mut roots = self.roots.lock().expect("cadence state poisoned");
        match roots.get_mut(&ended.root_id) {
            Some(current) => {
                let mut restored = old.all_save_points();
                restored.append(&mut current.pending_save_points);
                current.pending_save_points = restored;
                current.session_save_points.clear();
                current.uncaptured_activity = true;
                current.last_activity_at = current.last_activity_at.max(old.last_activity_at);
                current.last_capture_at = current.last_capture_at.min(old.last_capture_at);
            }
            None => {
                roots.insert(ended.root_id, old);
            }
        }
    }

    /// Is a session open on `root_id`?
    #[must_use]
    pub fn session_open(&self, root_id: Uuid) -> bool {
        self.roots
            .lock()
            .expect("cadence state poisoned")
            .contains_key(&root_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cadence::clock::TestClock;
    use crate::cadence::filter::SuffixFilter;

    fn engine() -> (Arc<TestClock>, CadenceEngine) {
        let clock = Arc::new(TestClock::default());
        (
            clock.clone(),
            CadenceEngine::new(CadenceConfig::default(), clock),
        )
    }

    /// Stands in for a media root's Ignore set. The real one is jj's,
    /// built in `files::ignore`; the engine only ever asked it these two
    /// questions.
    fn media_filter() -> SuffixFilter {
        SuffixFilter::new([".rpp-bak", ".reapeaks", ".wfm"], [".rpp", ".ptx"])
    }

    fn write(engine: &CadenceEngine, root: Uuid, path: &str) {
        let n = engine.note_activity(root, &[path.to_string()], &media_filter());
        assert_eq!(n, 1, "{path} should have survived the ignore set");
    }

    /// The acceptance criterion, at the state-machine level: writes
    /// every few minutes yield snapshots, and quiescence yields exactly
    /// one checkpoint.
    #[test]
    fn a_recording_storm_snapshots_then_checkpoints_once() {
        let (clock, engine) = engine();
        let root = Uuid::new_v4();

        let mut snapshots = 0;
        let mut checkpoints = 0;
        // A tracking day: a take lands every 3 minutes for 45 minutes,
        // then everyone goes home. The driver ticks every minute
        // throughout — 150 ticks, one cadence decision each.
        for minute in 0..150 {
            if minute <= 42 && minute % 3 == 0 {
                write(&engine, root, "Audio Files/take.wav");
            }
            for due in engine.take_due() {
                match due.kind {
                    DueKind::Snapshot => snapshots += 1,
                    DueKind::Checkpoint => checkpoints += 1,
                }
                engine.completed(&due);
            }
            clock.advance_minutes(1);
        }

        assert_eq!(
            snapshots, 5,
            "one snapshot per 10-minute window of uncaptured activity"
        );
        assert_eq!(
            checkpoints, 1,
            "exactly one Session checkpoint, at quiescence — not one per tick"
        );
        assert!(!engine.session_open(root), "the session closed with it");
    }

    #[test]
    fn ignored_paths_never_open_a_session() {
        let (clock, engine) = engine();
        let root = Uuid::new_v4();
        let accepted = engine.note_activity(
            root,
            &["El Artisa.rpp-bak".into(), "Audio/kick.reapeaks".into()],
            &media_filter(),
        );
        assert_eq!(accepted, 0);
        assert!(!engine.session_open(root), "backup churn is not a session");
        clock.advance_minutes(60);
        assert!(engine.take_due().is_empty());
    }

    #[test]
    fn a_project_file_save_marks_a_save_point_on_the_nearest_capture() {
        let (clock, engine) = engine();
        let root = Uuid::new_v4();

        write(&engine, root, "Audio Files/take.wav");
        write(&engine, root, "El Artisa.rpp");
        clock.advance_minutes(11);

        let due = engine.take_due();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].kind, DueKind::Snapshot);
        assert_eq!(
            due[0]
                .save_points
                .iter()
                .map(|s| s.path.as_str())
                .collect::<Vec<_>>(),
            ["El Artisa.rpp"],
            "only the project file marks a save point"
        );
        engine.completed(&due[0]);

        // The session's checkpoint still reports it: the save point is
        // session metadata, not snapshot-only.
        write(&engine, root, "Audio Files/take2.wav");
        clock.advance_minutes(31);
        let due = engine.take_due();
        assert_eq!(due[0].kind, DueKind::Checkpoint);
        assert_eq!(due[0].save_points.len(), 1);
    }

    #[test]
    fn a_failed_capture_is_retried_not_lost() {
        let (clock, engine) = engine();
        let root = Uuid::new_v4();
        write(&engine, root, "take.wav");
        clock.advance_minutes(11);

        let due = engine.take_due();
        assert_eq!(due.len(), 1);
        assert!(
            engine.take_due().is_empty(),
            "an in-flight capture is not handed out twice"
        );
        engine.failed(&due[0]);
        assert_eq!(
            engine.take_due().len(),
            1,
            "a failed capture falls due again"
        );
    }
}
