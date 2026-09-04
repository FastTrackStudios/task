//! A wiki that is a working copy of a path inside a git repository
//! (`wiki.source.repo`, `wiki.source.sync`, `wiki.source.editable`).
//!
//! The repository is the authority and the wiki root is a **working
//! copy** of one subtree of it: the live, collaboratively edited version
//! that every save lands in, kept fresh from upstream on a schedule, and
//! pushed back — all of it, as one commit on one branch — when the
//! people editing it say they are ready. Everything here is a pure
//! function over the `git` binary and two directories:
//!
//! - the **clone**, at `<org>/wikis/.repos/<slug>` — a hidden sibling
//!   of the wikis, so the org's wiki listing (which only accepts
//!   slug-shaped directory names) never mistakes it for a wiki;
//! - the **wiki root**, `<org>/wikis/<slug>`, where the files under
//!   `source.path` are exported to, byte for byte, and then edited.
//!
//! What the export last wrote — every path and the hash of its content
//! at `source.commit` — is remembered in `_state/repo_mirror.json`.
//! That manifest is the **base**, and it is what makes two questions
//! answerable without tracking anything by hand: "what has been changed
//! here?" is the wiki root against the base ([`local_changes`]), and
//! "may this sync overwrite this page?" is whether the page still
//! matches the base. A sync updates a page only when the working copy
//! has not touched it; a page changed on both sides is kept as it is
//! here and named as a conflict, never chosen for anyone. A file the
//! repository dropped is deleted only if it is unchanged here; a file
//! the wiki has that the repository never had — its own bookkeeping
//! under `_state/`, the scaffold `bootstrap` writes — is not the
//! repository's and is neither a change nor something a sync removes.
//!
//! Failure leaves the pages exactly as they were. A fetch against a
//! dead URL sets `source.last_error` and changes no page — stale
//! content is served, and marked as stale, rather than served as
//! current or replaced with nothing (`wiki.source.sync`).
//!
//! No `git2`: the server image already carries `git` for the
//! snapshot engine, the software-root tests drive the same binary,
//! and a shell-out is the one way to be sure a clone here behaves
//! like a clone anywhere.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, MutexGuard};

use wiki_proto::config::RepoSource;
use wiki_proto::error::WikiError;
use wiki_proto::paths;
use wiki_proto::service::registry::{ChangeKind, LocalChange, LocalChanges};

use crate::backend::sha256_hex;

/// Directory under `<org>/wikis/` that holds every clone. Hidden, and
/// not slug-shaped, so it is never listed as a wiki.
pub const REPOS_DIR: &str = ".repos";

/// The manifest of what the export last wrote, inside `_state/`.
const MIRROR_JSON: &str = "repo_mirror.json";

/// What one sync did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncOutcome {
    /// Full sha the wiki now reflects.
    pub commit: String,
    /// Files written or deleted in the wiki root.
    pub changed: u32,
    /// `changed == 0` — the repository had nothing new for us.
    pub unchanged: bool,
    /// Pages the sync left alone because both sides changed them.
    pub conflicts: Vec<String>,
    /// Whether the pending push was found merged by this sync.
    pub merged_pending: bool,
}

/// What a push put on the remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Landing {
    /// The branch pushed to `origin`.
    pub branch: String,
    /// Full sha of the commit on it.
    pub commit: String,
    /// URL of a pull request opened for it, when a forge client did.
    /// Nothing in this module opens one; the caller may, and records
    /// it here.
    pub pull_request: Option<String>,
}

// ────────────────────── Forge hook ──────────────────────

/// The forge half of pushing a working copy (`wiki.source.editable`).
///
/// This module pushes a branch; turning that branch into a pull request
/// needs a forge client, which lives above this crate. The server
/// supplies one through this hook; the default opens nothing, so a wiki
/// over a repository with no forge configured still lands as a pushed
/// branch a person can merge.
pub trait Lander: Send + Sync + 'static {
    /// Who the pushing Editor is on this repository's forge, and the
    /// credential the push and the pull request are made with — so the
    /// repository's history names the person, not the deployment
    /// (`wiki.source.editable`). Called before anything is pushed: an
    /// `Err` (no linked forge account) refuses the push with nothing
    /// sent. The default lands as the deployment: the editor's account
    /// id as committer, `origin` as the push target.
    fn identity_for(&self, source: &RepoSource, editor: &str) -> Result<ForgeIdentity, WikiError> {
        let _ = source;
        Ok(ForgeIdentity::deployment(editor))
    }

    /// Open a pull request for `landing`, as `identity`. `Ok(None)` when
    /// no forge client applies to this repository; the branch is still
    /// pushed.
    fn open_pull_request(
        &self,
        source: &RepoSource,
        landing: &Landing,
        title: &str,
        body: &str,
        identity: &ForgeIdentity,
    ) -> Result<Option<String>, WikiError>;
}

/// The pushing Editor as the forge knows them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ForgeIdentity {
    /// How a message and a log name them (`@octocat`, or the account id
    /// when the deployment pushes).
    pub display: String,
    /// Author and committer of the pushed commit.
    pub committer_name: String,
    pub committer_email: String,
    /// The person's own forge access token, when they have linked one.
    /// `None` means the deployment pushes and opens the request itself.
    pub token: Option<String>,
    /// Where the push goes (see [`Pusher::push_url`]).
    pub push_url: Option<String>,
}

impl ForgeIdentity {
    /// The deployment pushes on the editor's behalf: their account id
    /// is the committer, `origin` the target.
    #[must_use]
    pub fn deployment(editor: &str) -> Self {
        Self {
            display: editor.to_owned(),
            committer_name: editor.to_owned(),
            committer_email: format!("{editor}@task.invalid"),
            token: None,
            push_url: None,
        }
    }

    /// The push half of this identity.
    #[must_use]
    pub fn pusher(&self) -> Pusher {
        Pusher {
            committer_name: self.committer_name.clone(),
            committer_email: self.committer_email.clone(),
            push_url: self.push_url.clone(),
        }
    }
}

/// The default [`Lander`]: pushes only, as the deployment.
pub struct NoForge;

impl Lander for NoForge {
    fn open_pull_request(
        &self,
        _source: &RepoSource,
        _landing: &Landing,
        _title: &str,
        _body: &str,
        _identity: &ForgeIdentity,
    ) -> Result<Option<String>, WikiError> {
        Ok(None)
    }
}

