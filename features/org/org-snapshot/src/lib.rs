//! Server-native git snapshot engine over the Task data root.
//!
//! Operates the **same** detached-git-dir layout the chart's CronJob
//! script (`deploy/chart/task/templates/backup-script.yaml`)
//! established — that layout is live in production with real data, so
//! this engine never re-inits an existing repo and continues the
//! pre-existing histories:
//!
//! - `<data_root>/.gitstate/orgs/<slug>.git` — work-tree
//!   `<data_root>/orgs/<slug>`, remote `<base>/<org_prefix><slug>`.
//!   These continue users' pre-existing vault histories.
//! - `<data_root>/.gitstate/full.git` — work-tree `<data_root>`,
//!   excludes `.gitstate/` + `lost+found/`, remote `<base>/<full_repo>`.
//!
//! Mechanics carried over from the script verbatim: commit identity
//! `task-backup <task-backup@noreply>`, branch `main`, de-gitlink
//! defense (un-record any mode-160000 entry before `add -A`),
//! skip-if-clean, remote auto-create (private) via the Forgejo API,
//! plain fast-forward pushes — **never** `--force`.
//!
//! Shells out to `git` (subprocess-wrapper precedent: wiki-archive's
//! yt-dlp / curl / pdftotext). The server image carries git in its
//! runtime stage.
//!
//! ## Two-phase cycles
//!
//! The server snapshots under a write gate: [`SnapshotEngine::commit_phase`]
//! runs while writes are quiesced (cheap, local), then the gate is
//! released and [`SnapshotEngine::push_phase`] does the slow network
//! part against the already-immutable commits.
//!
//! ## The push-target extension point
//!
//! [`SnapshotEngine::push`] is deliberately the *one* function that
//! knows how bytes leave the machine. Today it speaks "git remote
//! over https (Forgejo auto-create) or a local filesystem base"; a
//! future Nextcloud / WebDAV / NAS backend (rclone-style remote)
//! slots in behind this same function without touching the commit
//! machinery.

use std::path::{Path, PathBuf};

use org_proto::snapshot::{BranchResult, RepoResult, SnapshotLogEntry, SnapshotReport};

/// Engine-boundary error. Token-scrubbed (the remote token never
/// appears in messages).
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// Spawning or running `git` (or `curl` for remote auto-create)
    /// failed at the process level.
    #[error("spawn {bin}: {source}")]
    Spawn {
        bin: &'static str,
        #[source]
        source: std::io::Error,
    },
    /// A git command exited non-zero. `detail` carries the scrubbed
    /// stderr.
    #[error("git {verb}: {detail}")]
    Git { verb: String, detail: String },
    /// Filesystem error outside git (scanning orgs, cleaning WAL
    /// sidecars, …).
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    /// Operation refused (unknown commit, no history yet, …).
    #[error("{0}")]
    Refused(String),
}

/// Where snapshots are pushed. Built from the `TASK_BACKUP_*` env —
/// the chart maps the existing backup secret's `GIT_USER`/`GIT_TOKEN`
/// through to `TASK_BACKUP_GIT_USER`/`TASK_BACKUP_GIT_TOKEN`.
#[derive(Debug, Clone)]
pub struct RemoteConfig {
    /// `https://<forge-host>/<owner>` (Forgejo-compatible API for
    /// auto-create), or a local filesystem directory of bare repos
    /// (tests / NAS mounts).
    pub base: String,
    /// Push auth user (https bases only).
    pub user: String,
    /// Push auth token (https bases only). Never logged.
    pub token: String,
}

