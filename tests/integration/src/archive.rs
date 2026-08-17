//! One concept: a studio's disk, read as a tree.
//!
//! Two of them, and the same code reads both:
//!
//! - the **example vault** committed beside this crate, small enough for
//!   git and shaped exactly like the real thing;
//! - the **real archive**, wherever `TASK_ARCHIVE_ROOT` points, which is
//!   6 TB and belongs to nobody's checkout.
//!
//! Sharing the reader is the point. An example that drifted from the
//! archive it stands for would pass while telling you nothing, so the
//! only difference between the two is which directory is walked.

use std::path::{Path, PathBuf};

use files_domain::layout::{self, Entry};

/// The example data root, committed at `tests/integration/Data`.
///
/// `Data` rather than `vault`, because a vault is one of the things an
/// org *has* — beside its wiki, its assets, its inbox and its projects —
/// and naming the whole tree after one of its parts made the other four
/// look like exceptions.
///
/// Found relative to this file rather than to the working directory,
/// because `cargo` and `nextest` disagree about what that is.
#[must_use]
pub fn example_data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("Data")
}

/// The real archive, when this machine has one.
///
/// `None` unless `TASK_ARCHIVE_ROOT` is set to a directory that exists —
/// so a checkout without one runs the whole suite, and a machine with one
/// exercises it against several terabytes of folders nobody designed for
/// a test.
///
/// An env var rather than a hardcoded path: the archive is one person's
/// disk, and a test that names `/mnt/…` is a test that only ever runs for
/// them.
#[must_use]
pub fn real_archive() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("TASK_ARCHIVE_ROOT")?);
    root.is_dir().then_some(root)
}

/// Every org directory under `root`, in either layout.
///
/// The model is **org-relative**: it classifies paths inside one org and
/// says nothing about where orgs live. That is deliberate, and this
/// function is the reason it can be — locating orgs is a deployment
/// question with more than one right answer, and two are already in use:
///
/// - `<root>/<org>/Projects/…` — the layout the model describes, and the
///   one a server's data root already uses (`orgs/<slug>/`).
/// - `<root>/Projects/<org>/…` — how the archive this was written against
///   is arranged, from before there was a model.
///
/// A reader that only knew the first would find nothing on a real disk,
/// and a *model* that knew both would carry a migration around inside it
/// forever. So the model knows one shape and this knows where to start.
#[must_use]
pub fn org_roots(root: &Path) -> Vec<OrgRoot> {
    let mut found = Vec::new();

    // Legacy: the orgs are under `Projects/`, and each org's *contents*
    // are its projects — there is no second `Projects/` inside.
    let legacy = root.join("Projects");
    if legacy.is_dir() {
        if let Ok(entries) = std::fs::read_dir(&legacy) {
            for entry in entries.flatten() {
                let Ok(name) = entry.file_name().into_string() else {
                    continue;
                };
                if entry.file_type().is_ok_and(|t| t.is_dir())
                    && !files_domain::layout::is_inbox(&name)
                {
                    found.push(OrgRoot {
                        org: name,
                        dir: entry.path(),
                        projects: Projects::AtRoot,
                    });
                }
            }
        }
    }
    if !found.is_empty() {
        found.sort_by(|a, b| a.org.cmp(&b.org));
        return found;
    }

    // Current: an org is a top-level directory holding `Projects/`.
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if entry.file_type().is_ok_and(|t| t.is_dir()) && entry.path().join("Projects").is_dir()
            {
                found.push(OrgRoot {
                    org: name,
                    dir: entry.path(),
                    projects: Projects::InSubdir,
                });
            }
        }
    }
    found.sort_by(|a, b| a.org.cmp(&b.org));
    found
}

/// One org's directory, and how to read it.
#[derive(Debug, Clone)]
pub struct OrgRoot {
    pub org: String,
    pub dir: PathBuf,
    pub projects: Projects,
}

