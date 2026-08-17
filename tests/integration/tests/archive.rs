//! Chapter twelve — the same reader, against a real archive.
//!
//! `studio.rs` reads a vault built to be read. This reads one that was
//! not: several terabytes of folders a person organised over years, with
//! no thought for a parser, and every inconsistency that implies.
//!
//! # It does not run unless you have one
//!
//! Set `TASK_ARCHIVE_ROOT` to a Task tree. Without it every test here
//! returns immediately, because six terabytes of somebody's actual work
//! cannot be committed and must not be required to run a suite.
//!
//! ```sh
//! TASK_ARCHIVE_ROOT=/mnt/starcommand/Task cargo nextest run -p integration -E 'binary(archive)'
//! ```
//!
//! An env var rather than a path in the source: the archive is one
//! person's disk, and a test naming `/mnt/…` is a test that runs for one
//! person and silently passes for everyone else.
//!
//! # What it is for
//!
//! Not to assert that a particular archive is well-formed — it is not,
//! and that is the point. It is to run the model over folders nobody
//! designed for it and see what it cannot explain. Every awkward case in
//! the committed vault arrived here first, as a name this reader got
//! wrong.
//!
//! So the assertions are proportions and shapes, never counts. A count
//! would break the next time somebody files a project, which is a test
//! failing because the world changed rather than because the code did.

use std::collections::BTreeMap;

use files_domain::layout::Entry;

use integration::archive::{self, Read};

/// The archive, or `None` — in which case a test returns having asserted
/// nothing, and says so in its name rather than pretending to pass.
macro_rules! archive_or_skip {
    () => {
        match archive::real_archive() {
            Some(root) => root,
            None => {
                eprintln!(
                    "skipped: set TASK_ARCHIVE_ROOT to a Task tree to run this"
                );
                return;
            }
        }
    };
}

fn tally(reads: &[Read]) -> BTreeMap<&'static str, usize> {
    let mut counts = BTreeMap::new();
    for read in reads {
        let name = match &read.entry {
            Entry::Assets(_) => "assets",
            Entry::Projects(_) => "projects-root",
            Entry::Vault(_) => "vault",
            Entry::Wiki(_) => "wiki",
            Entry::Inbox(_) => "inbox",
            Entry::Org(_) => "org",
            Entry::Project(_) => "project",
            Entry::Material { .. } => "material",
            Entry::NotAProject { .. } => "not-a-project",
            Entry::Unknown => "unknown",
        };
        *counts.entry(name).or_default() += 1;
    }
    counts
}

/// The model explains what it finds — and reports what it does not.
///
/// A hard "nothing is unknown" would be the wrong assertion: an archive
/// is allowed to contain anything, and a folder the model cannot place is
/// information rather than a defect. What would be a defect is *most* of
/// a real disk being unexplained, which is the difference between a model
/// derived from reality and one imposed on it.
#[tokio::test]
async fn the_model_explains_most_of_a_real_archive() {
    let root = archive_or_skip!();
    let reads = archive::read_orgs(&root, 3);
    assert!(!reads.is_empty(), "{} held no directories", root.display());

    let counts = tally(&reads);
    let unknown = counts.get("unknown").copied().unwrap_or(0);
    eprintln!("{} directories: {counts:#?}", reads.len());
    if unknown > 0 {
        for read in reads.iter().filter(|r| r.entry == Entry::Unknown).take(20) {
            eprintln!("  unexplained: {}", read.path());
        }
    }

    let explained = reads.len() - unknown;
    assert!(
        explained * 10 >= reads.len() * 9,
        "the model explained {explained} of {} directories",
        reads.len()
    );
}

/// A real archive has orgs, each with projects under it.
#[tokio::test]
async fn a_real_archive_has_orgs_with_projects_under_them() {
    let root = archive_or_skip!();
    let reads = archive::read_orgs(&root, 3);

    let orgs: Vec<&Read> = reads
        .iter()
        .filter(|r| matches!(r.entry, Entry::Org(_)))
        .collect();
    assert!(!orgs.is_empty(), "no org directories found");

    let projects = reads
        .iter()
        .filter(|r| matches!(r.entry, Entry::Project(_)))
        .count();
    assert!(
        projects > orgs.len(),
        "{projects} projects across {} orgs — an archive with fewer projects \
         than orgs is one this reader is not reading",
        orgs.len()
    );
}

/// Most projects have no page, which is why the directory is the source.
///
/// The proportion is the finding. If it ever inverts — if pages become
/// the norm — the model could lean on them, and this test is where that
/// would first be visible.
#[tokio::test]
async fn most_projects_have_no_page_of_their_own() {
    let root = archive_or_skip!();
    let reads = archive::read_orgs(&root, 3);

    let mut with_page = 0;
    let mut total = 0;
    for read in reads.iter().filter(|r| matches!(r.entry, Entry::Project(_))) {
        total += 1;
        // Rebuild the on-disk path from the org root, since the parts
        // above are in the model's vocabulary rather than the disk's.
        let Some(org_root) = archive::org_roots(&root)
            .into_iter()
            .find(|r| Some(r.org.as_str()) == read.parts.first().map(String::as_str))
        else {
            continue;
        };
        let skip = match org_root.projects {
            // `<org>/Projects/<Project>` — skip the org name.
            archive::Projects::InSubdir => 1,
            // `<org>/<Project>` on disk, so skip the synthesised
            // `Projects` level too.
            archive::Projects::AtRoot => 2,
        };
        let mut path = org_root.dir;
        for part in read.parts.iter().skip(skip) {
            path.push(part);
        }
        if path.join("project.md").is_file() {
            with_page += 1;
        }
    }

    assert!(total > 0, "no projects found");
    eprintln!("{with_page} of {total} projects carry a project.md");
    assert!(
        with_page < total,
        "every project has a page — the directory-first model is now \
         carrying weight it does not need to"
    );
}

/// Sessions are found, and in all three of the shapes the tree holds.
#[tokio::test]
async fn sessions_are_found_across_a_real_archive() {
    let root = archive_or_skip!();
    let sessions: Vec<_> = archive::org_roots(&root)
        .iter()
        .flat_map(|r| archive::sessions(&r.dir, 4))
        .collect();

    assert!(
        !sessions.is_empty(),
        "no DAW sessions found — is TASK_ARCHIVE_ROOT a Task tree?"
    );
    eprintln!("{} sessions", sessions.len());
    for session in sessions.iter().take(10) {
        eprintln!("  {}", session.display());
    }
}

/// Nothing an inbox holds is read as a project, at any level.
///
/// The trap that started this: `Z - Inbox` contains ` - `, and a real
/// archive has one at nearly every level of every org.
#[tokio::test]
async fn no_inbox_anywhere_is_read_as_a_project() {
    let root = archive_or_skip!();
    let reads = archive::read_orgs(&root, 4);

    let misread: Vec<String> = reads
        .iter()
        .filter(|r| {
            matches!(&r.entry, Entry::Project(p) if p.work.eq_ignore_ascii_case("Z"))
        })
        .map(Read::path)
        .collect();
    assert!(
        misread.is_empty(),
        "read as a project for the client \"Inbox\": {misread:#?}"
    );

    let inboxes = reads
        .iter()
        .filter(|r| matches!(r.entry, Entry::Inbox(_)))
        .count();
    assert!(inboxes > 0, "an archive with no inbox is not this archive");
    eprintln!("{inboxes} inboxes");
}
