//! Opens the jj repo backing one File Root — either flavor (ADR 0001).
//!
//! - **Media** roots wrap `files_store::version::repo::init_repo`
//!   (first touch) with a reopen path through jj-lib's own `RepoLoader`
//!   (every subsequent touch, including after a process restart) — the
//!   version-store crate owns both halves for media roots
//!   (`repo::open_or_init_repo_blocking`, moved down there in issue #262
//!   because every Storage agent hosting a live tree performs exactly
//!   the same open), so the media branches below delegate to it.
//! - **Software** roots (issue #273) are stock **colocated git**: the jj
//!   metadata lives in the root's [`STORE_DIR`](crate::consts::STORE_DIR)
//!   while the objects live in a perfectly ordinary `.git` at the root's
//!   top level, so `git`, GitHub, CI, and IDEs see a normal repository.
//!   A folder that already *is* a git repo is adopted rather than
//!   re-initialized, and its existing history is imported into the jj
//!   view so Files chains start from real project history.
//!
//! **Sync, not async.** jj-lib's own async fns (`load_at_head` in
//! particular, on the divergent-op-heads-merge path) hold a `&dyn Repo`
//! across an await point; `dyn Repo` isn't `Sync`, so that future isn't
//! `Send`. `#[architect::rpc]` methods must return a `MaybeSend`
//! future, so any of this crate's async fns that `.await`ed jj-lib
//! directly would poison the whole RPC method's future. Driving jj-lib
//! to completion with `pollster::block_on` inside a plain sync fn (same
//! pattern `VersionStoreBackend`'s own `block_on` helper documents for
//! its sync `Backend` methods) keeps the non-Send future entirely off
//! this crate's async call stack — see `backend.rs`'s module doc.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use files_proto::RootFlavor;
use jj_lib::backend::{Backend, BackendInitError, BackendLoadError};
use jj_lib::config::{ConfigLayer, ConfigSource};
use jj_lib::default_backend_factories::default_backend_factories;
use jj_lib::git_backend::GitBackend;
use jj_lib::repo::{ReadonlyRepo, RepoLoader};
use jj_lib::settings::UserSettings;
use jj_lib::signing::Signer;
use files_store::version::VersionStoreBackend;

use crate::consts::{GIT_DIR, STORE_DIR};
use crate::error::{Error, Result};
use crate::git_root;

/// `ReadonlyRepo::init` (via `init_repo`) requires a directory with no
/// existing repo internals; jj lays those out under `store/` on init,
/// so its presence is what distinguishes "never touched" from
/// "reopen".
fn already_initialized(repo_path: &Path) -> bool {
    repo_path.join("store").exists()
}

/// Where a root keeps its jj metadata.
#[must_use]
pub fn store_dir(root_path: &Path) -> PathBuf {
    root_path.join(STORE_DIR)
}

/// Open the repo backing the root at `root_path` **only if it already
/// exists** — `Ok(None)` when the root has no store yet.
///
/// Read paths use this instead of [`open_or_init_repo`] so that
/// browsing never *writes*: a registered root whose volume is
/// unmounted (or whose tree was deleted behind Files' back) would
/// otherwise have a fresh, empty store initialized inside the stale
/// mountpoint by nothing more than a click in the explorer — after
/// which the store says "this root is empty" and the next
/// `checkpoint_now` treats the real tree as brand new (PR #288
/// review). A read that finds no store degrades to a plain live-tree
/// listing with no badges, which is the truth: nothing is tracked yet.
pub fn open_existing_repo(
    root_path: &Path,
    flavor: RootFlavor,
) -> Result<Option<Arc<ReadonlyRepo>>> {
    let repo_path = store_dir(root_path);
    if !already_initialized(&repo_path) {
        return Ok(None);
    }
    let repo = open_existing(&repo_path, flavor)?;
    match flavor {
        RootFlavor::Software => {
            git_root::exclude_root_internals(&repo)?;
            git_root::import_from_git(repo).map(Some)
        }
        RootFlavor::Media => Ok(Some(repo)),
    }
}

/// Open the repo backing the root at `root_path`, initializing it on
/// first touch. Reopening goes through `RepoLoader::init_from_file_system`
/// with our own `VersionStoreBackend` layered onto jj-lib's stock
/// op-store/op-heads-store/index/submodule-store factories
/// (`default_backend_factories`, which already carries the git backend a
/// software root loads through) — this is what makes "root identity
/// survives" (issue #259 acceptance criteria) true across a
/// `FilesBackend` restart, not just within one process's lifetime.
pub fn open_or_init_repo(root_path: &Path, flavor: RootFlavor) -> Result<Arc<ReadonlyRepo>> {
    open_or_init_repo_at(&store_dir(root_path), Some(root_path), flavor)
}

