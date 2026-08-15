//! `checkpoint_now`'s full-scan enumeration (spec's "Session checkpoint
//! ... certified by a full scan"): walk the live tree and walk the
//! checkpoint head's tracked paths. [`crate::checkpoint`] turns the two
//! into a streamed, skip-unchanged commit.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use files_proto::RootFlavor;
use jj_lib::backend::{Backend, Tree, TreeValue};
use jj_lib::gitignore::GitIgnoreFile;
use jj_lib::repo_path::{RepoPath, RepoPathBuf};

use crate::consts::{GIT_DIR, MARKER_FILE, STORE_DIR};
use crate::error::{Error, Result};
use crate::ignore;

/// One regular file found in a root's live tree.
pub struct LiveFile {
    /// Root-relative jj path.
    pub repo_path: RepoPathBuf,
    /// Absolute path on disk.
    pub disk_path: PathBuf,
    /// The file's on-disk executable bit, carried through to the tree
    /// entry's git mode (100755 vs 100644). On a software root this is
    /// real git metadata a clone depends on; recording every file as
    /// non-executable would ship broken scripts (PR #282 review).
    pub executable: bool,
    /// Matched by the root's Ignore set ([`crate::ignore`]). Such a file
    /// is skipped by a checkpoint *unless it is already tracked* — an
    /// ignore pattern must never turn into a recorded deletion.
    pub ignored: bool,
    /// The on-disk content is a **pointer stub** ([`crate::stub`],
    /// issue #263) — a placeholder, never content. A checkpoint keeps
    /// the path's tracked state untouched (dehydration is invisible to
    /// history); nothing ever ingests the stub's own bytes.
    pub stub: Option<crate::stub::Stub>,
}

/// Recursively list every regular file under `root_path`, as [`LiveFile`]
/// entries. Skips [`STORE_DIR`] / [`MARKER_FILE`] at *every* depth, not
/// just the root's own top level — a File Root's internals must never be
/// ingested as ordinary content even if they show up nested (e.g. a
/// rejected-but-still-on-disk nested root, or a manually copied
/// `.fts-files` directory; see PR #280 review finding on nested roots).
/// [`GIT_DIR`] joins that list on software roots, where it *is* the
/// root's object store (and where a nested one is a submodule's store,
/// which git itself doesn't track either). On a media root a `.git`
/// directory is ordinary content, versioned like anything else — the
/// media flavor is unchanged by this ticket. Symlinks are skipped (v1
/// has no symlink writer wired through yet).
///
/// `ignores` is the root's whole Ignore set already composed — flavor
/// seed plus the root's stored patterns ([`crate::ignore::for_root`]) —
/// and `flavor` decides whether the tree's own `.gitignore` files are
/// chained onto it as the walk descends (software roots only).
///
/// `tracked` is the checkpoint head's path set, and it is what makes
/// ignoring safe: an ignored directory is normally pruned unvisited (both
/// correct gitignore semantics and what keeps a stray `node_modules` from
/// costing a full-tree stat walk), but a directory holding *already
/// tracked* files is descended into anyway, with everything under it
/// marked [`LiveFile::ignored`]. Without that, adding `docs/` to a
/// `.gitignore` — or adopting a repo that deliberately commits fixtures
/// under `target/`, which the software seed ignores — would make the next
/// checkpoint record a mass deletion (PR #282 review). An Ignore set
/// decides what *starts* being versioned; it never ends it.
pub fn walk_live_tree(
    root_path: &Path,
    flavor: RootFlavor,
    ignores: &Arc<GitIgnoreFile>,
    tracked: &BTreeSet<RepoPathBuf>,
) -> Result<Vec<LiveFile>> {
    let mut out = Vec::new();
    let ignores = chain_dir_gitignore(ignores, RepoPath::root(), root_path, flavor)?;
    let ctx = WalkCtx { flavor, tracked };
    walk_dir(root_path, RepoPath::root(), &ignores, false, &ctx, &mut out)?;
    Ok(out)
}

struct WalkCtx<'a> {
    flavor: RootFlavor,
    tracked: &'a BTreeSet<RepoPathBuf>,
}

impl WalkCtx<'_> {
    /// Does the checkpoint head track anything under `dir`? Answered by
    /// one ordered range probe, not a scan of the whole path set.
    fn tracks_anything_under(&self, dir: &RepoPath) -> bool {
        let prefix = format!("{}/", dir.as_internal_file_string());
        self.tracked
            .range(dir.to_owned()..)
            .next()
            .is_some_and(|p| p.as_internal_file_string().starts_with(&prefix))
    }
}

/// Layer `dir`'s own `.gitignore` onto `parent`, on flavors that honor it.
fn chain_dir_gitignore(
    parent: &Arc<GitIgnoreFile>,
    prefix: &RepoPath,
    dir: &Path,
    flavor: RootFlavor,
) -> Result<Arc<GitIgnoreFile>> {
    if !ignore::honors_gitignore(flavor) {
        return Ok(parent.clone());
    }
    parent
        .chain_with_file(prefix, dir.join(".gitignore"))
        .map_err(|e| Error::Repo(format!("{}: reading .gitignore: {e}", dir.display())))
}

