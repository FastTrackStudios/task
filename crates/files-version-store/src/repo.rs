//! Wires [`VersionStoreBackend`] into an actual jj-lib repo: everything
//! above the `Backend` trait — op-log concurrency, divergent changes,
//! transactions — is jj-lib's own machinery, unmodified. This module only
//! supplies the initializers `ReadonlyRepo::init` needs and a couple of
//! settings defaults.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use jj_lib::backend::{Backend, BackendLoadError};
use jj_lib::config::{ConfigLayer, ConfigSource, StackedConfig};
use jj_lib::default_backend_factories::default_backend_factories;
use jj_lib::repo::{ReadonlyRepo, RepoInitError, RepoLoader};
use jj_lib::settings::UserSettings;
use jj_lib::signing::Signer;

use crate::backend::{DEFAULT_GC_INTERVAL, VersionStoreBackend};
use crate::error::{Error, Result};

/// Default settings: jj-lib's own baked-in config (`config/misc.toml`) is
/// enough — `user.name`/`user.email` default to `""`, `signing.behavior`
/// defaults to `"keep"` with `signing.backend = "none"`, which resolves to
/// no signing backend configured (`Signer::new(None, vec![])` below) and no
/// commits ever get signed.
///
/// One override: ADR 0001's backend policy calls for
/// `snapshot.max-new-file-size = 0` (disabled) — this crate's own
/// `checkpoint` module never goes through jj's `local_working_copy`
/// snapshotting (it builds trees directly, so the limit has no effect
/// today), but a future working-copy-driven flow (the sync daemon,
/// desktop checkout) will load this same config, and multi-GB media must
/// never hit jj-cli's 1 MiB anti-footgun default.
pub fn default_settings() -> Result<UserSettings> {
    let mut config = StackedConfig::with_defaults();
    let overrides = ConfigLayer::parse(ConfigSource::User, "[snapshot]\nmax-new-file-size = 0\n")
        .map_err(|e| Error::Repo(format!("building snapshot policy overrides: {e}")))?;
    config.add_layer(overrides);
    UserSettings::from_config(config)
        .map_err(|e| Error::Repo(format!("building default UserSettings: {e}")))
}

/// Initialize a brand-new repo at `repo_path` (must not yet exist) backed
/// by [`VersionStoreBackend`]. `repo_path` becomes the jj repo's `.jj`-style
/// metadata directory; the backend's own chunk/object stores live under
/// `repo_path/store` (jj's own convention — see `ReadonlyRepo::init`).
pub async fn init_repo(repo_path: &Path) -> Result<Arc<ReadonlyRepo>> {
    init_repo_with_gc_interval(repo_path, DEFAULT_GC_INTERVAL).await
}

/// [`init_repo`] with a non-default chunk-level GC interval — the seam
/// tests use to observe iroh-blobs' background chunk reclamation within
/// their own runtime rather than waiting on [`DEFAULT_GC_INTERVAL`].
pub async fn init_repo_with_gc_interval(
    repo_path: &Path,
    gc_interval: Duration,
) -> Result<Arc<ReadonlyRepo>> {
    let settings = default_settings()?;
    tokio::fs::create_dir_all(repo_path).await?;

    let backend_initializer =
        |_settings: &UserSettings,
         store_path: &Path|
         -> std::result::Result<Box<dyn Backend>, jj_lib::backend::BackendInitError> {
            let store_path = store_path.to_path_buf();
            pollster::block_on(VersionStoreBackend::open_with_gc_interval(
                &store_path,
                gc_interval,
            ))
            .map(|backend| Box::new(backend) as Box<dyn Backend>)
            .map_err(|e| jj_lib::backend::BackendInitError(e.into()))
        };

    ReadonlyRepo::init(
        &settings,
        repo_path,
        &backend_initializer,
        Signer::new(None, vec![]),
        ReadonlyRepo::default_op_store_initializer(),
        ReadonlyRepo::default_op_heads_store_initializer(),
        ReadonlyRepo::default_index_store_initializer(),
        ReadonlyRepo::default_submodule_store_initializer(),
    )
    .await
    .map_err(|e: RepoInitError| Error::Repo(e.to_string()))
}

/// `ReadonlyRepo::init` requires a directory with no existing repo
/// internals; jj lays those out under `store/` on init, so its presence
/// is what distinguishes "never touched" from "reopen".
fn already_initialized(repo_path: &Path) -> bool {
    repo_path.join("store").exists()
}

/// Open the version-store repo at `repo_path`, initializing it on first
/// touch and going through jj-lib's own [`RepoLoader`] on every
/// subsequent one — with [`VersionStoreBackend`] layered onto jj-lib's
/// stock op-store / op-heads-store / index / submodule-store factories.
/// This is what makes a root's history survive a process restart rather
/// than only the lifetime of the process that created it.
///
/// **Sync, not async, deliberately.** jj-lib's `load_at_head` holds a
/// `&dyn Repo` across an await point on the divergent-op-heads-merge
/// path, and `dyn Repo` isn't `Sync`, so that future isn't `Send` —
/// awaiting it from inside an `#[architect::rpc]` method's future would
/// poison that future's own `Send` bound. Driving jj-lib to completion
/// with `pollster::block_on` inside a plain sync fn (the same pattern
/// [`VersionStoreBackend`]'s own `Backend` impl uses) keeps the non-Send
/// future off every async call stack above it. Callers run it on a
/// blocking thread (`tokio::task::spawn_blocking`).
pub fn open_or_init_repo_blocking(repo_path: &Path) -> Result<Arc<ReadonlyRepo>> {
    if already_initialized(repo_path) {
        open_existing_blocking(repo_path)
    } else {
        pollster::block_on(init_repo(repo_path))
    }
}

fn open_existing_blocking(repo_path: &Path) -> Result<Arc<ReadonlyRepo>> {
    let settings = default_settings()?;

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