/// Who pushes, and with what.
///
/// `push_url` is that person's own credential for the push
/// (`https://x-access-token:<token>@github.com/o/r.git`): used for this
/// one push and never written into the clone's remote config, so
/// nothing on disk outlives the call. `None` pushes to `origin` as the
/// clone was made, for a repository that needs no credential (a
/// `file://` source) or one the deployment itself is allowed to push.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pusher {
    pub committer_name: String,
    pub committer_email: String,
    pub push_url: Option<String>,
}

impl Pusher {
    /// Push to `origin`, committing as `name <email>`.
    #[must_use]
    pub fn origin(name: &str, email: &str) -> Self {
        Self {
            committer_name: name.to_owned(),
            committer_email: email.to_owned(),
            push_url: None,
        }
    }
}

// ────────────────────── Plumbing ──────────────────────

/// Where a wiki's clone lives: `<wikis_dir>/.repos/<slug>`.
#[must_use]
pub fn clone_dir(wikis_dir: &Path, slug: &str) -> PathBuf {
    wikis_dir.join(REPOS_DIR).join(slug)
}

/// Whether `commit` is reachable from the commit the working copy is
/// based on — that is, whether a pushed branch has since been merged
/// upstream (`wiki.source.editable`). Answered from the clone as it
/// stands; a caller wanting the latest word syncs first.
///
/// `false` for a clone that does not exist yet or a commit git has
/// never heard of: neither is an error, both mean "not merged".
#[must_use]
pub fn contains_commit(wikis_dir: &Path, slug: &str, source: &RepoSource, commit: &str) -> bool {
    if commit.is_empty() || source.commit.is_empty() {
        return false;
    }
    let dir = clone_dir(wikis_dir, slug);
    if !dir.is_dir() {
        return false;
    }
    let _guard = git_lock();
    is_ancestor(&dir, commit, &source.commit)
}

fn is_ancestor(clone: &Path, commit: &str, of: &str) -> bool {
    !commit.is_empty() && git(clone, &["merge-base", "--is-ancestor", commit, of]).is_ok()
}

/// One process-wide lock around every git operation on any clone.
///
/// A sync and a push on the same clone both move its HEAD, and the
/// server's periodic sync can coincide with a person's refresh. Serial
/// is correct and cheap; these are seconds apart at most.
fn git_lock() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Run `git` in `dir`, returning stdout, or the stderr as the error.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        // Never prompt: a URL that wants a password fails instead of
        // hanging a server thread.
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("running git {args:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_owned();
        Err(if err.is_empty() {
            format!("git {args:?} failed with {}", out.status)
        } else {
            err
        })
    }
}

/// Whether `git` is on this machine's PATH.
#[must_use]
pub fn git_on_path() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Clone if absent, fetch, and resolve the commit `source` follows.
///
/// Returns the clone's directory and the full sha of `origin/<branch>`
/// (or the remote's HEAD when the branch is empty).
fn fetch(wikis_dir: &Path, slug: &str, source: &RepoSource) -> Result<(PathBuf, String), String> {
    let clone = clone_dir(wikis_dir, slug);
    if !clone.join(".git").exists() {
        let parent = clone.parent().unwrap_or(wikis_dir);
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        // `--no-checkout`: the working tree is populated at the commit
        // we choose, not at whatever the remote's HEAD happens to be.
        git(
            parent,
            &[
                "clone",
                "--no-checkout",
                &source.url,
                clone.file_name().and_then(|f| f.to_str()).unwrap_or(slug),
            ],
        )?;
    } else {
        // A URL edit in the config must take effect on the next sync
        // rather than fetching the old remote forever.
        git(&clone, &["remote", "set-url", "origin", &source.url])?;
    }
    git(&clone, &["fetch", "--prune", "origin"])?;
    let target = if source.branch.trim().is_empty() {
        // Ask the remote which branch is its HEAD, every time: a repo
        // that switched from `master` to `main` should be followed.
        git(&clone, &["remote", "set-head", "origin", "--auto"])?;
        "refs/remotes/origin/HEAD".to_owned()
    } else {
        format!("refs/remotes/origin/{}", source.branch.trim())
    };
    let sha = git(
        &clone,
        &["rev-parse", "--verify", &format!("{target}^{{commit}}")],
    )?
    .trim()
    .to_owned();
    Ok((clone, sha))
}

/// The subtree of the repository the wiki is, trimmed of slashes.
fn subpath_of(source: &RepoSource) -> &str {
    source.path.trim().trim_matches('/')
}

// ────────────────────── The base ──────────────────────

/// What the export last wrote, and what it left in conflict.
///
/// `files` is the base: every wiki-relative path exported at
/// `source.commit` with the sha-256 of its content there. `conflicts`
/// remembers, per path, the local content a conflict was recorded
/// over, so the conflict clears the moment that content changes — the
/// person has looked — or comes to match upstream.
#[derive(Debug, Default, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Manifest {
    #[serde(default)]
    files: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    conflicts: BTreeMap<String, String>,
}

fn manifest_path(wiki_root: &Path) -> PathBuf {
    wiki_root.join(paths::STATE_DIR).join(MIRROR_JSON)
}

/// Read the manifest. A manifest written before content hashes were
/// kept (a bare list of paths) reads as a base of unknown content, so
/// the next sync treats those pages as untouched and updates them —
/// exactly what that older mirror would have done.
fn read_manifest(wiki_root: &Path) -> Manifest {
    let Ok(bytes) = std::fs::read(manifest_path(wiki_root)) else {
        return Manifest::default();
    };
    if let Ok(m) = serde_json::from_slice::<Manifest>(&bytes) {
        return m;
    }
    let legacy: BTreeSet<String> = serde_json::from_slice(&bytes).unwrap_or_default();
    Manifest {
        files: legacy.into_iter().map(|p| (p, String::new())).collect(),
        conflicts: BTreeMap::new(),
    }
}

fn write_manifest(wiki_root: &Path, manifest: &Manifest) -> Result<(), String> {
    let path = manifest_path(wiki_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    let json = serde_json::to_vec_pretty(manifest).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))
}

