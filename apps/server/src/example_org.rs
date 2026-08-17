//! Planting `examples/studio` on a server's disk.
//!
//! The example tree is a studio's disk small enough to commit: two
//! companies, their projects, and every awkward name that broke the tree
//! reader when it was first pointed at a real 6 TB archive. See
//! `examples/studio/README.md` for what each folder is a case of.
//!
//! Until now it was read by two chapters of the integration suite and by
//! nothing else — as a *tree*, by the reader, never as an org anybody
//! could sign into. The scenario booted instead from eight files written
//! inline in the harness, which is why "the suite passes" and "there is
//! something to demo" were unrelated facts.
//!
//! # It is compiled in, not looked up
//!
//! `include_dir!`, so a `task-server` binary carries the example wherever
//! it runs. The alternative is a path relative to the source tree, which
//! is correct exactly until someone installs the binary — and the whole
//! point of this module is that `admin demo` works on a machine that has
//! never seen the repository.
//!
//! 368 KB across 53 files. Every byte that had to be large is generated
//! at run time instead; see `files.scale` and the suite's `scale`
//! chapter.
//!
//! # The mapping, and why it is here rather than in the tree
//!
//! `examples/studio/<org>/` is laid out the way a studio's disk is laid
//! out — `Projects/`, `Assets/`, `Inbox/`, `Vault/`, `Wiki/` — and an
//! org root on a server is laid out the way this product stores an org:
//! `vault/`, `wiki/`, `files/`, some sqlite. Those are not the same
//! shape, so something has to translate:
//!
//! | example | org root |
//! |---|---|
//! | `Vault/`   | `vault/` |
//! | `Wiki/`    | `wiki/Knowledge/` |
//! | everything else | `files/` |
//!
//! Keeping the translation here rather than reshaping the example is
//! deliberate, and the same call `archive::org_roots` makes in the suite
//! for the older on-disk layout: the example is a picture of a real
//! studio's disk, and a picture edited to match our storage layout would
//! stop being evidence of anything.

use std::path::Path;

use include_dir::{Dir, include_dir};

/// The committed example studio, compiled into the binary.
static STUDIO: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/../../examples/studio");

/// The orgs the example describes, in the order a demo should boot them.
///
/// ACME first: it is the one that owns the audio work, holds the
/// deliverables a client is shown, and is the home org in every
/// arrangement here.
pub const ORGS: &[(&str, &str)] = &[("acme-audio", "ACME Audio"), ("vnt-video", "VNT Video")];

/// Whether the example describes this org.
#[must_use]
pub fn has(slug: &str) -> bool {
    STUDIO.get_dir(slug).is_some()
}

/// Plant `slug`'s half of the example on an org root.
///
/// Idempotent by omission: a file already on disk is left exactly as it
/// is. Re-running tops up what is missing rather than reverting what
/// somebody has since edited — which matters because the whole premise
/// of the adoption chapter is that other applications keep writing this
/// tree.
///
/// # Errors
///
/// Any filesystem error creating a directory or writing a file.
pub fn install(org_root: &org_proto::OrgRoot, slug: &str) -> std::io::Result<Planted> {
    let Some(dir) = STUDIO.get_dir(slug) else {
        return Ok(Planted::default());
    };

    let vault = org_root.vault_dir();
    let wiki = org_root.wiki_knowledge_dir();
    let files = org_root.path().join("files");

    // `Dir::files` is one level deep, so walk the whole subtree.
    let mut planted = Planted::default();
    plant(dir, slug, &vault, &wiki, &files, &mut planted)?;
    Ok(planted)
}

/// What an install actually did.
#[derive(Debug, Default, Clone, Copy)]
pub struct Planted {
    /// Files written, because they were not there.
    pub written: usize,
    /// Files left alone, because they were.
    pub kept: usize,
}

