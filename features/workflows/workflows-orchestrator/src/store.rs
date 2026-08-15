//! `WorkflowStore` — a per-org, **per-session-isolated** JSON file
//! store for the workflow audit log.
//!
//! Each session's rows live in their own files, keyed by session id:
//!
//! ```text
//! ~/.task/orgs/<slug>/workflows/
//!   sessions/<id>.json      one WorkSession
//!   goals/<id>.json         one GoalSession
//!   activities/<id>.json    Vec<Activity>   for that session
//!   transitions/<id>.json   Vec<Transition> for that session
//!   handoffs/<id>.json      Vec<Handoff>    for that session
//!   locks/<id>.lock         advisory lock for that session's writes
//! ```
//!
//! **Why per-session files:** two `task agent goal` loops on different
//! sessions then never touch the same file, so any number of agents
//! can run concurrently with zero contention — the isolation that
//! makes "as many task agents as you want" safe. Listing globs the
//! directory instead of reading one shared array.
//!
//! **Within-session safety:** writes to one session's files take an
//! advisory `flock` on `locks/<id>.lock`, so a `goal update` /
//! `subgoal` from another process can't interleave with the loop's
//! per-turn progress write (lost update). Reads are lock-free — the
//! atomic temp-file + rename on every write means a reader always sees
//! a complete file.
//!
//! Legacy flat files (`sessions.json`, …) from the previous layout are
//! read as a fallback when a per-session file is missing, so existing
//! state migrates lazily.

use std::fs::File;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use uuid::Uuid;
use workflows_proto::{Activity, GoalSession, Handoff, Transition, WorkSession, WorkflowError};

/// Maps any IO / codec failure onto [`WorkflowError::Backend`].
fn backend<E: std::fmt::Display>(ctx: &str) -> impl Fn(E) -> WorkflowError + '_ {
    move |e| WorkflowError::Backend(format!("{ctx}: {e}"))
}

// Per-session subdirectories.
const SESSIONS: &str = "sessions";
const GOALS: &str = "goals";
const ACTIVITIES: &str = "activities";
const TRANSITIONS: &str = "transitions";
const HANDOFFS: &str = "handoffs";

// Legacy flat files (previous layout) — read-only fallback.
const LEGACY_SESSIONS: &str = "sessions.json";
const LEGACY_GOALS: &str = "goals.json";
const LEGACY_ACTIVITIES: &str = "activities.json";
const LEGACY_TRANSITIONS: &str = "transitions.json";
const LEGACY_HANDOFFS: &str = "handoffs.json";

/// File-backed store rooted at one org's `workflows/` directory.
#[derive(Debug, Clone)]
pub struct WorkflowStore {
    root: PathBuf,
}

impl WorkflowStore {
    /// Open (lazily — nothing is read until a getter is called) the
    /// store rooted at `dir`. Directories are created on first write.
    pub fn open(dir: impl Into<PathBuf>) -> Self {
        Self { root: dir.into() }
    }

    fn entry_path(&self, kind: &str, id: Uuid) -> PathBuf {
        self.root.join(kind).join(format!("{id}.json"))
    }

    /// Take an exclusive advisory lock for one session's writes, held
    /// until the returned guard drops. Serializes concurrent
    /// read-modify-write on the *same* session across processes.
    #[allow(clippy::incompatible_msrv)] // File::lock — toolchain is 1.94
    fn lock_session(&self, id: Uuid) -> Result<File, WorkflowError> {
        let dir = self.root.join("locks");
        std::fs::create_dir_all(&dir).map_err(backend("mkdir locks"))?;
        let f = File::create(dir.join(format!("{id}.lock"))).map_err(backend("lockfile"))?;
        f.lock().map_err(backend("flock"))?; // exclusive, blocking
        Ok(f)
    }

    /// Read + JSON-decode a file. Missing → `None`.
    fn read_json<T: DeserializeOwned>(p: &Path) -> Result<Option<T>, WorkflowError> {
        if !p.exists() {
            return Ok(None);
        }
        let bytes = std::fs::read(p).map_err(backend("read"))?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(backend("decode"))
    }

