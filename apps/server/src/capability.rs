//! Minimal signing-key + error primitives the `attachments` flow
//! uses to issue + verify signed blob URLs.
//!
//! Was originally part of a wider capability-token scheme that
//! also fenced `DocId` access; the rest of that scheme was ripped
//! along with the Loro entity layer (no `DocIds` left to scope).
//! What survives here is the Ed25519 keypair + the
//! `CapabilityError` shape `BlobToken` matches on.

use std::path::{Path, PathBuf};

use ed25519_dalek::{SECRET_KEY_LENGTH, SigningKey, VerifyingKey};

#[derive(Debug, thiserror::Error)]
pub enum CapabilityError {
    #[error("token base64 invalid")]
    BadBase64,
    #[error("token signature invalid")]
    BadSignature,
    #[error("token expired")]
    Expired,
    #[error("token malformed")]
    Malformed,
    #[error("unsupported token version")]
    UnsupportedVersion,
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("keypair: {0}")]
    Keypair(String),
}

/// Server's signing keypair. Serialized on disk as the raw
/// 32-byte seed (no envelope — file mode 0600 is the trust
/// boundary).
#[derive(Clone)]
pub struct ServerKeypair {
    signing: SigningKey,
}

impl ServerKeypair {
    #[must_use]
    pub fn generate_ephemeral() -> Self {
        use rand::RngCore;
        let mut seed = [0u8; SECRET_KEY_LENGTH];
        rand::thread_rng().fill_bytes(&mut seed);
        Self {
            signing: SigningKey::from_bytes(&seed),
        }
    }

    pub fn load_or_generate(path: &Path) -> Result<Self, CapabilityError> {
        if let Ok(bytes) = std::fs::read(path) {
            if bytes.len() == SECRET_KEY_LENGTH {
                let mut arr = [0u8; SECRET_KEY_LENGTH];
                arr.copy_from_slice(&bytes);
                return Ok(Self {
                    signing: SigningKey::from_bytes(&arr),
                });
            }
            return Err(CapabilityError::Keypair(format!(
                "key file wrong length: {}",
                bytes.len()
            )));
        }
        // Generate + persist (mode 0600 on unix).
        let kp = Self::generate_ephemeral();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, kp.signing.to_bytes())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(path)?.permissions();
            perms.set_mode(0o600);
            let _ = std::fs::set_permissions(path, perms);
        }
        Ok(kp)
    }

    #[must_use]
    pub fn verifying_key(&self) -> VerifyingKey {
        self.signing.verifying_key()
    }

    /// Module-internal accessor for the raw signing key. Crate-
    /// private so callers can't accidentally serialize the
    /// secret seed.
    pub(crate) fn signing_key(&self) -> &SigningKey {
        &self.signing
    }
}

pub fn default_keypair_path() -> eyre::Result<PathBuf> {
    let root = org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
    std::fs::create_dir_all(root.path())
        .map_err(|e| eyre::eyre!("create {}: {e}", root.path().display()))?;
    Ok(root.server_keypair_path())
}
