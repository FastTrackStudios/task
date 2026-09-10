//! `project.md` — the file that makes a directory a project, and the
//! walk that finds every one of them.
//!
//! # A directory is a project when it says so
//!
//! `project.identity.declaration` is the rule, and it is short: *"A
//! project is a markdown document whose frontmatter declares
//! `type: project`. … A directory holding no such document is not a
//! project: it is unclassified content, which stays browsable and
//! adoptable."*
//!
//! On the Projects tier that document is `project.md` at the root of
//! the project's own directory. The convention is not new —
//! `ProjectService::adopt` has written `<dir>/project.md` into an
//! adopted tree since adoption existed — it is now simply where every
//! project's page lives rather than where an adopted one's did.
//!
//! # Why the walk stops at the page and not at the directory
//!
//! [`walk_projects`] descends the tier looking for `project.md` and does not
//! descend into anything else. Two consequences worth being explicit
//! about, because both are rules rather than optimisations:
//!
//! - **A directory without a page is not a project**, however
//!   project-shaped it looks. `Deliverables/`, `Track One/` before
//!   somebody promotes it, a folder of stems — none of them are
//!   projects, and the walk passing straight over them is the rule
//!   above being enforced rather than a heuristic being applied.
//!
//! - **A sub-project's directory is descended into anyway.** Finding
//!   `crescendum/project.md` does not stop the walk: `crescendum/
//!   track-two/project.md` is a project too, and
//!   `project.nesting.uniform` says *"there is one project entity and
//!   it nests without limit"*. Stopping at the first page found would
//!   make depth 1 special, which is the one thing that rule forbids.
//!
//! The walk is depth-limited all the same — see [`MAX_DEPTH`] — and
//! that limit is about a symlink loop, not about how deep a person may
//! nest their work.

use std::path::{Path, PathBuf};

/// The file that declares a directory to be a project.
pub const PROJECT_PAGE: &str = "project.md";

/// How far below the tier root [`walk_projects`] will look for a
/// project page.
///
/// Not a statement about how deeply projects may nest — the rule says
/// "without limit" and eight is more nesting than any real tree has.
/// It is a stop on a filesystem that lies: a symlink pointing at an
/// ancestor makes an infinite tree, and a walk with no floor turns that
/// into a hung boot rather than a wrong answer.
pub const MAX_DEPTH: usize = 8;

/// One project found on the tier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundProject {
    /// Tier-relative directory: `crescendum`, or
    /// `crescendum/track-two`. This is the string a reference carries
    /// and the string [`crate::OrgRoot::project_page`] takes.
    pub rel: String,
    /// The directory itself.
    pub dir: PathBuf,
}

impl FoundProject {
    /// The shelf this project's bytes belong to: the first segment of
    /// [`Self::rel`].
    ///
    /// A top-level project is its own shelf; a sub-project's shelf is
    /// its top-level ancestor's, because a sub-project is a subtree
    /// (see [`crate::OrgRoot::project_shelves`]).
    #[must_use]
    pub fn shelf(&self) -> &str {
        self.rel.split('/').next().unwrap_or(&self.rel)
    }

    /// Whether this is a top-level project — one whose whole directory
    /// is a shelf.
    #[must_use]
    pub fn is_top_level(&self) -> bool {
        !self.rel.contains('/')
    }
}

/// Whether `dir` holds a project page.
#[must_use]
pub fn is_project_dir(dir: &Path) -> bool {
    dir.join(PROJECT_PAGE).is_file()
}

/// Every project under `tier`, in a stable order.
///
/// Sorted by tier-relative path, so a parent always precedes its own
/// children and two machines holding the same tier enumerate it the
/// same way. Nothing here reads a page: this answers *where* the
/// projects are, and `project` parses *what* they say. Keeping those
/// apart is what lets this crate stay a layout resolver with no
/// opinion about frontmatter.
#[must_use]
pub fn walk_projects(tier: &Path) -> Vec<FoundProject> {
    let mut out = Vec::new();
    descend(tier, "", 0, &mut out);
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    out
}

fn descend(dir: &Path, prefix: &str, depth: usize, out: &mut Vec<FoundProject>) {
    if depth >= MAX_DEPTH {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        // A dot-directory is bookkeeping — `.fts-files`, `.git`, a
        // resource fork — and descending into one is how a walk finds a
        // version store's own contents and reports them as work.
        if name.starts_with('.') {
            continue;
        }
        let rel = if prefix.is_empty() {
            name
        } else {
            format!("{prefix}/{name}")
        };
        if is_project_dir(&path) {
            out.push(FoundProject {
                rel: rel.clone(),
                dir: path.clone(),
            });
        }
        // Descended into either way: a directory that is not a project
        // may still hold one (a `Songs/` folder somebody made), and a
        // directory that IS one certainly may — that is what a
        // sub-project is.
        descend(&path, &rel, depth + 1, out);
    }
}

