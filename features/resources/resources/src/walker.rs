//! Walk a Resource Library tree (`<org>/resources/`) and load every
//! `type: resource` manifest. Sidecars (`*.annotations.json`,
//! `*.transcript.json`) and non-resource markdown are skipped.

use std::path::{Path, PathBuf};

use crate::manifest::parse_manifest;
use crate::types::Resource;

/// A loaded manifest and where it lives (the path keys its sidecars).
pub struct LoadedResource {
    pub path: PathBuf,
    pub resource: Resource,
}

/// Recursively load every resource manifest under `root`. Files that
/// aren't `.md`, or whose frontmatter isn't a valid resource manifest,
/// are silently skipped (the tree holds plain notes + sidecars too).
#[must_use]
pub fn walk(root: impl AsRef<Path>) -> Vec<LoadedResource> {
    let mut out = Vec::new();
    visit(root.as_ref(), &mut out);
    out.sort_by(|a, b| a.resource.slug.cmp(&b.resource.slug));
    out
}

fn visit(dir: &Path, out: &mut Vec<LoadedResource>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            visit(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(resource) = parse_manifest(&text) {
                    out.push(LoadedResource { path, resource });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_manifests_skips_plain_notes() {
        let dir = tempfile::tempdir().unwrap();
        let songs = dir.path().join("songs");
        std::fs::create_dir_all(&songs).unwrap();
        std::fs::write(
            songs.join("a.md"),
            "---\ntype: resource\nresource_kind: song\nslug: a\ntitle: A\n---\n# a\n",
        )
        .unwrap();
        // A plain note with no resource frontmatter — must be skipped.
        std::fs::write(dir.path().join("readme.md"), "# just notes\n").unwrap();

        let found = walk(dir.path());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].resource.slug, "a");
    }
}
