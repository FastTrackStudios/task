//! The org tree resolver (issue #304): ONE unified namespace over an
//! org's Projects / Vault / Wiki / Assets, consumed identically by
//! the explorer RPC and (wave 2) the WebDAV mount — the app and a
//! mounted network share always show the same tree.
//!
//! Area semantics:
//! - `Projects/` — a JOIN: every vault project folder (`Projects/*`
//!   and `Albums/*`), with a virtual `Media/` entry when a File Root
//!   is linked to the project — by id from the note's `media_roots:`
//!   frontmatter, falling back to name-matching for unlinked projects.
//!   Descending into `Media/` resolves to [`TreeNode::Root`] — the
//!   client mounts the full root explorer there.
//! - `Vault/`, `Wiki/` — the physical directory tree, straight
//!   through (no extra lens level; surfacing tags into the tree is a
//!   later exploration).
//! - `Assets/` — the org's loose files: the Files area with the
//!   registered root directories filtered out.

use std::path::{Path, PathBuf};

use files_proto::{BrowseEntry, TreeNode};

use crate::backend::FilesBackend;
use crate::error::Error;

/// The four top-level areas, in display order.
const AREAS: [&str; 4] = ["Projects", "Vault", "Wiki", "Assets"];

impl FilesBackend {
    pub(crate) fn tree_browse_inner(&self, path: String) -> Result<TreeNode, Error> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        // Confinement first: the tree serves virtual paths, but every
        // segment still walks real directories underneath.
        if segments.iter().any(|s| *s == "." || *s == "..") {
            return Err(Error::BadRequest(format!("{path}: path escapes")));
        }