    /// Atomically write `value` as JSON (temp file + rename).
    fn write_json<T: Serialize>(p: &Path, value: &T) -> Result<(), WorkflowError> {
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent).map_err(backend("mkdir"))?;
        }
        let tmp = p.with_extension("json.tmp");
        let json = serde_json::to_vec_pretty(value).map_err(backend("encode"))?;
        std::fs::write(&tmp, json).map_err(backend("write"))?;
        std::fs::rename(&tmp, p).map_err(backend("rename"))?;
        Ok(())
    }

    /// All session ids that have a `kind/<id>.json` file.
    fn ids_in(&self, kind: &str) -> Result<Vec<Uuid>, WorkflowError> {
        let dir = self.root.join(kind);
        let rd = match std::fs::read_dir(&dir) {
            Ok(rd) => rd,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(backend("readdir")(e)),
        };
        let mut ids = Vec::new();
        for entry in rd {
            let entry = entry.map_err(backend("direntry"))?;
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "json") {
                if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                    if let Ok(id) = Uuid::parse_str(stem) {
                        ids.push(id);
                    }
                }
            }
        }
        Ok(ids)
    }

    /// Read a legacy flat `Vec<T>` file (previous layout). Missing → `[]`.
    fn legacy_vec<T: DeserializeOwned>(&self, file: &str) -> Result<Vec<T>, WorkflowError> {
        Ok(Self::read_json(&self.root.join(file))?.unwrap_or_default())
    }

    // ── sessions (one per file) ──────────────────────────────

    pub fn sessions(&self) -> Result<Vec<WorkSession>, WorkflowError> {
        let mut out = Vec::new();
        for id in self.ids_in(SESSIONS)? {
            if let Some(s) = Self::read_json(&self.entry_path(SESSIONS, id))? {
                out.push(s);
            }
        }
        if out.is_empty() {
            // Lazy migration fallback.
            return self.legacy_vec(LEGACY_SESSIONS);
        }
        Ok(out)
    }

    pub fn session(&self, id: Uuid) -> Result<WorkSession, WorkflowError> {
        if let Some(s) = Self::read_json(&self.entry_path(SESSIONS, id))? {
            return Ok(s);
        }
        self.legacy_vec::<WorkSession>(LEGACY_SESSIONS)?
            .into_iter()
            .find(|s| s.id == id)
            .ok_or(WorkflowError::SessionNotFound(id))
    }

    /// Insert or replace a session by id.
    pub fn put_session(&self, session: &WorkSession) -> Result<(), WorkflowError> {
        let _guard = self.lock_session(session.id)?;
        Self::write_json(&self.entry_path(SESSIONS, session.id), session)
    }

    // ── transitions (Vec per session file) ───────────────────

    /// All transitions for `session`, oldest first.
    pub fn transitions_for(&self, session: Uuid) -> Result<Vec<Transition>, WorkflowError> {
        let mut rows: Vec<Transition> =
            match Self::read_json(&self.entry_path(TRANSITIONS, session))? {
                Some(v) => v,
                None => self
                    .legacy_vec::<Transition>(LEGACY_TRANSITIONS)?
                    .into_iter()
                    .filter(|t| t.session_id == session)
                    .collect(),
            };
        rows.sort_by_key(|t| t.at);
        Ok(rows)
    }

    pub fn push_transition(&self, t: &Transition) -> Result<(), WorkflowError> {
        let _guard = self.lock_session(t.session_id)?;
        let mut rows = self.transitions_for(t.session_id)?;
        rows.push(t.clone());
        Self::write_json(&self.entry_path(TRANSITIONS, t.session_id), &rows)
    }

    // ── activities (Vec per session file) ────────────────────

    /// All activity for `session`, newest first.
    pub fn activities_for(&self, session: Uuid) -> Result<Vec<Activity>, WorkflowError> {
        let mut rows: Vec<Activity> = match Self::read_json(&self.entry_path(ACTIVITIES, session))?
        {
            Some(v) => v,
            None => self
                .legacy_vec::<Activity>(LEGACY_ACTIVITIES)?
                .into_iter()
                .filter(|a| a.session_id == session)
                .collect(),
        };
        rows.sort_by(|a, b| b.at.cmp(&a.at));
        Ok(rows)
    }

    pub fn push_activity(&self, a: &Activity) -> Result<(), WorkflowError> {
        let _guard = self.lock_session(a.session_id)?;
        // Re-read under the lock (oldest-first) so concurrent appends
        // to the same session don't clobber each other.
        let mut rows = self.activities_for(a.session_id)?;
        rows.sort_by(|x, y| x.at.cmp(&y.at));
        rows.push(a.clone());
        Self::write_json(&self.entry_path(ACTIVITIES, a.session_id), &rows)
    }

    // ── goal sessions (one per file) ─────────────────────────

    /// Every persisted goal-loop row.
    pub fn goals(&self) -> Result<Vec<GoalSession>, WorkflowError> {
        let mut out = Vec::new();
        for id in self.ids_in(GOALS)? {
            if let Some(g) = Self::read_json(&self.entry_path(GOALS, id))? {
                out.push(g);
            }
        }
        if out.is_empty() {
            return self.legacy_vec(LEGACY_GOALS);
        }
        Ok(out)
    }

    /// The goal row for `session`, if the session is a goal loop.
    pub fn goal(&self, session: Uuid) -> Result<GoalSession, WorkflowError> {
        if let Some(g) = Self::read_json(&self.entry_path(GOALS, session))? {
            return Ok(g);
        }
        self.legacy_vec::<GoalSession>(LEGACY_GOALS)?
            .into_iter()
            .find(|g| g.session_id == session)
            .ok_or(WorkflowError::SessionNotFound(session))
    }

    /// Insert or replace a goal row by `session_id`.
    pub fn put_goal(&self, goal: &GoalSession) -> Result<(), WorkflowError> {
        let _guard = self.lock_session(goal.session_id)?;
        Self::write_json(&self.entry_path(GOALS, goal.session_id), goal)
    }

    /// Atomic read-modify-write of a goal row under the session lock.
    /// Use this for partial updates (turn progress, condition,
    /// subgoals) so a concurrent writer can't lose your change.
    pub fn mutate_goal<F>(&self, session: Uuid, apply: F) -> Result<GoalSession, WorkflowError>
    where
        F: FnOnce(&mut GoalSession),
    {
        let _guard = self.lock_session(session)?;
        let mut g = match Self::read_json::<GoalSession>(&self.entry_path(GOALS, session))? {
            Some(g) => g,
            None => self
                .legacy_vec::<GoalSession>(LEGACY_GOALS)?
                .into_iter()
                .find(|g| g.session_id == session)
                .ok_or(WorkflowError::SessionNotFound(session))?,
        };
        apply(&mut g);
        Self::write_json(&self.entry_path(GOALS, session), &g)?;
        Ok(g)
    }

    /// Drop the goal row for `session` (no-op if absent).
    pub fn remove_goal(&self, session: Uuid) -> Result<(), WorkflowError> {
        let _guard = self.lock_session(session)?;
        let p = self.entry_path(GOALS, session);
        match std::fs::remove_file(&p) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(backend("remove goal")(e)),
        }
    }

    // ── handoffs (Vec per session file) ──────────────────────

    /// Handoffs for one session.
    pub fn handoffs_for(&self, session: Uuid) -> Result<Vec<Handoff>, WorkflowError> {
        match Self::read_json(&self.entry_path(HANDOFFS, session))? {
            Some(v) => Ok(v),
            None => Ok(self
                .legacy_vec::<Handoff>(LEGACY_HANDOFFS)?
                .into_iter()
                .filter(|h| h.session_id == session)
                .collect()),
        }
    }

    /// Replace one session's handoffs.
    pub fn save_handoffs_for(&self, session: Uuid, rows: &[Handoff]) -> Result<(), WorkflowError> {
        let _guard = self.lock_session(session)?;
        Self::write_json(&self.entry_path(HANDOFFS, session), &rows.to_vec())
    }

    /// Every handoff across all sessions (for inbox-style listing).
    pub fn handoffs(&self) -> Result<Vec<Handoff>, WorkflowError> {
        let mut out = Vec::new();
        for id in self.ids_in(HANDOFFS)? {
            if let Some(v) = Self::read_json::<Vec<Handoff>>(&self.entry_path(HANDOFFS, id))? {
                out.extend(v);
            }
        }
        if out.is_empty() {
            return self.legacy_vec(LEGACY_HANDOFFS);
        }
        Ok(out)
    }

    /// Convenience: the directory this store writes to.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use workflows_proto::{ActivityKind, AgentRef, SubjectRef, WorkflowKind};

    fn store() -> (WorkflowStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (WorkflowStore::open(dir.path()), dir)
    }

    #[test]
    fn sessions_round_trip_and_list() {
        let (s, _d) = store();
        let a = WorkSession::start(
            WorkflowKind::Coding,
            SubjectRef::Task { id: Uuid::new_v4() },
            AgentRef::agent("claude"),
        );
        let b = WorkSession::start(
            WorkflowKind::Coding,
            SubjectRef::Task { id: Uuid::new_v4() },
            AgentRef::agent("codex"),
        );
        s.put_session(&a).unwrap();
        s.put_session(&b).unwrap();
        assert_eq!(s.sessions().unwrap().len(), 2);
        assert_eq!(s.session(a.id).unwrap().id, a.id);
    }

    #[test]
    fn mutate_goal_is_atomic_rmw() {
        let (s, _d) = store();
        let sid = Uuid::new_v4();
        s.put_goal(&GoalSession::new(sid, "do the thing", 10))
            .unwrap();
        s.mutate_goal(sid, |g| g.turns_used = 3).unwrap();
        s.mutate_goal(sid, |g| g.condition = "steered".into())
            .unwrap();
        let g = s.goal(sid).unwrap();
        assert_eq!(g.turns_used, 3, "earlier mutation preserved");
        assert_eq!(g.condition, "steered");
    }

    #[test]
    fn concurrent_loops_on_distinct_sessions_dont_lose_rows() {
        let (s, _d) = store();
        let s = Arc::new(s);
        let n = 8;
        let mut handles = Vec::new();
        for _ in 0..n {
            let s = Arc::clone(&s);
            handles.push(std::thread::spawn(move || {
                let sess = WorkSession::start(
                    WorkflowKind::Coding,
                    SubjectRef::Task { id: Uuid::new_v4() },
                    AgentRef::agent("claude"),
                );
                s.put_session(&sess).unwrap();
                for _ in 0..5 {
                    let act = Activity::record(
                        sess.id,
                        ActivityKind::ToolCall,
                        AgentRef::agent("claude"),
                        &serde_json::json!({}),
                    );
                    s.push_activity(&act).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // Every session persisted, each with all 5 activities — no
        // cross-session clobbering.
        assert_eq!(s.sessions().unwrap().len(), n);
        for sess in s.sessions().unwrap() {
            assert_eq!(s.activities_for(sess.id).unwrap().len(), 5);
        }
    }

    #[test]
    fn concurrent_appends_same_session_keep_all() {
        let (s, _d) = store();
        let s = Arc::new(s);
        let sid = Uuid::new_v4();
        let sess = WorkSession {
            id: sid,
            ..WorkSession::start(
                WorkflowKind::Coding,
                SubjectRef::Task { id: Uuid::new_v4() },
                AgentRef::agent("claude"),
            )
        };
        s.put_session(&sess).unwrap();
        let mut handles = Vec::new();
        for _ in 0..10 {
            let s = Arc::clone(&s);
            handles.push(std::thread::spawn(move || {
                let act = Activity::record(
                    sid,
                    ActivityKind::ToolCall,
                    AgentRef::agent("claude"),
                    &serde_json::json!({}),
                );
                s.push_activity(&act).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        // The session lock serializes the appends — all 10 survive.
        assert_eq!(s.activities_for(sid).unwrap().len(), 10);
    }
}
