//! Read / write the per-resource annotation sidecar
//! (`<resource>.annotations.json`) — the geometry/metadata store that
//! sits next to a resource, exactly as Logseq writes `<pdf>.edn` next to
//! a PDF asset.

use std::path::{Path, PathBuf};

use crate::ResourceError;
use crate::types::AnnotationFile;

/// The sidecar path for a resource file: same directory, `.md` (or any
/// extension) swapped for `.annotations.json`
/// (`songs/keep-on-finding-more.md` → `songs/keep-on-finding-more.annotations.json`).
#[must_use]
pub fn sidecar_path(resource_path: impl AsRef<Path>) -> PathBuf {
    resource_path.as_ref().with_extension("annotations.json")
}

/// Load a sidecar; a missing file is an empty (not an error) sidecar, so
/// callers can read-modify-write uniformly.
pub fn load(path: impl AsRef<Path>) -> Result<AnnotationFile, ResourceError> {
    match std::fs::read_to_string(path.as_ref()) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| ResourceError::Json(e.to_string())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AnnotationFile::default()),
        Err(e) => Err(ResourceError::Io(e.to_string())),
    }
}

/// Write a sidecar (pretty JSON, so it stays human-inspectable like the
/// rest of the vault).
pub fn save(path: impl AsRef<Path>, file: &AnnotationFile) -> Result<(), ResourceError> {
    if let Some(parent) = path.as_ref().parent() {
        std::fs::create_dir_all(parent).map_err(|e| ResourceError::Io(e.to_string()))?;
    }
    let json =
        serde_json::to_string_pretty(file).map_err(|e| ResourceError::Json(e.to_string()))?;
    std::fs::write(path, json).map_err(|e| ResourceError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{Annotation, AnnotationFile};

    #[test]
    fn sidecar_path_swaps_extension() {
        let p = sidecar_path("/x/songs/keep-on-finding-more.md");
        assert!(p.ends_with("keep-on-finding-more.annotations.json"));
    }

    #[test]
    fn missing_sidecar_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        let f = load(dir.path().join("nope.annotations.json")).unwrap();
        assert!(f.annotations.is_empty());
    }

    #[test]
    fn round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("s.annotations.json");
        let mut file = AnnotationFile::new("s");
        file.upsert(Annotation {
            anchor: "chorus.L1".into(),
            label: "You are the maker".into(),
            text: "You are the maker".into(),
            color: None,
            geometry: None,
        });
        save(&path, &file).unwrap();
        assert_eq!(load(&path).unwrap(), file);
    }
}