        match segments.split_first() {
            None => Ok(TreeNode::Listing(
                AREAS.iter().map(|a| virtual_dir(a)).collect(),
            )),
            Some((&"Projects", rest)) => self.projects_area(rest),
            Some((&"Vault", rest)) => markdown_area(&self.vault_root_dir(), rest),
            Some((&"Wiki", rest)) => markdown_area(&self.wiki_root_dir(), rest),
            Some((&"Assets", rest)) => self.assets_area(rest),
            Some((other, _)) => Err(Error::NotFound(format!("{other}: no such area"))),
        }
    }

    /// The org's vault directory (the versions store knows it).
    fn vault_root_dir(&self) -> PathBuf {
        self.vault_root().to_path_buf()
    }

    /// The org's wiki directory — a sibling of the vault under the
    /// org dir (the server roots the wiki slice there too).
    fn wiki_root_dir(&self) -> PathBuf {
        self.vault_root()
            .parent()
            .map(|org| org.join("wiki"))
            .unwrap_or_else(|| self.vault_root().join("wiki"))
    }

    // ── Projects: the vault ⋈ roots join ─────────────────────────

    fn projects_area(&self, rest: &[&str]) -> Result<TreeNode, Error> {
        let vault = self.vault_root_dir();
        match rest.split_first() {
            // `Projects/` — every project folder, both homes.
            None => {
                let mut entries = Vec::new();
                for home in ["Projects", "Albums"] {
                    let Ok(dir) = confined_dir(&vault, &[home]) else {
                        continue;
                    };
                    for entry in std::fs::read_dir(&dir)? {
                        let entry = entry?;
                        if entry.file_type()?.is_dir() {
                            entries.push(virtual_dir(&entry.file_name().to_string_lossy()));
                        }
                    }
                }
                entries.sort_by(|a, b| a.name.cmp(&b.name));
                Ok(TreeNode::Listing(entries))
            }
            Some((project, rest)) => {
                let Some(home) = ["Projects", "Albums"]
                    .into_iter()
                    .find(|h| vault.join(h).join(project).is_dir())
                else {
                    return Err(Error::NotFound(format!("{project}: no such project")));
                };
                let project_dir = vault.join(home).join(project);
                let media_root = self.project_media_root(project);

                match rest.split_first() {
                    // `Projects/<name>/` — the project's own notes
                    // plus the virtual Media/ door to its root. A
                    // physical `Media` dir must not double the entry
                    // (duplicate names would also collide as Dioxus
                    // keys client-side).
                    None => {
                        let mut entries =
                            Self::list_dir(&confined_dir(&project_dir, &[])?, true, true)?;
                        if media_root.is_some() && !entries.iter().any(|e| e.name == "Media") {
                            entries.push(virtual_dir("Media"));
                            entries.sort_by(|a, b| a.name.cmp(&b.name));
                        }
                        Ok(TreeNode::Listing(entries))
                    }
                    // `Projects/<name>/Media[/…]` — the root's live
                    // tree when one is registered (the physical dir,
                    // if any, is shadowed by the handoff); a plain
                    // vault dir otherwise.
                    Some((&"Media", media_rest)) => match media_root {
                        Some(root) => Ok(TreeNode::Root {
                            id: root,
                            subpath: media_rest.join("/"),
                        }),
                        None => dir_node(&project_dir, rest),
                    },
                    // `Projects/<name>/<notes…>` — plain vault dirs.
                    Some(_) => dir_node(&project_dir, rest),
                }
            }
        }
    }

    /// The root behind a project's `Media/` door.
    ///
    /// **By id first**, from the project note's `media_roots:`
    /// frontmatter; by name only as a fallback for projects not yet
    /// linked.
    ///
    /// Name-matching alone was the original rule and it is too fragile
    /// to keep as the primary: rename either side and `Media/` silently
    /// empties, with no error to notice. Real material here has already
    /// broken it three ways — a folder carrying an invisible U+F022 from
    /// a Mac font, three spellings of one project, and a client's name
    /// spelled two ways inside a single project. An id survives all of
    /// it, and survives the renaming that sorting a migration inevitably
    /// involves.
    ///
    /// The link lives in the note because the vault is the
    /// human-editable source of truth and outlives the app; the registry
    /// is derived state.
    ///
    /// The frontmatter is a LIST because one project genuinely has
    /// several roots (a shoot with separate camera, session and
    /// deliverable piles), and the same root genuinely belongs to two
    /// projects (footage shared by a collaboration). Only the first
    /// resolvable entry becomes the `Media/` door; the rest are still
    /// the project's, and are what a richer media view would list.
    fn project_media_root(&self, project: &str) -> Option<uuid::Uuid> {
        let known = self.registry_list();
        for id in self.linked_media_roots(project) {
            if known.iter().any(|r| r.id == id) {
                return Some(id);
            }
        }
        // Not linked (or the link names a root this org no longer has):
        // fall back to the name rule so existing projects keep working
        // until they are linked.
        let album_name = format!("Album — {project}");
        known
            .into_iter()
            .find(|r| r.name == project || r.name == album_name)
            .map(|r| r.id)
    }

    /// Root ids declared by the project note's `media_roots:`
    /// frontmatter, in order.
    ///
    /// Silent on every failure — a missing note, unreadable file,
    /// malformed YAML or an unparseable uuid all yield "no links", which
    /// falls back to name-matching. A project must never become
    /// unbrowsable because someone hand-edited its frontmatter badly.
    fn linked_media_roots(&self, project: &str) -> Vec<uuid::Uuid> {
        let vault = self.vault_root_dir();
        let mut out = Vec::new();
        for home in ["Projects", "Albums"] {
            let dir = vault.join(home).join(project);
            // `<project>/<project>.md` is the folder-form note; the flat
            // form is `<project>.md` beside the folder.
            for candidate in [dir.join(format!("{project}.md")), dir.with_extension("md")] {
                let Ok(text) = std::fs::read_to_string(&candidate) else {
                    continue;
                };
                out.extend(media_roots_from_frontmatter(&text));
                if !out.is_empty() {
                    return out;
                }
            }
        }
        out
    }

    // ── Assets: loose files (the Files area minus root dirs) ─────

    fn assets_area(&self, rest: &[&str]) -> Result<TreeNode, Error> {
        let base = self.confine_root().to_path_buf();
        let dir = confined_dir(&base, rest)?;
        // Hide registered roots at EVERY depth — a root created in a
        // subdirectory surfaces through Projects/, never as loose
        // files. Two guards: the registry's canonical paths, and the
        // on-disk root marker (catches a root whose registered path
        // spelling differs from the canonical one).
        let root_dirs: Vec<PathBuf> = self
            .registry_list()
            .into_iter()
            .filter_map(|r| PathBuf::from(r.path).canonicalize().ok())
            .collect();
        let mut entries = Self::list_dir(&dir, true, true)?;
        entries.retain(|e| {
            if !e.is_dir {
                return true;
            }
            let full = dir.join(&e.name);
            if full.join(crate::consts::MARKER_FILE).exists() {
                return false;
            }
            match full.canonicalize() {
                Ok(canonical) => !root_dirs.iter().any(|r| r == &canonical),
                Err(_) => true,
            }
        });
        Ok(TreeNode::Listing(entries))
    }
}

// ── markdown areas (Vault, Wiki) ──────────────────────────────────

/// The physical directory tree, straight through. A missing area dir
/// (an org that never grew a wiki) is an empty listing, not an error.
fn markdown_area(base: &Path, rest: &[&str]) -> Result<TreeNode, Error> {
    if !base.is_dir() {
        if rest.is_empty() {
            return Ok(TreeNode::Listing(Vec::new()));
        }
        return Err(Error::NotFound(format!(
            "{}: not a directory",
            rest.join("/")
        )));
    }
    dir_node(base, rest)
}