/// Where an org keeps its projects on disk.
///
/// The difference between the two archives in front of this suite, and
/// the reason it is *here* rather than in the model: a disk arranged
/// before there was a model is a migration, and a model that knew both
/// shapes would carry that migration around forever. So the model knows
/// one shape and this translates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projects {
    /// `<org>/Projects/<Project>` — the model's shape.
    InSubdir,
    /// `<org>/<Project>` — the legacy archive, where the org directory
    /// holds its projects directly.
    AtRoot,
}

/// One directory in a tree, and what the model makes of it.
#[derive(Debug, Clone)]
pub struct Read {
    /// Components relative to the tree root.
    pub parts: Vec<String>,
    pub entry: Entry,
}

impl Read {
    /// The path as a human wrote it, for a failure message.
    #[must_use]
    pub fn path(&self) -> String {
        self.parts.join("/")
    }
}

/// Walk `root` to `depth` directories deep and classify each one.
///
/// Depth-bounded because the interesting structure is in the top four
/// levels — org, project, session, media — and the levels below are a
/// session's own business, of which a real archive has millions.
#[must_use]
pub fn read_tree(root: &Path, depth: usize) -> Vec<Read> {
    let mut out = Vec::new();
    walk(root, &mut Vec::new(), depth, &mut out);
    out.sort_by(|a, b| a.parts.cmp(&b.parts));
    out
}

/// Read every org under `root`, whichever layout it is in.
///
/// Each org's contents are classified relative to that org, then reported
/// under its name — so a caller sees one flat list in the model's own
/// vocabulary regardless of how the disk happens to be arranged.
#[must_use]
pub fn read_orgs(root: &Path, depth: usize) -> Vec<Read> {
    let mut out = Vec::new();
    for OrgRoot { org, dir, projects } in org_roots(root) {
        out.push(Read {
            parts: vec![org.clone()],
            entry: files_domain::layout::Entry::Org(org.clone()),
        });
        for mut read in read_tree(&dir, depth) {
            // A legacy org's contents *are* its projects, so the level
            // the model expects has to be put back before classifying.
            // An inbox is the exception: `<org>/Inbox` is the org's, not
            // a project's, in either layout.
            let in_projects = projects == Projects::InSubdir
                || read
                    .parts
                    .first()
                    .is_some_and(|first| !files_domain::layout::is_inbox(first));
            if projects == Projects::AtRoot && in_projects {
                read.parts.insert(0, "Projects".to_string());
            }
            read.parts.insert(0, org.clone());
            let borrowed: Vec<&str> = read.parts.iter().map(String::as_str).collect();
            read.entry = files_domain::layout::classify(&borrowed);
            out.push(read);
        }
    }
    out.sort_by(|a, b| a.parts.cmp(&b.parts));
    out
}

fn walk(dir: &Path, parts: &mut Vec<String>, left: usize, out: &mut Vec<Read>) {
    if left == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        parts.push(name);
        let borrowed: Vec<&str> = parts.iter().map(String::as_str).collect();
        out.push(Read {
            parts: parts.clone(),
            entry: layout::classify(&borrowed),
        });
        walk(&entry.path(), parts, left - 1, out);
        parts.pop();
    }
}

/// Every session folder under `root` — the directories that would be
/// adopted as File Roots.
///
/// A session is a folder holding a DAW project file. That is a question
/// about contents rather than about names, which is why it lives here and
/// not in the domain.
#[must_use]
pub fn sessions(root: &Path, depth: usize) -> Vec<PathBuf> {
    let mut out = Vec::new();
    find_sessions(root, depth, &mut out);
    out.sort();
    out
}

fn find_sessions(dir: &Path, left: usize, out: &mut Vec<PathBuf>) {
    if left == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut holds_session = false;
    let mut subdirs = Vec::new();
    for entry in entries.flatten() {
        let Ok(kind) = entry.file_type() else { continue };
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        if kind.is_dir() {
            subdirs.push(entry.path());
        } else if layout::is_session_file(&name) {
            holds_session = true;
        }
    }
    if holds_session {
        out.push(dir.to_path_buf());
        // Don't descend: a session's subfolders are its media, not more
        // sessions. A REAPER project with a nested backup would otherwise
        // register twice.
        return;
    }
    for sub in subdirs {
        find_sessions(&sub, left - 1, out);
    }
}
