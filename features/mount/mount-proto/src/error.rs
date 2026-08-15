//! Trait-boundary error type.

/// Errors a [`crate::ContentBackend`] impl can surface. Kept
/// flat + serde-clean so the same enum can travel over vox
/// once the proxy backend lands.
#[derive(Debug, thiserror::Error)]
pub enum MountError {
    /// The requested path doesn't exist under this mount.
    #[error("not found: {path}")]
    NotFound { path: String },

    /// Underlying I/O failure. The backend stringifies the
    /// source so callers don't depend on `std::io::Error` (or
    /// `reqwest::Error`, etc.) leaking through.
    #[error("i/o: {0}")]
    Io(String),

    /// Mount config inconsistency — e.g. a `Nextcloud` mount
    /// with no `url`, or a `Filesystem` mount pointing at a
    /// non-existent root.
    #[error("invalid mount: {0}")]
    InvalidMount(String),

    /// Backend-specific failure that doesn't fit the variants
    /// above (auth, quota, server 5xx). Free-form message.
    #[error("backend: {0}")]
    Backend(String),
}

impl From<std::io::Error> for MountError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::NotFound {
            Self::NotFound {
                path: e.to_string(),
            }
        } else {
            Self::Io(e.to_string())
        }
    }
}
