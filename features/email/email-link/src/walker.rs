//! Vault walker that turns markdown files into link-store rows.
//!
//! Walks every `.md` file under a root directory. For each one:
//! 1. Splits frontmatter from body via [`frontmatter::split`].
//! 2. Resolves the file to an [`EntityRef`] via a caller-supplied
//!    closure (default: frontmatter `type:` + frontmatter `id:`
//!    or the file stem as fallback).
//! 3. Collects every Message-ID it can find — both `emails:`
//!    frontmatter entries AND `[[email://...]]` wikilinks in
//!    the body.
//! 4. Yields `(EntityRef, Vec<message_id>)` pairs the caller can
//!    hand to [`crate::LinkStore::rebuild_from`].
//!
//! The walker doesn't depend on `vault` or `knowledge-proto` —
//! it's a thin, library-style helper any caller can wrap. The
//! vault crate's own walker can adopt this too once it grows
//! email-aware indexing.

use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::entity::{EntityKind, EntityRef};
use crate::frontmatter::{self, ParsedFrontmatter};
use crate::parse::parse_wikilinks;

/// One walked file with everything the link layer needs.
#[derive(Debug, Clone)]
pub struct WalkedFile {
    pub path: PathBuf,
    pub frontmatter: ParsedFrontmatter,
    /// Message-IDs found in `emails:` frontmatter.
    pub frontmatter_emails: Vec<String>,
    /// Message-IDs found via `[[email://...]]` wikilinks in the
    /// body (after the closing `---`).
    pub body_emails: Vec<String>,
}

impl WalkedFile {
    /// Every Message-ID referenced from this file, deduplicated.
    #[must_use]
    pub fn all_message_ids(&self) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        let mut out = Vec::new();
        for mid in self
            .frontmatter_emails
            .iter()
            .chain(self.body_emails.iter())
        {
            // Normalize by stripping angle brackets so the
            // dedup catches both `<a@b.com>` + `a@b.com`.
            let bare = crate::link::bare_message_id(mid).to_string();
            if seen.insert(bare.clone()) {
                out.push(bare);
            }
        }
        out
    }
}

/// Callback shape for resolving a walked file to an entity.
/// Returning `None` skips the file from indexing.
pub type EntityResolver = dyn Fn(&Path, &ParsedFrontmatter) -> Option<EntityRef> + Send + Sync;

/// Default resolver: kind from `type:` / `kind:` frontmatter
/// (lower-cased), id from `id:` / `uuid:` if present otherwise
/// from the file stem. Files without any kind are skipped.
#[must_use]
pub fn default_resolver(path: &Path, fm: &ParsedFrontmatter) -> Option<EntityRef> {
    let kind = fm.kind.as_deref()?;
    let id = match &fm.id {
        Some(id) => id.clone(),
        None => path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(std::string::ToString::to_string)?,
    };
    Some(EntityRef::new(EntityKind::new(kind), id))
}

/// Walk every `.md` file under `root` and return a
/// `WalkedFile` for each. The resolver is applied lazily by
/// the caller — `WalkedFile` carries the frontmatter so the
/// caller can decide entity kind based on disk location too
/// (e.g. "anything under `projects/` is a project").
pub fn walk_vault(root: &Path) -> Vec<WalkedFile> {
    let mut out = Vec::new();
    let walker = WalkDir::new(root).follow_links(false).into_iter();
    for entry in walker.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        // Skip Obsidian / vault metadata dirs by convention.
        if path
            .components()
            .any(|c| matches!(c.as_os_str().to_str(), Some(".obsidian" | ".trash")))
        {
            continue;
        }
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(err) => {
                tracing::warn!(%err, path = %path.display(), "read failed");
                continue;
            }
        };
        let (fm_str, body) = frontmatter::split(&content);
        let parsed = fm_str
            .map(frontmatter::parse_frontmatter)
            .unwrap_or_default();
        let body_links = parse_wikilinks(body);
        out.push(WalkedFile {
            path: path.to_path_buf(),
            frontmatter: parsed.clone(),
            frontmatter_emails: parsed.emails,
            body_emails: body_links.into_iter().map(|l| l.message_id).collect(),
        });
    }
    out
}