/// `dir_ignored` means this directory (or an ancestor) matched the Ignore
/// set: every file below it is ignored regardless of its own name, since
/// gitignore's per-file matching isn't meaningful inside an ignored
/// directory (jj's [`GitIgnoreFile`] documents exactly that).
fn walk_dir(
    dir: &Path,
    dir_repo_path: &RepoPath,
    ignores: &Arc<GitIgnoreFile>,
    dir_ignored: bool,
    ctx: &WalkCtx<'_>,
    out: &mut Vec<LiveFile>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let path = entry.path();
        let name = entry.file_name();
        if name == MARKER_FILE || name == STORE_DIR {
            continue;
        }
        if name == GIT_DIR && ctx.flavor == RootFlavor::Software {
            continue;
        }
        let Some(name_str) = name.to_str() else {
            continue; // non-UTF8 names are out of scope for v1
        };
        let Ok(component) = jj_lib::repo_path::RepoPathComponentBuf::new(name_str) else {
            continue;
        };
        let child_repo_path = dir_repo_path.join(&component);

        if file_type.is_dir() {
            // A nested root is a SUBMODULE, not content. Its own store
            // owns its history, so the outer root walks around it —
            // exactly as git records a gitlink rather than the
            // submodule's files.
            //
            // Without this the parent would ingest the child's entire
            // version store as ordinary files on every checkpoint: the
            // child's history duplicated inside the parent's, growing
            // each time the child is checkpointed. That is why
            // `create_root` refused nested roots outright before this
            // existed, and why the prune has to land before the
            // containment check is relaxed.
            //
            // One `stat` per directory, on a walk that already reads
            // every directory and stats every entry.
            if path.join(MARKER_FILE).exists() {
                continue;
            }
            let child_ignored = dir_ignored || ignores.matches_dir(&child_repo_path);
            if child_ignored && !ctx.tracks_anything_under(&child_repo_path) {
                // Ignored and holding no history: pruned unvisited.
                continue;
            }
            let child_ignores = chain_dir_gitignore(ignores, &child_repo_path, &path, ctx.flavor)?;
            walk_dir(
                &path,
                &child_repo_path,
                &child_ignores,
                child_ignored,
                ctx,
                out,
            )?;
        } else if file_type.is_file() {
            // Stub detection rides the stat the walk already takes:
            // only a file small enough to be a stub gets its header
            // read, so no media file is ever opened here. **Media
            // flavor only** — dehydration doesn't exist on software
            // roots, so a stub-shaped file there is ordinary content
            // and gets versioned as its literal bytes rather than
            // silently excluded (PR #289 review). Lenient by design:
            // one malformed or unreadable small file must never take
            // down every checkpoint for the root — `stub::probe` logs
            // and treats it as content.
            let metadata = entry.metadata()?;
            let stub =
                if ctx.flavor == RootFlavor::Media && crate::stub::candidate_len(metadata.len()) {
                    crate::stub::probe(&path)
                } else {
                    None
                };
            out.push(LiveFile {
                ignored: dir_ignored || ignores.matches_file(&child_repo_path),
                executable: is_executable(&entry)?,
                repo_path: child_repo_path,
                disk_path: path,
                stub,
            });
        }
        // symlinks: skipped (see doc comment).
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(entry: &std::fs::DirEntry) -> Result<bool> {
    use std::os::unix::fs::PermissionsExt as _;
    Ok(entry.metadata()?.permissions().mode() & 0o111 != 0)
}

/// Non-unix filesystems carry no executable bit. Returning `false` keeps
/// a file's recorded mode stable there: [`crate::checkpoint`] only rewrites
/// a tree entry whose content changed, and it carries the previous mode
/// forward on this platform rather than flipping every entry to 100644.
#[cfg(not(unix))]
fn is_executable(_entry: &std::fs::DirEntry) -> Result<bool> {
    Ok(false)
}

/// Recursively list every file path tracked in `tree` (root-relative jj
/// paths) — the checkpoint-head half of a checkpoint-now diff.
pub async fn walk_tree_paths(
    backend: &dyn Backend,
    tree: &Tree,
    prefix: &RepoPath,
    out: &mut BTreeSet<RepoPathBuf>,
) -> Result<()> {
    for name in tree.names() {
        let Some(value) = tree.value(name) else {
            continue;
        };
        let path = prefix.join(name);
        match value {
            TreeValue::Tree(id) => {
                let sub = backend.read_tree(&path, id).await?;
                Box::pin(walk_tree_paths(backend, &sub, &path, out)).await?;
            }
            TreeValue::File { .. } => {
                out.insert(path);
            }
            _ => {}
        }
    }
    Ok(())
}
