//! The daemon as a **peer**, not only as a puller.
//!
//! `files_sync`'s module docs say it in one line: "there is no push — a
//! replica that wants its offline checkpoints on the server is *pulled
//! from*". Everything else in this crate is the pulling half, and until
//! this module existed the pulling half was all a shipped daemon had.
//! The consequence was one-way sync with no error to show for it: a
//! laptop took the server's work and its own never left the laptop,
//! because nothing could dial it to ask.
//!
//! So the daemon binds an endpoint and serves [`files_sync::SyncService`]
//! on it, exactly as the integration suite's `Laptop` does and for the
//! same reason — `files.topology.multi-server`'s "where two peers can
//! reach each other, bytes move directly over iroh/QUIC" is not a
//! property a machine has by holding content, it is one it has by
//! answering.
//!
//! # There is nothing to enroll
//!
//! An iroh connection is mutually authenticated by construction, so the
//! endpoint id on the far end was *proved* during the handshake rather
//! than presented afterwards. That makes admission a list rather than a
//! secret ([`files::peer`]), and it is why this crate mints, stores and
//! rotates no credential: the device's key **is** its credential, and
//! `files.device.identity`'s "an identity it holds and persists itself"
//! is that key on disk.
//!
//! What that leaves for an operator is one symmetric step in each
//! direction — the org admits the device's endpoint id, the device
//! admits the org's — and both are ordinary list edits that survive
//! restart and revoke by removal.

use architect::iroh_link::iroh;
use files::FilesBackend;
use files_sync::SyncServiceClient;

use crate::error::{DaemonError, Result};

/// How long dialling a peer may take before it counts as unreachable.
///
/// The engine's own bound, re-exported: the server pulling a laptop and
/// the laptop pulling the server are the same dial, so a reader
/// reasoning about a tick's worst case should find one number rather
/// than two that have to be kept in step.
pub use files_sync::DIAL_TIMEOUT;

/// Bind this machine's endpoint from its persisted key.
///
/// `book` is the address lookup for deployments with nothing to
/// discover — two machines on one LAN with no internet, and the
/// integration suite. A real device passes `None` and is dialled by
/// bare id through n0's DNS.
pub async fn bind(
    key: iroh::SecretKey,
    book: Option<files::AddressBook>,
) -> Result<iroh::Endpoint> {
    files::bind_endpoint(key, book)
        .await
        .map_err(|e| DaemonError::Io(format!("binding the device endpoint: {e}")))
}

/// Serve the replica lane on `endpoint` until it closes.
///
/// Only the replica lane, and only to admitted endpoints — the gate is
/// [`files::peer::device_gate`], which has no sessions and no roles
/// because a device has no accounts. Spawn it; it never returns.
pub async fn serve(backend: FilesBackend, whose: String, endpoint: iroh::Endpoint) {
    files_sync::serve_peer(backend, whose, &endpoint).await;
}

/// Open the replica lane on `peer`, dialling from this machine's own
/// endpoint.
///
/// Signs nothing: what reaches the far gate is the endpoint iroh proved
/// during the handshake, so what authorises the pull is `peer` having
/// admitted this machine.
pub async fn dial(endpoint: &iroh::Endpoint, peer: &str) -> Result<SyncServiceClient> {
    files_sync::dial_peer(endpoint, peer)
        .await
        .map_err(crate::error::from_sync)
}