/// A physical directory listing under `base`, `rest` segments deep.
fn dir_node(base: &Path, rest: &[&str]) -> Result<TreeNode, Error> {
    Ok(TreeNode::Listing(FilesBackend::list_dir(
        &confined_dir(base, rest)?,
        true,
        true,
    )?))
}

/// Resolve `rest` under `base` with REAL confinement: canonicalize
/// both and require the target to stay inside the base. The literal
/// `..` scan upstream catches lazy escapes; this catches symlinks —
/// a link inside the vault pointing at `~/.ssh` (synced content, a
/// shared volume) must not hand its listing to every org member.
/// Every sibling browse surface confines; the tree is no exception.
fn confined_dir(base: &Path, rest: &[&str]) -> Result<PathBuf, Error> {
    let canonical_base = base
        .canonicalize()
        .map_err(|e| Error::NotFound(format!("{}: {e}", base.display())))?;
    let mut dir = canonical_base.clone();
    for segment in rest {
        dir.push(segment);
    }
    let resolved = dir
        .canonicalize()
        .map_err(|_| Error::NotFound(format!("{}: not a directory", rest.join("/"))))?;
    if !resolved.starts_with(&canonical_base) {
        return Err(Error::BadRequest(format!(
            "{}: path escapes the area",
            rest.join("/")
        )));
    }
    if !resolved.is_dir() {
        return Err(Error::NotFound(format!(
            "{}: not a directory",
            rest.join("/")
        )));
    }
    Ok(resolved)
}

/// `media_roots:` from a note's YAML frontmatter.
///
/// Accepts the list form and a bare scalar, because someone linking one
/// root by hand will write the scalar and be right to expect it to work:
///
/// ```yaml
/// media_roots:
///   - 3f9c1e88-...
/// # or
/// media_roots: 3f9c1e88-...
/// ```
///
/// Anything that is not a parseable uuid is skipped rather than
/// failing the read — see [`FilesBackend::linked_media_roots`].
fn media_roots_from_frontmatter(text: &str) -> Vec<uuid::Uuid> {
    let Some(rest) = text.strip_prefix("---") else {
        return Vec::new();
    };
    let Some(end) = rest.find("\n---") else {
        return Vec::new();
    };
    let Ok(doc) = serde_yaml::from_str::<serde_yaml::Value>(&rest[..end]) else {
        return Vec::new();
    };
    let Some(field) = doc.get("media_roots") else {
        return Vec::new();
    };
    let parse = |v: &serde_yaml::Value| v.as_str().and_then(|s| s.trim().parse().ok());
    match field {
        serde_yaml::Value::Sequence(items) => items.iter().filter_map(parse).collect(),
        other => parse(other).into_iter().collect(),
    }
}

// ── entry constructors ────────────────────────────────────────────

fn virtual_dir(name: &str) -> BrowseEntry {
    BrowseEntry {
        name: name.to_string(),
        is_dir: true,
        size: None,
        stub: false,
        divergent: false,
    }
}

#[cfg(test)]
mod frontmatter_tests {
    use super::media_roots_from_frontmatter as parse;

    const ID: &str = "3f9c1e88-0000-4000-8000-000000000001";

    #[test]
    fn a_list_of_ids_is_read_in_order() {
        let ids = parse(&format!(
            "---\ntype: project\nmedia_roots:\n  - {ID}\n  - 3f9c1e88-0000-4000-8000-000000000002\n---\nbody\n"
        ));
        assert_eq!(ids.len(), 2);
        assert_eq!(ids[0].to_string(), ID);
    }

    #[test]
    fn a_bare_scalar_works_too() {
        // Someone linking one root by hand will write this, and being
        // strict about list-vs-scalar would only punish them for it.
        let ids = parse(&format!("---\nmedia_roots: {ID}\n---\n"));
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn a_note_without_the_field_links_nothing() {
        assert!(parse("---\ntype: project\ntitle: X\n---\nbody").is_empty());
        assert!(parse("no frontmatter at all").is_empty());
    }

    #[test]
    fn a_malformed_link_is_skipped_not_fatal() {
        // The point of falling back: a hand-edited note with a typo must
        // not make its project unbrowsable. The good id in the same list
        // still resolves.
        let ids = parse(&format!(
            "---\nmedia_roots:\n  - not-a-uuid\n  - {ID}\n---\n"
        ));
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0].to_string(), ID);
    }

    #[test]
    fn broken_yaml_yields_no_links_rather_than_an_error() {
        assert!(parse("---\nmedia_roots: [unclosed\n---\n").is_empty());
        assert!(parse("---\nno terminator\n").is_empty());
    }
}
