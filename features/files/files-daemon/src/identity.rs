//! The daemon's **device identity** (issue #265): a per-machine
//! `(device id, secret)` persisted to disk, so the daemon re-announces
//! as the SAME device across restarts and — crucially — survives a
//! user-session expiry, because it lives in a file the daemon reads at
//! start, not in any login session's state.
//!
//! This maps onto the storage-agent enrollment already built in #262:
//! the device id is the agent id, the secret is the agent's enrollment
//! token. The operator approves a pending device; revoking it (marking
//! the agent Rejected) cuts THIS device without touching others, since
//! every device holds its own id + secret. That mapping is why #265
//! needs no new enrollment protocol — only a place to keep the identity
//! and the daemon loop that uses it.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{DaemonError, Result};

/// A machine's persisted device identity.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceIdentity {
    pub device_id: Uuid,
    /// The enrollment secret the coordinator minted (the agent token).
    /// `None` until the first successful enrollment reply.
    pub secret: Option<String>,
}

impl DeviceIdentity {
    /// The identity file inside the daemon's data dir.
    fn path(data_dir: &Path) -> PathBuf {
        data_dir.join("device-identity.json")
    }

    /// Load this machine's identity, minting a fresh device id (no
    /// secret yet — enrollment fills it) on first run. Persisted
    /// immediately so a crash before enrollment still keeps one stable
    /// id rather than announcing as a new device every start.
    pub fn load_or_create(data_dir: &Path) -> Result<Self> {
        let path = Self::path(data_dir);
        if path.exists() {
            let bytes = std::fs::read(&path).map_err(|e| DaemonError::Io(e.to_string()))?;
            return serde_json::from_slice(&bytes)
                .map_err(|e| DaemonError::Io(format!("device identity: {e}")));
        }
        let fresh = Self {
            device_id: Uuid::new_v4(),
            secret: None,
        };
        fresh.save(data_dir)?;
        Ok(fresh)
    }

    /// Record the enrollment secret (atomic write) — called once the
    /// coordinator's announce reply mints it.
    pub fn record_secret(&mut self, data_dir: &Path, secret: String) -> Result<()> {
        self.secret = Some(secret);
        self.save(data_dir)
    }

    fn save(&self, data_dir: &Path) -> Result<()> {
        std::fs::create_dir_all(data_dir).map_err(|e| DaemonError::Io(e.to_string()))?;
        let path = Self::path(data_dir);
        let tmp = path.with_extension("json.tmp");
        std::fs::write(
            &tmp,
            serde_json::to_vec_pretty(self).expect("identity serializes"),
        )
        .map_err(|e| DaemonError::Io(e.to_string()))?;
        std::fs::rename(&tmp, &path).map_err(|e| DaemonError::Io(e.to_string()))?;
        Ok(())
    }
}