/// Open or create a repo at an explicit store directory.
///
/// `tree` is the root's working tree, and `None` means there is not one
/// — a host holding an org's structure and not its content. A media
/// repo needs no tree at all: `open_or_init_repo_blocking` takes a
/// directory, and commits and manifests are structure, so such a store
/// is a complete copy of what the host is hosting.
///
/// A software root is different in kind and says so. Its repo is a
/// colocated git checkout, so "the history without the files" is not a
/// state git has — there is nothing to open.
pub fn open_or_init_repo_at(
    repo_path: &Path,
    tree: Option<&Path>,
    flavor: RootFlavor,
) -> Result<Arc<ReadonlyRepo>> {
    let repo_path = repo_path.to_path_buf();
    let repo_path = &repo_path;
    let repo = if already_initialized(repo_path) {
        open_existing(repo_path, flavor)?
    } else {
        match flavor {
            RootFlavor::Media => {
                files_store::version::repo::open_or_init_repo_blocking(repo_path)
                    .map_err(Error::from)?
            }
            RootFlavor::Software => {
                let Some(root_path) = tree else {
                    return Err(Error::BadRequest(
                        "a software root is a colocated git checkout; \
                         it cannot be hosted without its tree"
                            .to_owned(),
                    ));
                };
                init_software_repo(repo_path, root_path)?
            }
        }
    };
    match flavor {
        // Reflect whatever git itself has done since we last looked
        // (an adopted repo's existing history on first touch; commits,
        // fetches, or branch moves made by git tooling between RPCs) —
        // this is the half of "colocated" that keeps the jj view honest.
        RootFlavor::Software => {
            git_root::exclude_root_internals(&repo)?;
            git_root::import_from_git(repo)
        }
        RootFlavor::Media => Ok(repo),
    }
}

/// Settings for a software root. jj-lib's defaults leave
/// `user.name`/`user.email` empty, which would write git commits with an
/// empty identity — legal bytes, but `git log`/`git blame` and every
/// forge render them as a blank author. Files authors checkpoints as
/// itself; a human's own commits through `git` are untouched by this.
fn software_settings() -> Result<UserSettings> {
    let mut config = files_store::version::repo::default_settings()
        .map_err(|e| Error::Repo(e.to_string()))?
        .config()
        .clone();
    let layer = ConfigLayer::parse(
        ConfigSource::User,
        "[user]\nname = \"Task Files\"\nemail = \"files@fasttrackstudio.app\"\n",
    )
    .map_err(|e| Error::Repo(format!("building software-root settings: {e}")))?;
    config.add_layer(layer);
    UserSettings::from_config(config)
        .map_err(|e| Error::Repo(format!("building software-root settings: {e}")))
}

fn settings_for(flavor: RootFlavor) -> Result<UserSettings> {
    match flavor {
        RootFlavor::Media => files_store::version::repo::default_settings()
            .map_err(|e| Error::Repo(e.to_string())),
        RootFlavor::Software => software_settings(),
    }
}

/// Initialize a colocated git-backed repo: jj metadata under
/// `repo_path` (`<root>/.fts-files`), git objects in `<root>/.git`.
///
/// The git repo path handed to jj-lib is *relative* (`../../.git` from
/// the jj store directory), which is what lets a root be moved or
/// mounted at a different absolute path — a Storage-Location relocation
/// (ADR 0001) must not invalidate the link.
fn init_software_repo(repo_path: &Path, root_path: &Path) -> Result<Arc<ReadonlyRepo>> {
    let settings = software_settings()?;
    std::fs::create_dir_all(repo_path)?;
    let adopt_existing = root_path.join(GIT_DIR).exists();

    let initializer = |settings: &UserSettings,
                       store_path: &Path|
     -> std::result::Result<Box<dyn Backend>, BackendInitError> {
        // `store_path` is `<root>/.fts-files/store`, so the root itself
        // is two levels up.
        let workspace_root = Path::new("..").join("..");
        let backend = if adopt_existing {
            GitBackend::init_external(settings, store_path, &workspace_root.join(GIT_DIR))
        } else {
            GitBackend::init_colocated(settings, store_path, &workspace_root, gix::hash::Kind::Sha1)
        }
        .map_err(|e| BackendInitError(e.into()))?;
        Ok(Box::new(backend) as Box<dyn Backend>)
    };

    pollster::block_on(ReadonlyRepo::init(
        &settings,
        repo_path,
        &initializer,
        Signer::new(None, vec![]),
        ReadonlyRepo::default_op_store_initializer(),
        ReadonlyRepo::default_op_heads_store_initializer(),
        ReadonlyRepo::default_index_store_initializer(),
        ReadonlyRepo::default_submodule_store_initializer(),
    ))
    .map_err(|e| Error::Repo(e.to_string()))
}

fn open_existing(repo_path: &Path, flavor: RootFlavor) -> Result<Arc<ReadonlyRepo>> {
    // A media root's reopen is the version store's own — same settings,
    // same factories — so it is called rather than re-implemented here
    // (issue #262 moved it down for the Storage agents, which do the
    // identical open when they host a live tree). Callers only reach
    // this once `already_initialized` holds, so the shared function's
    // init branch is unreachable from here.
    if flavor == RootFlavor::Media {
        return files_store::version::repo::open_or_init_repo_blocking(repo_path)
            .map_err(Error::from);
    }
    let settings = settings_for(flavor)?;

    let mut factories = default_backend_factories();
    factories.add_backend(
        VersionStoreBackend::NAME,
        Box::new(|_settings, store_path| {
            let store_path = store_path.to_path_buf();
            pollster::block_on(VersionStoreBackend::open(&store_path))
                .map(|backend| Box::new(backend) as Box<dyn Backend>)
                .map_err(|e| BackendLoadError(e.into()))
        }),
    );

    let loader = RepoLoader::init_from_file_system(&settings, repo_path, &factories)
        .map_err(|e| Error::Repo(e.to_string()))?;
    pollster::block_on(loader.load_at_head()).map_err(|e| Error::Repo(e.to_string()))
}