/// Convenience: walk + resolve + flatten into the shape
/// [`crate::LinkStore::rebuild_from`] expects. Files the
/// resolver returns `None` for are skipped.
pub fn collect_links(root: &Path, resolver: &EntityResolver) -> Vec<(EntityRef, Vec<String>)> {
    walk_vault(root)
        .into_iter()
        .filter_map(|f| {
            let entity = resolver(&f.path, &f.frontmatter)?;
            let ids = f.all_message_ids();
            if ids.is_empty() {
                return None;
            }
            Some((entity, ids))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, body: &str) {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p).unwrap();
        }
        std::fs::write(path, body).unwrap();
    }

    fn fixture_vault() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        write(
            &dir.path().join("projects/montreal-album.md"),
            "---\n\
             type: project\n\
             id: montreal-album\n\
             emails:\n  - <booking@studio.test>\n  - <master@studio.test>\n\
             ---\n\
             # Montreal Album\n\n\
             Mix v3 thread: [[email://mix@studio.test|Mix v3]]\n\
             Reply to booking: [[email://<booking@studio.test>]]\n",
        );
        write(
            &dir.path().join("tasks/finalize-master.md"),
            "---\n\
             type: task\n\
             emails:\n\
             - message_id: <master@studio.test>\n  subject: Master\n\
             ---\n\
             follow-up on the mastering quote\n",
        );
        write(
            &dir.path().join("notes/scratch.md"),
            "no frontmatter, just text\n",
        );
        write(&dir.path().join(".obsidian/config"), "ignore me\n");
        dir
    }

    #[test]
    fn walk_finds_md_files_only() {
        let dir = fixture_vault();
        let files = walk_vault(dir.path());
        let names: Vec<_> = files
            .iter()
            .filter_map(|f| f.path.file_name().and_then(|n| n.to_str()))
            .collect();
        assert!(names.contains(&"montreal-album.md"));
        assert!(names.contains(&"finalize-master.md"));
        assert!(names.contains(&"scratch.md"));
        // .obsidian/config has no .md extension; not picked up.
        assert!(!names.iter().any(|n| n.contains("config")));
    }

    #[test]
    fn all_message_ids_merges_and_dedupes() {
        let dir = fixture_vault();
        let files = walk_vault(dir.path());
        let project = files
            .iter()
            .find(|f| f.path.ends_with("montreal-album.md"))
            .unwrap();
        let ids = project.all_message_ids();
        // booking@ appears in BOTH frontmatter AND body — dedup
        // to one entry.
        assert_eq!(ids.len(), 3);
        assert!(ids.contains(&"booking@studio.test".to_string()));
        assert!(ids.contains(&"master@studio.test".to_string()));
        assert!(ids.contains(&"mix@studio.test".to_string()));
    }

    #[test]
    fn default_resolver_uses_frontmatter_kind_and_id() {
        let dir = fixture_vault();
        let files = walk_vault(dir.path());
        let project = files
            .iter()
            .find(|f| f.path.ends_with("montreal-album.md"))
            .unwrap();
        let entity = default_resolver(&project.path, &project.frontmatter).unwrap();
        assert_eq!(entity.kind.as_str(), "project");
        assert_eq!(entity.id, "montreal-album");
    }

    #[test]
    fn default_resolver_falls_back_to_file_stem() {
        let dir = fixture_vault();
        let files = walk_vault(dir.path());
        let task = files
            .iter()
            .find(|f| f.path.ends_with("finalize-master.md"))
            .unwrap();
        // No id: in frontmatter, so the stem wins.
        let entity = default_resolver(&task.path, &task.frontmatter).unwrap();
        assert_eq!(entity.kind.as_str(), "task");
        assert_eq!(entity.id, "finalize-master");
    }

    #[test]
    fn default_resolver_skips_files_without_kind() {
        let dir = fixture_vault();
        let files = walk_vault(dir.path());
        let scratch = files
            .iter()
            .find(|f| f.path.ends_with("scratch.md"))
            .unwrap();
        assert!(default_resolver(&scratch.path, &scratch.frontmatter).is_none());
    }

    #[test]
    fn collect_links_flattens_into_rebuild_shape() {
        let dir = fixture_vault();
        let pairs = collect_links(dir.path(), &default_resolver);
        assert_eq!(pairs.len(), 2);
        let project = pairs
            .iter()
            .find(|(e, _)| e.kind.as_str() == "project")
            .unwrap();
        assert_eq!(project.1.len(), 3);
        let task = pairs
            .iter()
            .find(|(e, _)| e.kind.as_str() == "task")
            .unwrap();
        assert_eq!(task.1.len(), 1);
    }
}
