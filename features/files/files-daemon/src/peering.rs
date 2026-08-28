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
    Ok(files_sync::dial_peer(endpoint, peer).await?)
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
#[derive(Debug)]
pub struct DeviceRoots(Vec<std::path::PathBuf>);

impl DeviceRoots {
    /// Permit `dir`, creating it first — a boundary that does not exist
    /// cannot be canonicalized, and a first sync is precisely when it
    /// does not exist yet.
    pub fn at(dir: impl Into<std::path::PathBuf>) -> Result<Self> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)
            .map_err(|e| DaemonError::Io(format!("creating {}: {e}", dir.display())))?;
        let dir = dir
            .canonicalize()
            .map_err(|e| DaemonError::Io(format!("resolving {}: {e}", dir.display())))?;
        Ok(Self(vec![dir]))
    }
}

impl files::LocationBoundaries for DeviceRoots {
    fn permitted(&self) -> Vec<std::path::PathBuf> {
        self.0.clone()
    }
}