impl RemoteConfig {
    /// Read `TASK_BACKUP_REMOTE_BASE` / `TASK_BACKUP_GIT_USER` /
    /// `TASK_BACKUP_GIT_TOKEN`. `None` when no base is configured —
    /// snapshots then commit locally and skip the push phase.
    #[must_use]
    pub fn from_env() -> Option<Self> {
        let base = std::env::var("TASK_BACKUP_REMOTE_BASE")
            .ok()
            .filter(|s| !s.is_empty())?;
        Some(Self {
            base: base.trim_end_matches('/').to_owned(),
            user: std::env::var("TASK_BACKUP_GIT_USER").unwrap_or_default(),
            token: std::env::var("TASK_BACKUP_GIT_TOKEN").unwrap_or_default(),
        })
    }
}

/// One snapshot repo: a detached git dir + the work tree it views.
#[derive(Debug, Clone)]
pub struct RepoSpec {
    /// Detached git dir under `<data_root>/.gitstate/`.
    pub git_dir: PathBuf,
    /// The live data this repo snapshots.
    pub work_tree: PathBuf,
    /// Remote repo name (`task-org-<slug>` / `task-data`).
    pub name: String,
    /// Patterns written to `<git_dir>/info/exclude` (the full repo
    /// excludes `.gitstate/` + `lost+found/`).
    pub excludes: Vec<String>,
    /// Force-add gitignored files (minus volatile sidecars). The
    /// FULL-STATE repo sets this: org dirs may carry user
    /// `.gitignore`s excluding `*.sqlite` ("rebuildable from vault")
    /// — a fine policy for the per-org repos, but the full backup
    /// must contain the databases (auth.sqlite is NOT rebuildable).
    /// Discovered the hard way: a restore from a full snapshot that
    /// honored those ignores produced a server hosting zero orgs.
    pub force_ignored: bool,
}

/// The snapshot engine. Cheap to construct per cycle; all state
/// lives on disk in the git dirs.
#[derive(Debug, Clone)]
pub struct SnapshotEngine {
    data_root: PathBuf,
    remote: Option<RemoteConfig>,
    full_repo: String,
    org_prefix: String,
}

