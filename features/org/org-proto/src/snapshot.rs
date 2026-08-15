//! Server-native git snapshot RPC.
//!
//! [`SnapshotService`] runs at `/server/vox` next to
//! [`crate::OrgManagementService`] (one endpoint per task-server
//! process, not per-org). It is the server-native replacement for
//! the chart's CronJob git script: the server itself quiesces
//! writes, checkpoints every open sqlite handle, commits the
//! detached git repos under `<data_root>/.gitstate/`, and pushes
//! them — so the committed sqlite files are *consistent*, not
//! best-effort point-in-time copies.
//!
//! ## Repo layout (unchanged from the script — live in prod)
//!
//! - `<data_root>/.gitstate/orgs/<slug>.git` — work-tree
//!   `<data_root>/orgs/<slug>`, pushed to
//!   `<remote_base>/<org_repo_prefix><slug>`. These repos continue
//!   users' pre-existing vault histories; they are never re-inited.
//! - `<data_root>/.gitstate/full.git` — work-tree `<data_root>`,
//!   excludes `.gitstate/` + `lost+found/`, pushed to
//!   `<remote_base>/<full_repo>`.
//!
//! ## Authorization
//!
//! Same model as [`crate::OrgManagementService`]: bootstrap mode
//! (no orgs hosted) is permissive; otherwise `session_token` must
//! validate against the home org. The server additionally accepts
//! the configured backup token (`TASK_BACKUP_GIT_TOKEN`) as the
//! session token, so non-interactive automation can drive the
//! verbs without a user session.

use facet::Facet;
use thiserror::Error;

/// Trait-boundary error type. Variants stay flat so the same enum
/// travels cleanly over vox.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(u8)]
pub enum SnapshotError {
    /// Caller's token isn't a valid home-org session nor the
    /// configured backup token.
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    /// A snapshot/restore cycle is already running. Cycles are
    /// serialized — retry after the running one finishes.
    #[error("busy: {0}")]
    Busy(String),
    /// The server refused the operation (bad confirmation string,
    /// dirty tree without `force`, unknown commit, …).
    #[error("refused: {0}")]
    Refused(String),
    /// A `git` subprocess failed. Carries the (token-scrubbed)
    /// stderr.
    #[error("git: {0}")]
    Git(String),
    /// Catch-all (I/O, missing data root, …).
    #[error("internal: {0}")]
    Internal(String),
}

/// Outcome for one repo in a snapshot cycle (one per org repo,
/// plus one for the full-state repo).
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct RepoResult {
    /// Remote repo name (`task-org-<slug>` / `task-data`). Stable
    /// even when no remote is configured — it identifies the repo.
    pub repo: String,
    /// New commit sha when a commit was made; empty when the tree
    /// was clean (or the commit failed — see `error`).
    pub committed: String,
    /// True when the work tree had no changes and the commit was
    /// skipped.
    pub clean: bool,
    /// True when the push to the configured remote succeeded.
    /// Always false when no remote is configured.
    pub pushed: bool,
    /// Per-repo failure detail (commit or push). Empty = ok. A
    /// failed repo never aborts the rest of the cycle.
    pub error: String,
}

/// Result of one full snapshot cycle.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct SnapshotReport {
    /// UTC stamp used in the commit messages (`snapshot <stamp>`).
    pub stamp: String,
    /// Per-repo outcomes: every org repo, then the full repo.
    pub repos: Vec<RepoResult>,
}

/// One commit in the full-state repo's history.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct SnapshotLogEntry {
    /// Full commit sha.
    pub commit: String,
    /// Committer date, ISO-8601.
    pub timestamp: String,
    /// Subject line (`snapshot <stamp>`, or whatever a human
    /// committed out-of-band).
    pub message: String,
}

/// Result of `branch` — the "branch the data" verb.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct BranchResult {
    /// Branch name as created.
    pub name: String,
    /// Commit the branch points at (full-repo HEAD).
    pub commit: String,
    /// True when the branch was pushed to the configured remote.
    pub pushed: bool,
}

/// Result of `restore`. When `restarting` is true the server
/// process exits right after replying — the supervisor
/// (k8s/systemd) restarts it on the restored data. Local dev runs
/// must restart `task-server` by hand.
#[derive(Debug, Clone, PartialEq, Eq, Facet)]
#[repr(C)]
pub struct RestoreReport {
    /// The commit the data root was restored to (resolved sha).
    pub commit: String,
    /// Rescue snapshot taken before the restore (default path).
    /// Empty when `force` skipped it.
    pub pre_restore: Vec<RepoResult>,
    /// True when the server is about to exit for restart.
    pub restarting: bool,
}

/// Server-data git snapshots. Mounted at `/server/vox`.
#[architect::rpc]
pub trait SnapshotService {
    /// Run one full snapshot cycle: quiesce writes → WAL-checkpoint
    /// every open sqlite handle → commit every org repo + the full
    /// repo → release the gate → push (when a remote is
    /// configured). Returns per-repo results; individual repo
    /// failures are reported, not fatal.
    async fn snapshot(&self, session_token: String) -> Result<SnapshotReport, SnapshotError>;

    /// Recent snapshot commits on the full-state repo, newest
    /// first. `limit` caps the count (0 = server default of 20).
    async fn log(
        &self,
        session_token: String,
        limit: u32,
    ) -> Result<Vec<SnapshotLogEntry>, SnapshotError>;

    /// Create branch `name` at the full repo's HEAD (and push it
    /// when a remote is configured) — the "branch the data" verb.
    async fn branch(
        &self,
        session_token: String,
        name: String,
    ) -> Result<BranchResult, SnapshotError>;

    /// Restore the data root to `commit` via the full repo
    /// (`git checkout <commit> -- .`), then **exit the server
    /// process** so the supervisor restarts it on the restored
    /// data.
    ///
    /// Safety rails:
    /// - `confirm` must equal `commit` exactly (a deliberate
    ///   retype, not a copy of another arg the caller already had
    ///   in a variable).
    /// - by default a rescue snapshot cycle runs first, so the
    ///   pre-restore state is committed and the tree is clean.
    /// - `force = true` skips the rescue snapshot and proceeds
    ///   even when the work tree has uncommitted changes. Without
    ///   `force`, a dirty tree that fails to snapshot aborts the
    ///   restore.
    async fn restore(
        &self,
        session_token: String,
        commit: String,
        confirm: String,
        force: bool,
    ) -> Result<RestoreReport, SnapshotError>;
}