/// The title a project's page declares, for callers that want the name
/// and not the page.
///
/// Used to register a project's tree as a File Root under the project's
/// own name, which is the convention `files_ui::review::locate_titled`
/// resolves a deliverable through. `None` for a directory with no
/// legible page.
///
/// A three-line frontmatter reader rather than a YAML dependency: this
/// crate is a layout resolver and may not take one, the key is a string
/// this workspace itself wrote, and the caller's fallback for every way
/// of failing is the same (use the directory name). Indented lines and
/// list items are skipped, so a nested block cannot be mistaken for a
/// top-level key.
#[must_use]
pub fn title_of(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(dir.join(PROJECT_PAGE)).ok()?;
    for line in text.lines() {
        if line.starts_with(char::is_whitespace) || line.starts_with('-') || line == "---" {
            continue;
        }
        let Some((k, v)) = line.split_once(':') else {
            continue;
        };
        if k.trim() != "title" {
            continue;
        }
        let v = v.trim().trim_matches('"').trim_matches('\'').trim();
        return (!v.is_empty()).then(|| v.to_owned());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn project(tier: &Path, rel: &str, title: &str) {
        let dir = tier.join(rel);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(PROJECT_PAGE),
            format!("---\ntype: project\ntitle: {title}\n---\n\nBody.\n"),
        )
        .unwrap();
    }

    /// t[verify project.nesting.uniform] — the walk finds a project at
    /// depth one and a project at depth two, and says nothing about
    /// them that differs except where they are. No level is special.
    #[test]
    fn the_walk_finds_projects_at_every_depth() {
        let tmp = tempfile::tempdir().unwrap();
        let tier = tmp.path();
        project(tier, "crescendum", "Crescendum");
        project(tier, "crescendum/track-two", "Track Two");
        project(tier, "crescendum/track-two/b-side", "B Side");
        project(tier, "first-single", "First Single");

        let found = walk_projects(tier);
        let rels: Vec<&str> = found.iter().map(|f| f.rel.as_str()).collect();
        assert_eq!(
            rels,
            [
                "crescendum",
                "crescendum/track-two",
                "crescendum/track-two/b-side",
                "first-single",
            ],
            "sorted, and a parent precedes its children"
        );

        // The shelf boundary: everything under one top-level project
        // belongs to that project's shelf, however deep it is.
        let shelves: Vec<&str> = found.iter().map(FoundProject::shelf).collect();
        assert_eq!(
            shelves,
            ["crescendum", "crescendum", "crescendum", "first-single"]
        );
        assert_eq!(
            found.iter().filter(|f| f.is_top_level()).count(),
            2,
            "two shelves, four projects"
        );
    }

    /// t[verify project.identity.declaration] — *"A directory holding
    /// no such document is not a project: it is unclassified content."*
    /// The tree is full of directories, and the pages are what make two
    /// of them projects.
    #[test]
    fn a_directory_without_a_page_is_not_a_project() {
        let tmp = tempfile::tempdir().unwrap();
        let tier = tmp.path();
        project(tier, "crescendum", "Crescendum");
        for plain in [
            "crescendum/Deliverables",
            "crescendum/Track One",
            "crescendum/Track One/Audio Files",
            "loose-folder/with/depth",
        ] {
            std::fs::create_dir_all(tier.join(plain)).unwrap();
        }
        // Bookkeeping the walk must not wander into.
        std::fs::create_dir_all(tier.join("crescendum/.fts-files/store")).unwrap();
        std::fs::write(tier.join("crescendum/.fts-files/store/project.md"), "x").unwrap();

        let rels: Vec<String> = walk_projects(tier).into_iter().map(|f| f.rel).collect();
        assert_eq!(rels, ["crescendum"]);
    }

    /// A symlink to an ancestor makes an infinite tree. The walk stops.
    #[test]
    fn the_walk_has_a_floor() {
        let tmp = tempfile::tempdir().unwrap();
        let tier = tmp.path();
        let mut deep = String::from("a");
        for _ in 0..(MAX_DEPTH + 4) {
            project(tier, &deep, "Deep");
            deep.push_str("/a");
        }
        let found = walk_projects(tier);
        assert_eq!(found.len(), MAX_DEPTH, "bounded, and did not hang");
    }

    #[test]
    fn a_projects_title_is_read_from_its_own_page() {
        let tmp = tempfile::tempdir().unwrap();
        project(tmp.path(), "crescendum", "Crescendum");
        assert_eq!(
            title_of(&tmp.path().join("crescendum")).as_deref(),
            Some("Crescendum")
        );
        assert_eq!(title_of(tmp.path()), None, "no page, no title");
    }
}