/// Where a device may keep live trees, beyond its own store directory.
///
/// A `FilesBackend` confines adopted trees to its data dir, and on a
/// server that boundary has real work to do: `create_root` takes a path
/// from a network caller, every org shares one data root, and a path
/// argument that escaped would reach another org's files.
///
/// On a device none of that holds, and the rule is simply wrong. The
/// only caller is the person at the keyboard, over a socket bound to
/// localhost, and their projects live in `~/Task` or `~/Music/Sessions`
/// — not inside an application-support directory. Confined to its own
/// store, a daemon refuses every root it is asked to sync, which is
/// exactly what it did the first time one was pointed at a real server.
///
/// So a device declares its roots directory as a permitted location.
/// Same seam a deployment's Storage Locations use; the only difference
/// is who decides, and on a device that is the person who installed it.
/// The directories this device may hold live trees in.
///
/// Mutable and persisted, because a person adds one whenever they share
/// a folder: `fts-files-daemon share ~/Music/Sessions` has to widen the
/// boundary or the adoption it is asking for would be refused by the
/// backend that just agreed to it. A boundary fixed at startup would
/// mean "restart the agent to share a folder", and forgetting to
/// persist it would mean "the folder you shared is refused after a
/// reboot".
#[derive(Debug)]
pub struct DeviceRoots {
    dirs: std::sync::RwLock<Vec<std::path::PathBuf>>,
    /// Where the list lives, so it survives a restart.
    file: std::path::PathBuf,
}

impl DeviceRoots {
    /// The boundary for a daemon whose data dir is `data_dir` and whose
    /// adopted replicas land under `roots_dir`.
    ///
    /// Both are created first: a boundary that does not exist cannot be
    /// canonicalized, and a first sync is precisely when it does not
    /// exist yet.
    pub fn open(data_dir: &std::path::Path, roots_dir: &std::path::Path) -> Result<Self> {
        let file = data_dir.join("shared-dirs.json");
        let mut dirs = vec![Self::ready(roots_dir)?];
        if let Ok(raw) = std::fs::read_to_string(&file) {
            for saved in serde_json::from_str::<Vec<String>>(&raw).unwrap_or_default() {
                // A shared folder that has gone away — an unplugged
                // drive, a deleted directory — is skipped rather than
                // fatal: the rest of this machine's sync is none of its
                // business.
                match std::path::PathBuf::from(&saved).canonicalize() {
                    Ok(dir) if !dirs.contains(&dir) => dirs.push(dir),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(dir = %saved, error = %e, "shared directory is not reachable"),
                }
            }
        }
        Ok(Self {
            dirs: std::sync::RwLock::new(dirs),
            file,
        })
    }

    /// A boundary over one directory and nothing persisted — for tests
    /// and embedders that manage their own list.
    pub fn at(dir: impl AsRef<std::path::Path>) -> Result<Self> {
        Ok(Self {
            dirs: std::sync::RwLock::new(vec![Self::ready(dir.as_ref())?]),
            file: std::path::PathBuf::new(),
        })
    }

    fn ready(dir: &std::path::Path) -> Result<std::path::PathBuf> {
        std::fs::create_dir_all(dir)
            .map_err(|e| DaemonError::Io(format!("creating {}: {e}", dir.display())))?;
        dir.canonicalize()
            .map_err(|e| DaemonError::Io(format!("resolving {}: {e}", dir.display())))
    }

    /// Permit `dir` from now on, and after the next restart.
    pub fn permit(&self, dir: &std::path::Path) -> Result<std::path::PathBuf> {
        let dir = Self::ready(dir)?;
        let mut dirs = self.dirs.write().expect("shared dirs lock");
        if !dirs.contains(&dir) {
            dirs.push(dir.clone());
        }
        if !self.file.as_os_str().is_empty() {
            let saved: Vec<String> = dirs.iter().map(|d| d.to_string_lossy().into_owned()).collect();
            let bytes = serde_json::to_vec_pretty(&saved)
                .map_err(|e| DaemonError::Io(format!("shared dirs: {e}")))?;
            std::fs::write(&self.file, bytes)
                .map_err(|e| DaemonError::Io(format!("writing {}: {e}", self.file.display())))?;
        }
        Ok(dir)
    }
}

impl files::LocationBoundaries for DeviceRoots {
    fn permitted(&self) -> Vec<std::path::PathBuf> {
        self.dirs.read().expect("shared dirs lock").clone()
    }
}