fn plant(
    dir: &Dir<'_>,
    slug: &str,
    vault: &Path,
    wiki: &Path,
    files: &Path,
    planted: &mut Planted,
) -> std::io::Result<()> {
    for file in dir.files() {
        // Path inside the org, e.g. `Projects/Example Album/…`.
        let rel = file
            .path()
            .strip_prefix(slug)
            .unwrap_or_else(|_| file.path());
        let dest = match rel.iter().next().and_then(|s| s.to_str()) {
            // A vault is one of the things an org has, and on a server
            // it is the org's own `vault/` — not a folder inside its
            // files.
            Some("Vault") => vault.join(rel.strip_prefix("Vault").unwrap_or(rel)),
            Some("Wiki") => wiki.join(rel.strip_prefix("Wiki").unwrap_or(rel)),
            _ => files.join(rel),
        };
        if dest.exists() {
            planted.kept += 1;
            continue;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, file.contents())?;
        planted.written += 1;
    }
    for child in dir.dirs() {
        plant(child, slug, vault, wiki, files, planted)?;
    }
    Ok(())
}

// ── The cast ─────────────────────────────────────────────────────────
//
// Four accounts and what each was given. This lives here, beside the
// tree, because two things need it and neither can own it: the
// integration suite hires these people in `people.rs`, and `admin demo`
// creates them on a server you can sign into. Two lists would drift, and
// the drift would be invisible — a suite proving a client is refused at
// the session folder while the demo hands them the whole org reads as a
// passing suite either way.

/// What a person was given, as a name rather than a capability list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Holds {
    /// Everything, including the guest list.
    Owner,
    /// The work, but not the guest list.
    Employee,
    /// The mix, and an opinion about it.
    Client,
}

impl Holds {
    /// The capabilities this role carries.
    ///
    /// `Client` is the one worth reading twice: `Comment` without
    /// `Download` is the whole distinction between a client who can
    /// review a deliverable and a client who can keep it. `Employee` is
    /// an owner minus `Share`, which is what makes "an employee cannot
    /// widen the guest list" a property of the system rather than of the
    /// employee.
    #[must_use]
    pub fn capabilities(self) -> Vec<files::service::access::Capability> {
        use files::service::access::Capability::{Comment, Download, History, Read, Share, Write};
        match self {
            Self::Owner => vec![Read, Write, History, Comment, Download, Share],
            Self::Employee => vec![Read, Write, History, Comment, Download],
            Self::Client => vec![Read, Comment],
        }
    }
}

/// One person in the example.
#[derive(Debug, Clone, Copy)]
pub struct Member {
    pub email: &'static str,
    pub name: &'static str,
    /// The org whose auth store holds the account.
    pub org: &'static str,
    pub holds: Holds,
    /// The subtree they were granted, relative to the org's adopted
    /// roots. Empty means the whole of every root they were given.
    ///
    /// Casey's is `Deliverables`, and that one path is the difference
    /// between a client link and an org membership.
    pub scope: &'static str,
}

/// Everyone in the example, and what each was given.
pub const CAST: &[Member] = &[
    Member {
        email: "alice@acme.test",
        name: "Alice",
        org: "acme-audio",
        holds: Holds::Owner,
        scope: "",
    },
    Member {
        email: "victor@vnt.test",
        name: "Victor",
        org: "vnt-video",
        holds: Holds::Owner,
        scope: "",
    },
    Member {
        email: "sam@acme.test",
        name: "Sam",
        org: "acme-audio",
        holds: Holds::Employee,
        scope: "",
    },
    Member {
        email: "casey@client.test",
        name: "Casey",
        org: "acme-audio",
        holds: Holds::Client,
        scope: "Deliverables",
    },
];

/// The password every example account is created with.
///
/// One password, printed on every boot, protecting nothing. A demo whose
/// credentials have to be looked up is a demo nobody runs.
pub const PASSWORD: &str = "correct-horse-battery-staple";

/// The example's members of one org.
#[must_use]
pub fn cast_of(slug: &str) -> Vec<Member> {
    CAST.iter().filter(|m| m.org == slug).copied().collect()
}

/// Where a project of this org's example tree sits on disk.
///
/// The adoptable roots are under `files/Projects/`, so a caller that
/// wants to adopt "Example Album" needs this rather than a guess about
/// the layout.
#[must_use]
pub fn project_path(org_root: &org_proto::OrgRoot, name: &str) -> std::path::PathBuf {
    org_root.path().join("files").join("Projects").join(name)
}