/// The sha-256 of a file's content, or `None` when it does not exist.
fn hash_of(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(sha256_hex(&bytes))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Subtrees that are the wiki's own and never the repository's: agent
/// state, the raw layer, extracted media, and anything hidden.
fn is_wiki_bookkeeping(rel: &str) -> bool {
    rel.split('/').any(|seg| seg.starts_with('.'))
        || rel == paths::STATE_DIR
        || rel.starts_with(&format!("{}/", paths::STATE_DIR))
        || rel == paths::RAW_DIR
        || rel.starts_with(&format!("{}/", paths::RAW_DIR))
        || rel == paths::MEDIA_DIR
        || rel.starts_with(&format!("{}/", paths::MEDIA_DIR))
}

/// The files `bootstrap` scaffolds for every wiki. Not a change when
/// the repository never had them; an edit to one the repository *does*
/// have is a change like any other.
fn is_scaffold(rel: &str) -> bool {
    matches!(
        rel,
        paths::PURPOSE_MD | paths::SCHEMA_MD | paths::INDEX_MD | paths::LOG_MD | paths::OVERVIEW_MD
    )
}

/// Every non-bookkeeping file under the wiki root, wiki-relative with
/// `/` separators, sorted.
fn working_files(wiki_root: &Path) -> Result<BTreeSet<String>, String> {
    let mut out = BTreeSet::new();
    let walk = walkdir::WalkDir::new(wiki_root)
        .min_depth(1)
        .into_iter()
        .filter_entry(|e| {
            !e.file_name()
                .to_str()
                .is_some_and(|n| n.starts_with('.') || (e.depth() == 1 && is_wiki_bookkeeping(n)))
        });
    for entry in walk {
        let entry = entry.map_err(|e| e.to_string())?;
        if !entry.file_type().is_file() {
            continue;
        }
        let rel = entry
            .path()
            .strip_prefix(wiki_root)
            .map_err(|e| e.to_string())?
            .components()
            .map(|c| c.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/");
        if !is_wiki_bookkeeping(&rel) {
            out.insert(rel);
        }
    }
    Ok(out)
}

/// The conflicts in `manifest` that still stand: the page is still the
/// content the conflict was recorded over (an empty hash meaning "not
/// here"), and still not what upstream has. Anything else has been
/// dealt with.
fn standing_conflicts(
    wiki_root: &Path,
    manifest: &Manifest,
) -> Result<BTreeMap<String, String>, String> {
    let mut out = BTreeMap::new();
    for (rel, seen) in &manifest.conflicts {
        let local = hash_of(&wiki_root.join(rel))?;
        let matches_upstream = manifest.files.get(rel) == local.as_ref();
        if local.unwrap_or_default() == *seen && !matches_upstream {
            out.insert(rel.clone(), seen.clone());
        }
    }
    Ok(out)
}

/// What the working copy holds that the base does not
/// (`wiki.source.editable`): the wiki root against the manifest, path
/// by path. Derived every time, so an edit through any door counts
/// and a merge upstream makes the list empty on its own.
pub fn local_changes(wiki_root: &Path, source: &RepoSource) -> Result<LocalChanges, WikiError> {
    local_changes_inner(wiki_root, source).map_err(WikiError::Io)
}

fn local_changes_inner(wiki_root: &Path, source: &RepoSource) -> Result<LocalChanges, String> {
    let manifest = read_manifest(wiki_root);
    let mut changes = Vec::new();
    for rel in working_files(wiki_root)? {
        match manifest.files.get(&rel) {
            Some(base) => {
                if hash_of(&wiki_root.join(&rel))?.as_deref() != Some(base.as_str()) {
                    changes.push(LocalChange {
                        path: rel,
                        kind: ChangeKind::Modified,
                    });
                }
            }
            None if is_scaffold(&rel) => {}
            None => changes.push(LocalChange {
                path: rel,
                kind: ChangeKind::Added,
            }),
        }
    }
    for rel in manifest.files.keys() {
        if !wiki_root.join(rel).is_file() {
            changes.push(LocalChange {
                path: rel.clone(),
                kind: ChangeKind::Deleted,
            });
        }
    }
    changes.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(LocalChanges {
        base_commit: source.commit.clone(),
        changes,
        pending: source.pending.clone(),
        conflicts: standing_conflicts(wiki_root, &manifest)?
            .into_keys()
            .collect(),
    })
}

// ────────────────────── Sync ──────────────────────

/// Bring the working copy up to date with its repository, keeping
/// every local edit.
///
/// Clone if absent, fetch, check out the followed commit, and export
/// `source.path` into `wiki_root` — page by page, and only onto pages
/// the working copy has not changed. A page changed here and upstream
/// both is left as it is here and recorded in `source.conflicts`. On
/// success `source.commit` and `source.fetched_at` are set,
/// `source.last_error` cleared, and `source.pending` cleared if the
/// followed branch now contains its commit; on any failure
/// `source.last_error` says why, the pages are untouched, and the error
/// is returned. The caller persists `source` either way — that is what
/// makes a stale wiki *say* it is stale.
///
/// The slug is the wiki root's directory name, which is also the clone's.
pub fn sync(
    wikis_dir: &Path,
    wiki_root: &Path,
    source: &mut RepoSource,
) -> Result<SyncOutcome, WikiError> {
    let slug = wiki_root
        .file_name()
        .and_then(|f| f.to_str())
        .ok_or_else(|| WikiError::Backend(format!("{}: no slug", wiki_root.display())))?;
    let _held = git_lock();
    match sync_inner(wikis_dir, wiki_root, slug, source) {
        Ok(outcome) => {
            source.commit = outcome.commit.clone();
            source.fetched_at = chrono::Utc::now().to_rfc3339();
            source.last_error.clear();
            source.conflicts = outcome.conflicts.clone();
            if outcome.merged_pending {
                source.pending = None;
            }
            Ok(outcome)
        }
        Err(reason) => {
            source.last_error = reason.clone();
            Err(WikiError::Io(format!("sync {}: {reason}", source.url)))
        }
    }
}

fn sync_inner(
    wikis_dir: &Path,
    wiki_root: &Path,
    slug: &str,
    source: &RepoSource,
) -> Result<SyncOutcome, String> {
    let (clone, sha) = fetch(wikis_dir, slug, source)?;
    // `changed` is computed even when the commit is the one already
    // reflected: a manifest can be behind its pages.
    git(&clone, &["checkout", "--force", "--detach", &sha])?;

    let subpath = subpath_of(source);
    let mut ls = vec!["ls-files", "-z", "--"];
    if !subpath.is_empty() {
        ls.push(subpath);
    }
    let listed = git(&clone, &ls)?;
    let tracked: BTreeSet<String> = listed
        .split('\0')
        .filter(|s| !s.is_empty())
        .map(|repo_rel| {
            if subpath.is_empty() {
                repo_rel.to_owned()
            } else {
                repo_rel
                    .strip_prefix(subpath)
                    .and_then(|r| r.strip_prefix('/'))
                    .unwrap_or(repo_rel)
                    .to_owned()
            }
        })
        // Nothing may land in the wiki's own bookkeeping.
        .filter(|rel| !is_wiki_bookkeeping(rel))
        .collect();
    if tracked.is_empty() && !subpath.is_empty() && !clone.join(subpath).is_dir() {
        return Err(format!(
            "`{subpath}` is not a directory in the repository at {sha}"
        ));
    }

    let previous = read_manifest(wiki_root);
    let mut next = Manifest {
        files: BTreeMap::new(),
        // Conflicts already dealt with go; the rest carry over and are
        // re-judged below against the new base.
        conflicts: standing_conflicts(wiki_root, &previous)?,
    };
    let mut changed: u32 = 0;
    let src_base = if subpath.is_empty() {
        clone.clone()
    } else {
        clone.join(subpath)
    };
    for rel in &tracked {
        let from = src_base.join(rel);
        let to = wiki_root.join(rel);
        let bytes = std::fs::read(&from).map_err(|e| format!("{}: {e}", from.display()))?;
        let upstream = sha256_hex(&bytes);
        let base = previous.files.get(rel).map(String::as_str);
        let local = hash_of(&to)?;
        next.files.insert(rel.clone(), upstream.clone());
        match (base, local.as_deref()) {
            // Already what upstream has.
            (_, Some(l)) if l == upstream => {
                next.conflicts.remove(rel);
            }
            // Untouched here (or a base of unknown content, see
            // `read_manifest`): take upstream.
            (Some(b), Some(l)) if b == l || b.is_empty() => {
                write_page(&to, &bytes)?;
                changed += 1;
            }
            // Changed here, not upstream: keep ours.
            (Some(b), Some(_)) if b == upstream => {}
            // Deleted here; upstream unchanged: stays deleted.
            (Some(b), None) if b == upstream || b.is_empty() => {}
            // New upstream, nothing here yet: take it.
            (None, None) => {
                write_page(&to, &bytes)?;
                changed += 1;
            }
            // Changed on both sides — a page edited here and moved
            // upstream, deleted here and edited upstream, or added on
            // both sides with different content. Keep ours, and say so.
            (_, local) => {
                next.conflicts
                    .insert(rel.clone(), local.map(str::to_owned).unwrap_or_default());
            }
        }
    }

    // Delete what the export wrote last time and the repository no
    // longer has — if it is unchanged here. A page edited here that
    // upstream dropped is kept and named a conflict. Only paths the
    // export wrote: the wiki's scaffold and state are not the
    // repository's to remove.
    for (stale, base) in &previous.files {
        if tracked.contains(stale) {
            continue;
        }
        let path = wiki_root.join(stale);
        match hash_of(&path)? {
            None => {}
            Some(local) if &local == base || base.is_empty() => {
                std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                changed += 1;
                prune_empty_parents(wiki_root, &path);
            }
            Some(local) => {
                next.conflicts.insert(stale.clone(), local);
            }
        }
    }
    write_manifest(wiki_root, &next)?;

    let merged_pending = source
        .pending
        .as_ref()
        .is_some_and(|p| is_ancestor(&clone, &p.commit, &sha));
    Ok(SyncOutcome {
        commit: sha,
        changed,
        unchanged: changed == 0,
        conflicts: next.conflicts.into_keys().collect(),
        merged_pending,
    })
}

fn write_page(to: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = to.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    std::fs::write(to, bytes).map_err(|e| format!("{}: {e}", to.display()))
}

/// Remove now-empty directories between a deleted file and the root,
/// so a folder the repository dropped does not linger as an empty
/// page group. Stops at the first non-empty one, and never removes
/// the root.
fn prune_empty_parents(wiki_root: &Path, deleted: &Path) {
    let mut dir = deleted.parent();
    while let Some(d) = dir {
        if d == wiki_root || !d.starts_with(wiki_root) {
            break;
        }
        if std::fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
}

// ────────────────────── Push ──────────────────────

/// Send the working copy's changes to the repository as one commit on
/// `branch` (`wiki.source.editable`).
///
/// The branch is (re)created from `source.commit` — the base the
/// working copy sits on, so the diff is exactly [`local_changes`] —
/// and force-pushed: a second push while the first is unmerged
/// rewrites the same branch with the fuller change set. `changes` is
/// what the caller derived and showed; it is applied from the wiki
/// root as it stands now. Author and committer are both the pusher,
/// because the repository's history must say who pushed.
///
/// Nothing here opens a pull request; see [`Lander`].
pub fn push_working_copy(
    wikis_dir: &Path,
    wiki_root: &Path,
    slug: &str,
    source: &RepoSource,
    branch: &str,
    changes: &[LocalChange],
    message: &str,
    pusher: &Pusher,
) -> Result<Landing, WikiError> {
    let _held = git_lock();
    let run = || -> Result<Landing, String> {
        if changes.is_empty() {
            return Err("nothing to push".into());
        }
        if source.commit.trim().is_empty() {
            return Err("the wiki has never synced; there is no base to push from".into());
        }
        let mut edits = Vec::with_capacity(changes.len());
        for change in changes {
            let content = match change.kind {
                ChangeKind::Deleted => None,
                ChangeKind::Added | ChangeKind::Modified => {
                    let path = wiki_root.join(&change.path);
                    Some(std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?)
                }
            };
            edits.push((change.path.clone(), content));
        }
        let (clone, _tip) = fetch(wikis_dir, slug, source)?;
        let committer = (
            pusher.committer_name.as_str(),
            pusher.committer_email.as_str(),
        );
        commit_and_push(
            &clone,
            subpath_of(source),
            source.commit.trim(),
            branch,
            &edits,
            message,
            committer,
            committer,
            pusher.push_url.as_deref(),
            true,
        )
    };
    run().map_err(|reason| {
        WikiError::Io(scrub(pusher, &format!("push to {}: {reason}", source.url)))
    })
}

/// Commit `changes` onto a new branch in the clone and push it.
///
/// `changes` are wiki-relative paths (repository-relative under
/// `source.path`) with the new content, or `None` to delete. The branch
/// is created from `source.commit`, or from the followed branch's tip
/// when the wiki has never synced. Author and committer are both
/// `author` unless the pusher names a committer. Kept for a caller that
/// lands a prepared change set rather than the working copy — the push
/// of a working copy is [`push_working_copy`].
pub fn land(
    wikis_dir: &Path,
    slug: &str,
    source: &RepoSource,
    branch: &str,
    changes: &[(String, Option<String>)],
    message: &str,
    author: (&str, &str),
    pusher: &Pusher,
) -> Result<Landing, WikiError> {
    let _held = git_lock();
    let run = || -> Result<Landing, String> {
        if changes.is_empty() {
            return Err("nothing to land".into());
        }
        let (clone, tip) = fetch(wikis_dir, slug, source)?;
        let base = if source.commit.trim().is_empty() {
            tip
        } else {
            source.commit.trim().to_owned()
        };
        let edits: Vec<(String, Option<Vec<u8>>)> = changes
            .iter()
            .map(|(p, c)| (p.clone(), c.as_ref().map(|s| s.as_bytes().to_vec())))
            .collect();
        // A pusher with no name falls back to the author, so a caller
        // that has not resolved an identity still lands truthfully.
        let committer = (
            non_empty(&pusher.committer_name).unwrap_or(author.0),
            non_empty(&pusher.committer_email).unwrap_or(author.1),
        );
        commit_and_push(
            &clone,
            subpath_of(source),
            &base,
            branch,
            &edits,
            message,
            author,
            committer,
            pusher.push_url.as_deref(),
            false,
        )
    };
    run().map_err(|reason| {
        WikiError::Io(scrub(pusher, &format!("land on {}: {reason}", source.url)))
    })
}

fn non_empty(s: &str) -> Option<&str> {
    let s = s.trim();
    (!s.is_empty()).then_some(s)
}

/// A push URL carries the pusher's credential; git echoes the URL in
/// its errors, so the reason is scrubbed before it can reach a log
/// line or a person.
fn scrub(pusher: &Pusher, reason: &str) -> String {
    pusher
        .push_url
        .as_deref()
        .map_or(reason.to_owned(), |url| reason.replace(url, "<push url>"))
}

#[allow(clippy::too_many_arguments)]
fn commit_and_push(
    clone: &Path,
    subpath: &str,
    base: &str,
    branch: &str,
    changes: &[(String, Option<Vec<u8>>)],
    message: &str,
    (author_name, author_email): (&str, &str),
    (committer_name, committer_email): (&str, &str),
    push_url: Option<&str>,
    force: bool,
) -> Result<Landing, String> {
    let branch = branch.trim();
    if branch.is_empty() {
        return Err("a push needs a branch name".into());
    }
    git(clone, &["checkout", "--force", "-B", branch, base])?;
    let base_dir = if subpath.is_empty() {
        clone.to_path_buf()
    } else {
        clone.join(subpath)
    };
    for (rel, content) in changes {
        let rel = rel.trim_start_matches('/');
        if rel.is_empty() || rel.split('/').any(|seg| seg == ".." || seg == ".git") {
            return Err(format!("refusing path `{rel}`"));
        }
        let path = base_dir.join(rel);
        match content {
            Some(bytes) => write_page(&path, bytes)?,
            None => match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(format!("{}: {e}", path.display())),
            },
        }
    }
    let mut add = vec!["add", "-A", "--"];
    add.push(if subpath.is_empty() { "." } else { subpath });
    git(clone, &add)?;
    let ident_name = format!("user.name={committer_name}");
    let ident_email = format!("user.email={committer_email}");
    let author_line = format!("{author_name} <{author_email}>");
    git(
        clone,
        &[
            "-c",
            &ident_name,
            "-c",
            &ident_email,
            "commit",
            "--author",
            &author_line,
            "-m",
            message,
        ],
    )?;
    let commit = git(clone, &["rev-parse", "HEAD"])?.trim().to_owned();
    // The push goes to the pusher's own URL when they brought one — the
    // credential rides the command line for this push only, never the
    // clone's config — else to `origin`.
    let target = push_url.unwrap_or("origin");
    let refspec = format!("{branch}:{branch}");
    let mut push = vec!["push"];
    if force {
        push.push("--force");
    }
    push.extend([target, refspec.as_str()]);
    git(clone, &push)?;
    // Leave the clone detached at the base again so a later sync
    // finds the shape it expects and the branch is not left checked
    // out with the pushed tree over the export's.
    let _ = git(clone, &["checkout", "--force", "--detach", base]);
    Ok(Landing {
        branch: branch.to_owned(),
        commit,
        pull_request: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Run `git` with a fixed identity, asserting success.
    fn g(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(["-c", "user.email=test@example.com", "-c", "user.name=Test"])
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
        assert!(
            out.status.success(),
            "git {args:?} failed: {}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
        String::from_utf8_lossy(&out.stdout).into_owned()
    }

    /// A bare remote and a working clone of it with `docs/` committed
    /// on `main`. Returns `(bare, work)`.
    fn upstream(dir: &Path) -> (PathBuf, PathBuf) {
        let bare = dir.join("remote.git");
        std::fs::create_dir_all(&bare).unwrap();
        g(&bare, &["init", "--bare", "--initial-branch=main"]);
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        g(&work, &["init", "--initial-branch=main"]);
        std::fs::write(work.join("README.md"), "# not docs\n").unwrap();
        std::fs::create_dir_all(work.join("docs/guide")).unwrap();
        std::fs::write(work.join("docs/index.md"), "# Docs\n").unwrap();
        std::fs::write(work.join("docs/guide/setup.md"), "# Setup\n\nStep one.\n").unwrap();
        std::fs::write(work.join("docs/old.md"), "# Old\n").unwrap();
        g(&work, &["add", "-A"]);
        g(&work, &["commit", "-m", "docs"]);
        g(&work, &["remote", "add", "origin", bare.to_str().unwrap()]);
        g(&work, &["push", "-u", "origin", "main"]);
        (bare, work)
    }

    /// Commit and push everything in `work` to `main`.
    fn commit_upstream(work: &Path, message: &str) {
        g(work, &["add", "-A"]);
        g(work, &["commit", "-m", message]);
        g(work, &["push", "origin", "main"]);
    }

    fn file_url(path: &Path) -> String {
        format!("file://{}", path.display())
    }

    fn source(bare: &Path) -> RepoSource {
        RepoSource {
            url: file_url(bare),
            branch: "main".into(),
            path: "docs".into(),
            ..Default::default()
        }
    }

    fn is_sha(s: &str) -> bool {
        s.len() == 40 && s.chars().all(|c| c.is_ascii_hexdigit())
    }

    fn kinds(changes: &LocalChanges) -> Vec<(&str, ChangeKind)> {
        changes
            .changes
            .iter()
            .map(|c| (c.path.as_str(), c.kind))
            .collect()
    }

    /// The clone sits where no wiki listing looks: hidden, and not a
    /// slug, so a directory scan that accepts only slug-shaped names
    /// never lists it.
    #[test]
    fn the_clone_is_a_hidden_sibling_and_not_slug_shaped() {
        let d = clone_dir(Path::new("/org/wikis"), "docs");
        assert_eq!(d, PathBuf::from("/org/wikis/.repos/docs"));
        let parent = d.parent().unwrap().file_name().unwrap().to_str().unwrap();
        assert!(parent.starts_with('.'));
        assert_ne!(
            wiki_proto::config::slugify(parent),
            parent,
            "`.repos` must not pass as a slug"
        );
    }

    /// t[verify wiki.source.repo] — the first sync mirrors the subpath
    /// and nothing outside it, and records the commit it reflects.
    #[test]
    fn the_first_sync_mirrors_the_subpath_and_records_the_commit() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);

        let out = sync(&wikis, &root, &mut src).unwrap();
        assert!(is_sha(&out.commit), "{out:?}");
        assert_eq!(out.commit, g(&work, &["rev-parse", "HEAD"]).trim());
        assert_eq!(out.changed, 3);
        assert!(!out.unchanged);
        assert_eq!(src.commit, out.commit);
        assert!(!src.fetched_at.is_empty());
        assert!(src.last_error.is_empty());

        assert_eq!(
            std::fs::read_to_string(root.join("index.md")).unwrap(),
            "# Docs\n"
        );
        assert!(root.join("guide/setup.md").is_file());
        assert!(
            !root.join("README.md").exists(),
            "outside `docs/` is not mirrored"
        );
        assert!(
            !root.join("docs").exists(),
            "the subpath is flattened into the root"
        );
        assert!(clone_dir(&wikis, "docs").join(".git").exists());

        // Again, with nothing new: no change, same commit.
        let again = sync(&wikis, &root, &mut src).unwrap();
        assert!(again.unchanged);
        assert_eq!(again.commit, out.commit);

        // And a fresh export has no local changes.
        let changes = local_changes(&root, &src).unwrap();
        assert!(changes.changes.is_empty(), "{changes:?}");
        assert_eq!(changes.base_commit, out.commit);
    }

    /// t[verify wiki.source.sync] — a commit upstream becomes wiki
    /// content on the next sync without anyone re-importing: changed
    /// pages update, a dropped page goes, and the wiki's own `_state/`
    /// is never the repository's to touch.
    #[test]
    fn a_later_commit_updates_pages_deletes_dropped_ones_and_spares_state() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        let first = sync(&wikis, &root, &mut src).unwrap();

        // The wiki's own bookkeeping and scaffold, not from the repo.
        std::fs::create_dir_all(root.join("_state")).unwrap();
        std::fs::write(root.join("_state/wiki.json"), r#"{"slug":"docs"}"#).unwrap();
        std::fs::write(root.join("purpose.md"), "# Docs wiki\n").unwrap();

        std::fs::write(work.join("docs/index.md"), "# Docs v2\n").unwrap();
        std::fs::write(work.join("docs/guide/deploy.md"), "# Deploy\n").unwrap();
        std::fs::remove_file(work.join("docs/old.md")).unwrap();
        commit_upstream(&work, "v2");

        let second = sync(&wikis, &root, &mut src).unwrap();
        assert_ne!(second.commit, first.commit);
        assert_eq!(second.changed, 3, "one edit, one add, one delete");
        assert!(second.conflicts.is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("index.md")).unwrap(),
            "# Docs v2\n"
        );
        assert!(root.join("guide/deploy.md").is_file());
        assert!(!root.join("old.md").exists(), "a dropped page is gone");
        assert_eq!(
            std::fs::read_to_string(root.join("_state/wiki.json")).unwrap(),
            r#"{"slug":"docs"}"#,
            "_state/ survives a sync"
        );
        assert!(
            root.join("purpose.md").is_file(),
            "the wiki's own scaffold survives"
        );
        // ...and is not a local change either: the repository never
        // had it.
        assert!(local_changes(&root, &src).unwrap().changes.is_empty());
    }

    /// t[verify wiki.source.sync] — a fetch that fails says so on the
    /// source and serves what it had, rather than serving nothing.
    #[test]
    fn a_broken_url_sets_last_error_and_leaves_pages_intact() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, _work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        let first = sync(&wikis, &root, &mut src).unwrap();

        src.url = file_url(&tmp.path().join("nowhere.git"));
        let err = sync(&wikis, &root, &mut src).unwrap_err();
        assert!(matches!(err, WikiError::Io(_)), "{err:?}");
        assert!(
            !src.last_error.is_empty(),
            "the failure is recorded on the source"
        );
        assert_eq!(
            src.commit, first.commit,
            "the reflected commit is unchanged"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("index.md")).unwrap(),
            "# Docs\n"
        );
        assert!(root.join("guide/setup.md").is_file());

        // A repo that never synced fails the same way, with nothing
        // written.
        let root2 = wikis.join("other");
        std::fs::create_dir_all(&root2).unwrap();
        let mut bad = RepoSource {
            url: file_url(&tmp.path().join("nowhere.git")),
            ..Default::default()
        };
        assert!(sync(&wikis, &root2, &mut bad).is_err());
        assert!(!bad.last_error.is_empty());
        assert!(bad.commit.is_empty());
        assert!(
            std::fs::read_dir(&root2).unwrap().next().is_none(),
            "nothing written"
        );
    }

    /// An empty branch follows the remote's HEAD, and an empty path is
    /// the repository root.
    #[test]
    fn empty_branch_and_path_mean_remote_head_and_repo_root() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("all");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = RepoSource {
            url: file_url(&bare),
            ..Default::default()
        };
        let out = sync(&wikis, &root, &mut src).unwrap();
        assert_eq!(out.commit, g(&work, &["rev-parse", "HEAD"]).trim());
        assert!(root.join("README.md").is_file());
        assert!(root.join("docs/index.md").is_file());
    }

    /// t[verify wiki.source.editable] — edits to several pages, through
    /// any door, accumulate as one derived change set: added, modified
    /// and deleted, with the wiki's own files never among them.
    #[test]
    fn edits_accumulate_as_local_changes_against_the_base() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, _work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        sync(&wikis, &root, &mut src).unwrap();

        // Two people, three pages, plus the wiki's own bookkeeping.
        std::fs::write(
            root.join("guide/setup.md"),
            "# Setup\n\nStep one, clearer.\n",
        )
        .unwrap();
        std::fs::write(root.join("guide/deploy.md"), "# Deploy\n").unwrap();
        std::fs::remove_file(root.join("old.md")).unwrap();
        std::fs::write(root.join("purpose.md"), "# Docs wiki\n").unwrap();
        std::fs::write(root.join("log.md"), "# Log\n").unwrap();
        std::fs::create_dir_all(root.join("_state")).unwrap();
        std::fs::write(root.join("_state/wiki.json"), "{}").unwrap();
        std::fs::create_dir_all(root.join("raw/sources")).unwrap();
        std::fs::write(root.join("raw/sources/paper.md"), "raw").unwrap();

        let changes = local_changes(&root, &src).unwrap();
        assert_eq!(
            kinds(&changes),
            vec![
                ("guide/deploy.md", ChangeKind::Added),
                ("guide/setup.md", ChangeKind::Modified),
                ("old.md", ChangeKind::Deleted),
            ]
        );
        assert_eq!(changes.base_commit, src.commit);
        assert!(changes.conflicts.is_empty());
        assert!(changes.pending.is_none());

        // Writing a page back to its base content is no change.
        std::fs::write(root.join("guide/setup.md"), "# Setup\n\nStep one.\n").unwrap();
        assert_eq!(local_changes(&root, &src).unwrap().changes.len(), 2);
    }

    /// t[verify wiki.source.editable] — a sync arriving mid-edit
    /// updates the pages nobody touched, leaves an edited page as it
    /// is, and names a page both sides changed as a conflict rather
    /// than choosing. The conflict clears when the page is written
    /// again.
    #[test]
    fn a_sync_keeps_local_edits_and_flags_what_both_sides_changed() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        sync(&wikis, &root, &mut src).unwrap();

        // Here: setup.md edited, old.md edited too.
        std::fs::write(root.join("guide/setup.md"), "# Setup\n\nOurs.\n").unwrap();
        std::fs::write(root.join("old.md"), "# Old\n\nStill useful here.\n").unwrap();
        // Upstream: index.md edited (untouched here), old.md dropped,
        // setup.md left alone, a new page added.
        std::fs::write(work.join("docs/index.md"), "# Docs v2\n").unwrap();
        std::fs::write(work.join("docs/guide/deploy.md"), "# Deploy\n").unwrap();
        std::fs::remove_file(work.join("docs/old.md")).unwrap();
        commit_upstream(&work, "v2");

        let out = sync(&wikis, &root, &mut src).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("index.md")).unwrap(),
            "# Docs v2\n",
            "an untouched page follows upstream"
        );
        assert!(root.join("guide/deploy.md").is_file());
        assert_eq!(
            std::fs::read_to_string(root.join("guide/setup.md")).unwrap(),
            "# Setup\n\nOurs.\n",
            "an edited page is never overwritten"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("old.md")).unwrap(),
            "# Old\n\nStill useful here.\n",
            "a page edited here and dropped upstream is kept"
        );
        assert_eq!(out.conflicts, vec!["old.md".to_owned()]);
        assert_eq!(src.conflicts, vec!["old.md".to_owned()]);
        let changes = local_changes(&root, &src).unwrap();
        assert_eq!(
            kinds(&changes),
            vec![
                ("guide/setup.md", ChangeKind::Modified),
                ("old.md", ChangeKind::Added),
            ],
            "the change set is against the new base"
        );
        assert_eq!(changes.conflicts, vec!["old.md".to_owned()]);

        // Both sides edit the same page: kept ours, named.
        std::fs::write(work.join("docs/guide/setup.md"), "# Setup\n\nTheirs.\n").unwrap();
        commit_upstream(&work, "v3");
        let out = sync(&wikis, &root, &mut src).unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("guide/setup.md")).unwrap(),
            "# Setup\n\nOurs.\n"
        );
        assert_eq!(
            out.conflicts,
            vec!["guide/setup.md".to_owned(), "old.md".to_owned()]
        );

        // Writing the page again — the person has looked — clears it;
        // so does bringing it to what upstream has.
        std::fs::write(
            root.join("guide/setup.md"),
            "# Setup\n\nOurs, having read theirs.\n",
        )
        .unwrap();
        std::fs::write(root.join("old.md"), "# Old\n").unwrap();
        let changes = local_changes(&root, &src).unwrap();
        assert!(changes.conflicts.is_empty(), "{changes:?}");
        let out = sync(&wikis, &root, &mut src).unwrap();
        assert!(out.conflicts.is_empty());
        assert!(src.conflicts.is_empty());
        assert_eq!(
            std::fs::read_to_string(root.join("guide/setup.md")).unwrap(),
            "# Setup\n\nOurs, having read theirs.\n",
            "still ours after the conflict cleared"
        );
    }

    /// t[verify wiki.source.editable] — a push is one commit on one
    /// branch holding every local change, authored and committed by the
    /// pusher, based on the current upstream commit; a second push
    /// rewrites the same branch; once upstream merges it a sync finds
    /// the two agree and the change set is empty.
    #[test]
    fn a_push_is_one_commit_on_one_branch_and_a_merge_empties_the_working_copy() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        sync(&wikis, &root, &mut src).unwrap();

        std::fs::write(root.join("guide/setup.md"), "# Setup\n\nOurs.\n").unwrap();
        std::fs::write(root.join("guide/deploy.md"), "# Deploy\n").unwrap();
        std::fs::remove_file(root.join("old.md")).unwrap();
        // Upstream moved meanwhile on a page nobody touched here; the
        // sync takes it and the push is based on that commit.
        std::fs::write(work.join("docs/index.md"), "# Docs v2\n").unwrap();
        commit_upstream(&work, "v2");
        sync(&wikis, &root, &mut src).unwrap();
        let base = src.commit.clone();

        let changes = local_changes(&root, &src).unwrap();
        let pusher = Pusher::origin("Eve Editor", "eve@acme.test");
        let first = push_working_copy(
            &wikis,
            &root,
            "docs",
            &src,
            "wiki/docs/eve",
            &changes.changes,
            "Clarify setup; add deploy; drop old\n\nPushed from the wiki.",
            &pusher,
        )
        .unwrap();
        assert_eq!(first.branch, "wiki/docs/eve");
        assert!(is_sha(&first.commit));
        assert_eq!(
            g(&bare, &["rev-parse", "refs/heads/wiki/docs/eve"]).trim(),
            first.commit
        );
        assert_eq!(
            g(&bare, &["rev-parse", &format!("{}^", first.commit)]).trim(),
            base,
            "based on the commit the working copy sits on"
        );
        let who = g(
            &bare,
            &["log", "-1", "--format=%an <%ae>|%cn <%ce>", &first.commit],
        );
        assert_eq!(
            who.trim(),
            "Eve Editor <eve@acme.test>|Eve Editor <eve@acme.test>"
        );
        let files = g(
            &bare,
            &["show", "--name-status", "--format=", &first.commit],
        );
        assert!(files.contains("M\tdocs/guide/setup.md"), "{files}");
        assert!(files.contains("A\tdocs/guide/deploy.md"), "{files}");
        assert!(files.contains("D\tdocs/old.md"), "{files}");
        assert!(!files.contains("index.md"), "only local edits: {files}");
        // `main` did not move: the push goes to a branch for review.
        assert_eq!(g(&bare, &["rev-parse", "refs/heads/main"]).trim(), base);
        src.pending = Some(wiki_proto::config::PendingPush {
            branch: first.branch.clone(),
            commit: first.commit.clone(),
            pull_request: String::new(),
        });

        // More edits; a second push rewrites the same branch with the
        // fuller set, still one commit off the base.
        std::fs::write(root.join("guide/setup.md"), "# Setup\n\nOurs, again.\n").unwrap();
        let changes = local_changes(&root, &src).unwrap();
        assert_eq!(changes.changes.len(), 3);
        let second = push_working_copy(
            &wikis,
            &root,
            "docs",
            &src,
            &first.branch,
            &changes.changes,
            "Clarify setup; add deploy; drop old",
            &pusher,
        )
        .unwrap();
        assert_ne!(second.commit, first.commit);
        assert_eq!(
            g(&bare, &["rev-parse", "refs/heads/wiki/docs/eve"]).trim(),
            second.commit
        );
        assert_eq!(
            g(&bare, &["rev-parse", &format!("{}^", second.commit)]).trim(),
            base
        );
        assert_eq!(g(&bare, &["branch", "--list", "wiki/*"]).lines().count(), 1);
        src.pending.as_mut().unwrap().commit = second.commit.clone();
        let not_yet = sync(&wikis, &root, &mut src).unwrap();
        assert!(!not_yet.merged_pending);
        assert!(src.pending.is_some(), "unmerged: still pending");
        assert_eq!(
            std::fs::read_to_string(root.join("guide/setup.md")).unwrap(),
            "# Setup\n\nOurs, again.\n",
            "a sync while pending keeps the working copy"
        );

        // Upstream merges the branch; the next sync sees it.
        g(&work, &["fetch", "origin"]);
        g(&work, &["merge", "--no-edit", "origin/wiki/docs/eve"]);
        g(&work, &["push", "origin", "main"]);
        let merged = sync(&wikis, &root, &mut src).unwrap();
        assert!(merged.merged_pending);
        assert!(src.pending.is_none(), "merged: pending cleared");
        assert!(src.conflicts.is_empty());
        let changes = local_changes(&root, &src).unwrap();
        assert!(changes.changes.is_empty(), "{changes:?}");
        assert_eq!(
            std::fs::read_to_string(root.join("guide/setup.md")).unwrap(),
            "# Setup\n\nOurs, again.\n"
        );
        assert!(!root.join("old.md").exists());

        // Nothing to push is a refusal, and a stale manifest format
        // still reads.
        let err = push_working_copy(
            &wikis,
            &root,
            "docs",
            &src,
            "wiki/docs/eve",
            &[],
            "x",
            &pusher,
        )
        .unwrap_err();
        assert!(
            matches!(&err, WikiError::Io(m) if m.contains("nothing to push")),
            "{err:?}"
        );
        std::fs::write(manifest_path(&root), r#"["guide/setup.md","index.md"]"#).unwrap();
        let legacy = read_manifest(&root);
        assert_eq!(legacy.files.len(), 2);
        assert!(legacy.files.values().all(String::is_empty));
    }

    /// `land` pushes a prepared change set as a branch whose commit the
    /// remote holds, authored by the person given, with the change
    /// under the wiki's subpath — and the working copy untouched.
    #[test]
    fn land_pushes_a_branch_with_the_given_author() {
        let tmp = tempfile::tempdir().unwrap();
        let (bare, _work) = upstream(tmp.path());
        let wikis = tmp.path().join("org/wikis");
        let root = wikis.join("docs");
        std::fs::create_dir_all(&root).unwrap();
        let mut src = source(&bare);
        sync(&wikis, &root, &mut src).unwrap();

        let landing = land(
            &wikis,
            "docs",
            &src,
            "wiki/edit-42",
            &[
                (
                    "guide/setup.md".to_owned(),
                    Some("# Setup\n\nStep one, clearer.\n".to_owned()),
                ),
                ("old.md".to_owned(), None),
            ],
            "Clarify setup; drop old page",
            ("Alice Owner", "alice@acme.test"),
            &Pusher::origin("Eve Editor", "eve@acme.test"),
        )
        .unwrap();
        assert_eq!(landing.branch, "wiki/edit-42");
        assert!(is_sha(&landing.commit));
        assert!(landing.pull_request.is_none());

        assert_eq!(
            g(&bare, &["rev-parse", "refs/heads/wiki/edit-42"]).trim(),
            landing.commit,
            "the remote branch is at the landed commit"
        );
        let author = g(&bare, &["log", "-1", "--format=%an <%ae>", &landing.commit]);
        assert_eq!(author.trim(), "Alice Owner <alice@acme.test>");
        let files = g(
            &bare,
            &["show", "--name-status", "--format=", &landing.commit],
        );
        assert!(files.contains("M\tdocs/guide/setup.md"), "{files}");
        assert!(files.contains("D\tdocs/old.md"), "{files}");
        assert_eq!(
            g(&bare, &["rev-parse", "refs/heads/main"]).trim(),
            src.commit
        );
        assert_eq!(
            std::fs::read_to_string(root.join("guide/setup.md")).unwrap(),
            "# Setup\n\nStep one.\n"
        );
    }
}
