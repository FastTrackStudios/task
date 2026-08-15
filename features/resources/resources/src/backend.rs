//! [`ResourcesBackend`] — serves `resources_proto::ResourcesService` by
//! reading sidecars under `<org>/resources/`. The query engine for
//! annotations stays elsewhere; this just hands the watch/reader UI the
//! transcript it can't fetch directly (the resources tier isn't the vault).

use std::path::PathBuf;
use std::sync::Arc;

use resources_proto::{ResourcesError, ResourcesService, TranscriptDoc};

#[derive(Clone, architect::HasDispatcher)]
pub struct ResourcesBackend {
    /// `<org>/resources`.
    root: Arc<PathBuf>,
}

impl ResourcesBackend {
    #[must_use]
    pub fn new(resources_root: impl Into<PathBuf>) -> Self {
        Self {
            root: Arc::new(resources_root.into()),
        }
    }
}

impl ResourcesService for ResourcesBackend {
    fn transcript(&self, rel_path: &str) -> Result<TranscriptDoc, ResourcesError> {
        // No traversal outside the resources tier.
        if rel_path.contains("..") {
            return Err(ResourcesError::NotFound(rel_path.to_string()));
        }
        let path = self.root.join(rel_path);
        let text = std::fs::read_to_string(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ResourcesError::NotFound(rel_path.to_string())
            } else {
                ResourcesError::Io(e.to_string())
            }
        })?;
        serde_json::from_str(&text).map_err(|e| ResourcesError::Io(e.to_string()))
    }
}