impl SnapshotEngine {
    /// Engine with no remote and the production repo names
    /// (`task-data` / `task-org-`).
    #[must_use]
    pub fn new(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            remote: None,
            full_repo: "task-data".into(),
            org_prefix: "task-org-".into(),
        }
    }

    /// Production constructor: remote from `TASK_BACKUP_*`, repo
    /// names from `TASK_BACKUP_FULL_REPO` / `TASK_BACKUP_ORG_REPO_PREFIX`
    /// (defaults match the chart's `task-data` / `task-org-`).
    #[must_use]
    pub fn from_env(data_root: impl Into<PathBuf>) -> Self {
        let mut engine = Self::new(data_root);
        engine.remote = RemoteConfig::from_env();
        if let Ok(v) = std::env::var("TASK_BACKUP_FULL_REPO") {
            if !v.is_empty() {
                engine.full_repo = v;
            }
        }
        if let Ok(v) = std::env::var("TASK_BACKUP_ORG_REPO_PREFIX") {
            if !v.is_empty() {
                engine.org_prefix = v;
            }
        }
        engine
    }

    /// Override the remote (tests push to a local directory base).
    #[must_use]
    pub fn with_remote(mut self, remote: Option<RemoteConfig>) -> Self {
        self.remote = remote;
        self
    }

    /// True when a push target is configured.
    #[must_use]
    pub fn has_remote(&self) -> bool {
        self.remote.is_some()
    }

    fn gitstate(&self) -> PathBuf {
        self.data_root.join(".gitstate")
    }

    /// Patterns every snapshot repo excludes: sqlite WAL/SHM/journal
    /// sidecars. They are transient runtime state — the cycle
    /// checkpoints (`TRUNCATE`) right before committing, so the main
    /// db file is complete; committing a *live* sidecar both races
    /// `git add` ("unstable object source data") and would shadow a
    /// restored db with stale WAL content.
    fn sidecar_excludes() -> [String; 3] {
        [
            "*.sqlite-wal".into(),
            "*.sqlite-shm".into(),
            "*.sqlite-journal".into(),
        ]
    }

    /// The full-state repo over the whole data root.
    #[must_use]
    pub fn full_spec(&self) -> RepoSpec {
        let mut excludes = vec![".gitstate/".to_owned(), "lost+found/".to_owned()];
        excludes.extend(Self::sidecar_excludes());
        RepoSpec {
            git_dir: self.gitstate().join("full.git"),
            work_tree: self.data_root.clone(),
            name: self.full_repo.clone(),
            excludes,
            force_ignored: true,
        }
    }

    /// One repo per org dir under `<data_root>/orgs/`, sorted by
    /// slug for deterministic reports.
    pub fn org_specs(&self) -> Result<Vec<RepoSpec>, EngineError> {
        let orgs_dir = self.data_root.join("orgs");
        let mut specs = Vec::new();
        let entries = match std::fs::read_dir(&orgs_dir) {
            Ok(e) => e,
            // No orgs dir yet — nothing to snapshot per-org.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(specs),
            Err(e) => return Err(e.into()),
        };
        for entry in entries {
            let entry = entry?;
            if !entry.path().is_dir() {
                continue;
            }
            let Some(slug) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            specs.push(RepoSpec {
                git_dir: self.gitstate().join("orgs").join(format!("{slug}.git")),
                work_tree: entry.path(),
                name: format!("{}{slug}", self.org_prefix),
                excludes: Self::sidecar_excludes().into(),
                // Per-org repos respect the org's own .gitignore —
                // the user's vault-centric policy stands there.
                force_ignored: false,
            });
        }
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(specs)
    }

    /// Every repo a cycle touches: org repos first, full repo last
    /// (same order as the script).
    pub fn specs(&self) -> Result<Vec<RepoSpec>, EngineError> {
        let mut specs = self.org_specs()?;
        specs.push(self.full_spec());
        Ok(specs)
    }

    // ── git plumbing ──────────────────────────────────────────────

    /// Run `git` against a repo spec, capturing output. Errors carry
    /// scrubbed stderr.
    async fn git(&self, spec: &RepoSpec, args: &[&str]) -> Result<String, EngineError> {
        let out = tokio::process::Command::new("git")
            .arg("--git-dir")
            .arg(&spec.git_dir)
            .arg("--work-tree")
            .arg(&spec.work_tree)
            .args(args)
            .output()
            .await
            .map_err(|source| EngineError::Spawn { bin: "git", source })?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            Err(EngineError::Git {
                verb: args.first().map_or("?", |s| s).to_owned(),
                detail: self.scrub(&String::from_utf8_lossy(&out.stderr)),
            })
        }
    }

    /// Replace the remote token (if any) with `***` so it never
    /// reaches logs or wire errors.
    fn scrub(&self, s: &str) -> String {
        match &self.remote {
            Some(r) if !r.token.is_empty() => s.replace(&r.token, "***"),
            _ => s.trim().to_owned(),
        }
    }

    /// Make sure the detached git dir exists. **Never** re-inits an
    /// existing one — production org repos continue users'
    /// pre-existing vault histories. Excludes are (re)written every
    /// call so spec changes take effect.
    async fn ensure_repo(&self, spec: &RepoSpec) -> Result<(), EngineError> {
        if !spec.git_dir.is_dir() {
            if let Some(parent) = spec.git_dir.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            let out = tokio::process::Command::new("git")
                .arg("init")
                .arg("-q")
                .arg("--bare")
                .arg(&spec.git_dir)
                .output()
                .await
                .map_err(|source| EngineError::Spawn { bin: "git", source })?;
            if !out.status.success() {
                return Err(EngineError::Git {
                    verb: "init".into(),
                    detail: self.scrub(&String::from_utf8_lossy(&out.stderr)),
                });
            }
            // Same setup the script does: bare git dir used with an
            // external work tree, identity, branch main.
            self.git(spec, &["config", "core.bare", "false"]).await?;
            self.git(spec, &["config", "user.name", "task-backup"])
                .await?;
            self.git(spec, &["config", "user.email", "task-backup@noreply"])
                .await?;
            self.git(spec, &["symbolic-ref", "HEAD", "refs/heads/main"])
                .await?;
        }
        if !spec.excludes.is_empty() {
            let info = spec.git_dir.join("info");
            tokio::fs::create_dir_all(&info).await?;
            let mut body = spec.excludes.join("\n");
            body.push('\n');
            tokio::fs::write(info.join("exclude"), body).await?;
        }
        Ok(())
    }

    /// Force-add files excluded by in-worktree `.gitignore`s, minus
    /// volatile sqlite sidecars (`-wal`/`-shm`/`-journal` — excluded
    /// by design: the WAL-checkpointed main db is the consistent
    /// artifact). `git add -f` would also drag the sidecars in, so
    /// enumerate ignored files and filter.
    async fn force_add_ignored(&self, spec: &RepoSpec) -> Result<(), EngineError> {
        let out = self
            .git(
                spec,
                &[
                    "ls-files",
                    "--others",
                    "--ignored",
                    "--exclude-standard",
                    "-z",
                ],
            )
            .await?;
        let wanted: Vec<&str> = out
            .split('\0')
            .filter(|p| !p.is_empty())
            .filter(|p| {
                !p.ends_with("-wal")
                    && !p.ends_with("-shm")
                    && !p.ends_with("-journal")
                    && !p.contains(".gitstate/")
            })
            .collect();
        for chunk in wanted.chunks(64) {
            let mut args: Vec<&str> = vec!["add", "-f", "--"];
            args.extend(chunk);
            self.git(spec, &args).await?;
        }
        Ok(())
    }

    /// Index scrub before `add -A`:
    ///
    /// - **De-gitlink defense** — data migrated in from elsewhere
    ///   can carry stray `.git` dirs; once recorded as a gitlink
    ///   (mode 160000) that path's CONTENTS are invisible to this
    ///   repo forever. Un-record any.
    /// - **Sidecar un-track** — script-era commits versioned live
    ///   `*.sqlite-wal`/`-shm` files; they're excluded now (see
    ///   [`Self::sidecar_excludes`]) but `info/exclude` only covers
    ///   *untracked* paths, so drop tracked ones from the index.
    async fn scrub_index(&self, spec: &RepoSpec) -> Result<(), EngineError> {
        let listing = self.git(spec, &["ls-files", "-s"]).await?;
        for line in listing.lines() {
            let Some((meta, path)) = line.split_once('\t') else {
                continue;
            };
            let sidecar = path.ends_with(".sqlite-wal")
                || path.ends_with(".sqlite-shm")
                || path.ends_with(".sqlite-journal");
            if meta.starts_with("160000 ") || sidecar {
                // Failure here mirrors the script's `|| true` — a
                // racing delete is fine, `add -A` resolves it.
                let _ = self.git(spec, &["rm", "-q", "--cached", "--", path]).await;
            }
        }
        Ok(())
    }

    /// True when HEAD exists (the repo has at least one commit).
    async fn has_head(&self, spec: &RepoSpec) -> bool {
        self.git(spec, &["rev-parse", "-q", "--verify", "HEAD"])
            .await
            .is_ok()
    }

    /// Stage + commit one repo. Skips the commit when the tree is
    /// clean (and history exists). Never pushes.
    pub async fn commit(&self, spec: &RepoSpec, stamp: &str) -> RepoResult {
        let mut result = RepoResult {
            repo: spec.name.clone(),
            committed: String::new(),
            clean: false,
            pushed: false,
            error: String::new(),
        };
        if let Err(e) = self.commit_inner(spec, stamp, &mut result).await {
            result.error = e.to_string();
        }
        result
    }

    async fn commit_inner(
        &self,
        spec: &RepoSpec,
        stamp: &str,
        result: &mut RepoResult,
    ) -> Result<(), EngineError> {
        self.ensure_repo(spec).await?;
        self.scrub_index(spec).await?;
        // `add -A` with a short retry: a file mutating while git
        // hashes it fails with "unstable object source data". The
        // server gates new writes during the commit phase, but an
        // in-flight straggler (or a sqlite auto-checkpoint) can
        // still race the first attempt; by the retry it has
        // settled.
        let mut attempt = 0u32;
        loop {
            match self.git(spec, &["add", "-A"]).await {
                Ok(_) => break,
                Err(EngineError::Git { detail, .. })
                    if attempt < 3 && detail.contains("unstable object source data") =>
                {
                    attempt += 1;
                    tracing::debug!(repo = %spec.name, attempt, "unstable object during add — retrying");
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                }
                Err(e) => return Err(e),
            }
        }
        if spec.force_ignored {
            self.force_add_ignored(spec).await?;
        }
        let staged_clean = self
            .git(spec, &["diff", "--cached", "--quiet"])
            .await
            .is_ok();
        if staged_clean && self.has_head(spec).await {
            result.clean = true;
            return Ok(());
        }
        let message = format!("snapshot {stamp}");
        self.git(
            spec,
            &[
                // Belt-and-braces identity: pre-existing script-era
                // repos already carry it in config; fresh clones of
                // the git dir might not.
                "-c",
                "user.name=task-backup",
                "-c",
                "user.email=task-backup@noreply",
                "commit",
                "-q",
                "-m",
                &message,
            ],
        )
        .await?;
        result.committed = self
            .git(spec, &["rev-parse", "HEAD"])
            .await?
            .trim()
            .to_owned();
        Ok(())
    }

    /// Phase 1 of a cycle: commit every repo (org repos, then full).
    /// Run this under the server's write gate — it's local-only and
    /// fast. Returns the commit stamp + per-repo outcomes paired
    /// with their specs for [`Self::push_phase`].
    pub async fn commit_phase(&self) -> Result<(String, Vec<(RepoSpec, RepoResult)>), EngineError> {
        let stamp = chrono::Utc::now().format("%Y-%m-%dT%H:%M:%SZ").to_string();
        let mut out = Vec::new();
        for spec in self.specs()? {
            let result = self.commit(&spec, &stamp).await;
            out.push((spec, result));
        }
        Ok((stamp, out))
    }

    /// Phase 2 of a cycle: push every repo that didn't already fail.
    /// Run this *after* releasing the write gate — commits are
    /// immutable, so the slow network part needs no quiesce. No-op
    /// when no remote is configured.
    pub async fn push_phase(&self, repos: Vec<(RepoSpec, RepoResult)>) -> Vec<RepoResult> {
        let mut results = Vec::with_capacity(repos.len());
        for (spec, mut result) in repos {
            if self.remote.is_some() && result.error.is_empty() {
                match self.push(&spec, "main:main").await {
                    Ok(()) => result.pushed = true,
                    Err(e) => {
                        result.error = format!("push: {e}");
                    }
                }
            }
            results.push(result);
        }
        results
    }

    /// Full cycle without a write gate (tests, CLI-against-local).
    /// The server prefers `commit_phase` under its gate +
    /// `push_phase` after.
    pub async fn snapshot(&self) -> Result<SnapshotReport, EngineError> {
        let (stamp, repos) = self.commit_phase().await?;
        let repos = self.push_phase(repos).await;
        Ok(SnapshotReport { stamp, repos })
    }

    // ── push (THE storage-backend extension point) ───────────────

    /// Push one repo's `refspec` to the configured remote. This is
    /// the single function that knows how snapshot bytes leave the
    /// machine — a future Nextcloud/WebDAV/NAS backend replaces the
    /// body, nothing else.
    ///
    /// - `https://` base: Forgejo-style — auto-create the (private)
    ///   remote repo via the API on first push, then plain
    ///   fast-forward `git push` over https with token auth.
    ///   **Never** `--force`.
    /// - any other base: treated as a directory of bare repos
    ///   (local disk / NAS mount); auto-`git init --bare` then push.
    pub async fn push(&self, spec: &RepoSpec, refspec: &str) -> Result<(), EngineError> {
        let Some(remote) = &self.remote else {
            return Err(EngineError::Refused("no remote configured".into()));
        };
        let url = if remote.base.starts_with("https://") || remote.base.starts_with("http://") {
            self.ensure_forge_remote(remote, &spec.name).await;
            let (scheme, rest) = remote
                .base
                .split_once("://")
                .unwrap_or(("https", remote.base.as_str()));
            format!(
                "{scheme}://{}:{}@{rest}/{}.git",
                remote.user, remote.token, spec.name
            )
        } else {
            // Local/NAS directory base: a bare repo per name.
            let bare = Path::new(&remote.base).join(format!("{}.git", spec.name));
            if !bare.is_dir() {
                tokio::fs::create_dir_all(&bare).await?;
                let out = tokio::process::Command::new("git")
                    .args(["init", "-q", "--bare"])
                    .arg(&bare)
                    .output()
                    .await
                    .map_err(|source| EngineError::Spawn { bin: "git", source })?;
                if !out.status.success() {
                    return Err(EngineError::Git {
                        verb: "init remote".into(),
                        detail: self.scrub(&String::from_utf8_lossy(&out.stderr)),
                    });
                }
            }
            bare.display().to_string()
        };
        self.git(spec, &["push", "-q", &url, refspec]).await?;
        Ok(())
    }

    /// Auto-create the remote repo (private) via the Forgejo API on
    /// first push. Best-effort, like the script: failures fall
    /// through to the push, which reports loudly. Shells out to
    /// `curl` (present in the server runtime image).
    async fn ensure_forge_remote(&self, remote: &RemoteConfig, name: &str) {
        let Some((_scheme, rest)) = remote.base.split_once("://") else {
            return;
        };
        let Some((host, owner)) = rest.split_once('/') else {
            return;
        };
        let api = format!("https://{host}/api/v1");
        let auth = format!("Authorization: token {}", remote.token);
        let exists = tokio::process::Command::new("curl")
            .args(["-fsS", "-o", "/dev/null", "-H", &auth])
            .arg(format!("{api}/repos/{owner}/{name}"))
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false);
        if exists {
            return;
        }
        let body = format!(
            "{{\"name\":\"{name}\",\"private\":true,\"description\":\"Task data snapshots (automated)\"}}"
        );
        for endpoint in [
            format!("{api}/orgs/{owner}/repos"),
            format!("{api}/user/repos"),
        ] {
            let created = tokio::process::Command::new("curl")
                .args([
                    "-fsS",
                    "-o",
                    "/dev/null",
                    "-H",
                    &auth,
                    "-H",
                    "Content-Type: application/json",
                    "-d",
                    &body,
                ])
                .arg(&endpoint)
                .output()
                .await
                .map(|o| o.status.success())
                .unwrap_or(false);
            if created {
                return;
            }
        }
        tracing::warn!(
            repo = name,
            "could not auto-create remote repo — push may fail"
        );
    }

    // ── full-repo verbs ───────────────────────────────────────────

    /// Recent commits on the full-state repo, newest first.
    pub async fn log(&self, limit: u32) -> Result<Vec<SnapshotLogEntry>, EngineError> {
        let spec = self.full_spec();
        if !spec.git_dir.is_dir() {
            return Ok(Vec::new());
        }
        if !self.has_head(&spec).await {
            return Ok(Vec::new());
        }
        let n = if limit == 0 { 20 } else { limit }.to_string();
        let raw = self
            .git(&spec, &["log", "-n", &n, "--pretty=format:%H%x1f%cI%x1f%s"])
            .await?;
        Ok(raw
            .lines()
            .filter_map(|line| {
                let mut parts = line.splitn(3, '\u{1f}');
                Some(SnapshotLogEntry {
                    commit: parts.next()?.to_owned(),
                    timestamp: parts.next()?.to_owned(),
                    message: parts.next().unwrap_or_default().to_owned(),
                })
            })
            .collect())
    }

    /// Create `name` at the full repo's HEAD and push it when a
    /// remote is configured — the "branch the data" verb.
    pub async fn branch(&self, name: &str) -> Result<BranchResult, EngineError> {
        let spec = self.full_spec();
        if !spec.git_dir.is_dir() || !self.has_head(&spec).await {
            return Err(EngineError::Refused(
                "full-state repo has no history yet — run a snapshot first".into(),
            ));
        }
        self.git(&spec, &["branch", name, "HEAD"]).await?;
        let commit = self
            .git(&spec, &["rev-parse", "HEAD"])
            .await?
            .trim()
            .to_owned();
        let mut pushed = false;
        if self.remote.is_some() {
            let refspec = format!("refs/heads/{name}:refs/heads/{name}");
            self.push(&spec, &refspec).await?;
            pushed = true;
        }
        Ok(BranchResult {
            name: name.to_owned(),
            commit,
            pushed,
        })
    }

    /// True when the full-state work tree differs from HEAD
    /// (including untracked files). A repo with no history counts
    /// as dirty.
    pub async fn full_tree_dirty(&self) -> Result<bool, EngineError> {
        let spec = self.full_spec();
        if !spec.git_dir.is_dir() || !self.has_head(&spec).await {
            return Ok(true);
        }
        // ensure_repo keeps info/exclude current so status doesn't
        // report `.gitstate/` itself.
        self.ensure_repo(&spec).await?;
        let out = self.git(&spec, &["status", "--porcelain"]).await?;
        Ok(!out.trim().is_empty())
    }

    /// Restore the data root to `commit` via the full repo:
    /// `git checkout <commit> -- .`, then delete sqlite WAL/SHM
    /// sidecars so a restarted server doesn't replay a stale WAL
    /// over the restored database files. Returns the resolved sha.
    ///
    /// The caller (server) decides when to exit the process; this
    /// function only mutates the work tree.
    pub async fn restore_checkout(&self, commit: &str) -> Result<String, EngineError> {
        let spec = self.full_spec();
        if !spec.git_dir.is_dir() {
            return Err(EngineError::Refused(
                "full-state repo does not exist — nothing to restore from".into(),
            ));
        }
        let probe = format!("{commit}^{{commit}}");
        let sha = self
            .git(&spec, &["rev-parse", "--verify", &probe])
            .await
            .map_err(|_| EngineError::Refused(format!("unknown commit `{commit}`")))?
            .trim()
            .to_owned();
        self.git(&spec, &["checkout", &sha, "--", "."]).await?;
        self.clean_sqlite_sidecars()?;
        Ok(sha)
    }

    /// Delete `*.sqlite-wal` / `*.sqlite-shm` / `*.sqlite-journal`
    /// under the data root (skipping `.gitstate/`). The committed
    /// snapshots are taken right after `PRAGMA wal_checkpoint
    /// (TRUNCATE)`, so the main db files are complete; a leftover
    /// live WAL would otherwise be replayed over the restored db on
    /// next open.
    fn clean_sqlite_sidecars(&self) -> Result<(), EngineError> {
        for entry in walkdir::WalkDir::new(&self.data_root)
            .into_iter()
            .filter_entry(|e| e.file_name() != ".gitstate")
            .filter_map(Result::ok)
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if name.ends_with(".sqlite-wal")
                || name.ends_with(".sqlite-shm")
                || name.ends_with(".sqlite-journal")
            {
                if let Err(e) = std::fs::remove_file(entry.path()) {
                    tracing::warn!(path = %entry.path().display(), error = %e, "could not remove sqlite sidecar");
                }
            }
        }
        Ok(())
    }
}
