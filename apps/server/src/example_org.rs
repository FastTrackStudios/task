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
    let resources = org_root.resources_dir();

    // `Dir::files` is one level deep, so walk the whole subtree.
    let mut planted = Planted::default();
    plant(dir, slug, &vault, &wiki, &files, &resources, &mut planted)?;
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
    resources: &Path,
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
            // Deliverable media (and anything else the org serves over
            // `GET /org/{slug}/media/…`): the route reads the org's
            // `resources/` tree, so that is where these belong.
            Some("Resources") => resources.join(rel.strip_prefix("Resources").unwrap_or(rel)),
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
        plant(child, slug, vault, wiki, files, resources, planted)?;
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

/// A project the seeder DECLARES — a `ProjectInfo` page in the org
/// vault, which is what the app's Projects view lists.
///
/// Planting the trees alone left the app's Projects page empty, which
/// read as broken rather than as "adoption is your first move". The
/// declaration and the adoption are different acts on purpose — a
/// project *is* its page (`project.identity.declaration`); its session
/// trees become File Roots when someone adopts them — so the seeder
/// declaring these takes nothing away from the adoption half of the
/// demo. The trees under `files/Projects/` are still sitting there
/// unadopted.
pub struct DeclaredProject {
    pub org: &'static str,
    /// Directory name under `files/Projects/` — what `project_path`
    /// resolves, and where the parts (if any) are read from.
    pub dir: &'static str,
    pub title: &'static str,
    /// Rendered into the page body — clients are people in the story,
    /// not yet a field on the model.
    pub clients: &'static str,
    pub form: Option<project::Form>,
    pub capabilities: &'static [&'static str],
    /// The parts, in PLAYING order — "its songs are declared as parts"
    /// (`scenario.album.declare`), and the declaration's order is the
    /// album's running order, which a directory listing (alphabetical:
    /// One, Three, Two) cannot express. Each name should match a
    /// subdirectory of `dir` (its session tree) and, for audio
    /// deliverables, a committed song folder
    /// (`Resources/songs/<slug>/`) — `example_org` tests pin both.
    pub parts: &'static [&'static str],
    /// What the project owes: `(name, medium, scope, audience)`,
    /// declared through the real `declare_deliverable`. The media
    /// behind them lives under `Resources/deliverables/<project-slug>/`
    /// — the convention the app resolves an item's playback URL by.
    pub deliverables: &'static [(
        &'static str,
        project::Medium,
        project::Scope,
        project::Audience,
    )],
    /// A believable board: `(title, status, due in N days from plant)`.
    /// Statuses are the task model's slugs (`open`, `in-progress`,
    /// `done`).
    pub tasks: &'static [(&'static str, &'static str, Option<i64>)],
}

/// What the example studio's orgs have committed to. `Z - Duplicates`
/// and `tasks/` are deliberately NOT here: they are edge-case material
/// for other features, not projects anyone declared.
pub const DECLARED: &[DeclaredProject] = &[
    DeclaredProject {
        org: "acme-audio",
        dir: "Example Album",
        title: "Example Album",
        clients: "",
        form: Some(project::Form::Album),
        capabilities: &["music-production"],
        parts: &["Track One", "Track Two", "Track Three"],
        deliverables: &[(
            "Album master",
            project::Medium::Audio,
            project::Scope::PerPart,
            project::Audience::Client,
        )],
        tasks: &[
            ("Comp lead vocals — Track One", "in-progress", None),
            ("Mix revisions — Track Two", "open", Some(2)),
            ("Re-track drums — Track Three", "done", None),
            ("Master review with the label", "open", Some(7)),
            ("Sequence the album", "open", Some(10)),
        ],
    },
    DeclaredProject {
        org: "acme-audio",
        dir: "First Single - Example Client",
        title: "First Single",
        clients: "Example Client",
        form: Some(project::Form::Single),
        capabilities: &["music-production"],
        parts: &[],
        deliverables: &[
            (
                "Single master",
                project::Medium::Audio,
                project::Scope::WholeProject,
                project::Audience::Client,
            ),
            // Both media on one project: the master streams through the
            // global player, the lyric video opens in the page's player
            // — the pair every deliverable surface is exercised by.
            (
                "Lyric video",
                project::Medium::Video,
                project::Scope::WholeProject,
                project::Audience::Client,
            ),
        ],
        tasks: &[
            ("Deliver final master to Example Client", "done", None),
            ("Collect streaming metadata", "open", Some(1)),
            ("Cut the lyric video to the final master", "in-progress", None),
        ],
    },
    DeclaredProject {
        org: "vnt-video",
        dir: "Example Documentary - First Client, Second Client",
        title: "Example Documentary",
        clients: "First Client, Second Client",
        form: None,
        capabilities: &["video-production"],
        parts: &[],
        deliverables: &[(
            "Final cut",
            project::Medium::Video,
            project::Scope::WholeProject,
            project::Audience::Client,
        )],
        tasks: &[
            ("Rough cut review", "in-progress", None),
            ("Color grade — interview scenes", "open", Some(5)),
        ],
    },
    // The collaboration piece — the project the demo's federation story
    // shares across the ACME/VNT boundary — and deliberately the one
    // carrying BOTH media: an audio master (streams through the global
    // player) and a video cut (opens in the page's player), so one
    // project exercises every deliverable surface at once.
    DeclaredProject {
        org: "vnt-video",
        dir: "Shared Project",
        title: "Shared Project",
        clients: "",
        form: None,
        capabilities: &["music-production", "video-production"],
        parts: &[],
        deliverables: &[
            (
                "Live session recording",
                project::Medium::Audio,
                project::Scope::WholeProject,
                project::Audience::Client,
            ),
            (
                "Recap cut",
                project::Medium::Video,
                project::Scope::WholeProject,
                project::Audience::Client,
            ),
        ],
        tasks: &[
            ("Kickoff with ACME", "open", Some(3)),
            ("Sync the recap cut to the live recording", "in-progress", None),
        ],
    },
];

/// The declared projects of one org.
pub fn declared_of(slug: &str) -> impl Iterator<Item = &'static DeclaredProject> {
    DECLARED.iter().filter(move |p| p.org == slug)
}

/// An audio deliverable's song-folder slug — the same spelling the app
/// derives a playback queue entry from (`song_slug` in the project
/// detail page) and `examples/studio/tools/gen_audio.py` names its
/// folders with: lowercase, every non-alphanumeric run one dash.
#[must_use]
pub fn song_slug(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut dash = false;
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            dash = false;
        } else if !dash && !out.is_empty() {
            out.push('-');
            dash = true;
        }
    }
    out.trim_end_matches('-').to_string()
}

// The seed is a contract, and these tests are what keep it one: every
// declaration must be backed by the committed tree, or a demo plants a
// world where clicking the thing the seed promised does nothing. The
// failure mode is silent (an outstanding chip, an empty queue), so the
// suite fails instead.
#[cfg(test)]
mod declared_tests {
    use super::*;

    #[test]
    fn every_declared_project_has_its_tree() {
        for d in DECLARED {
            assert!(
                ORGS.iter().any(|(s, _)| *s == d.org),
                "{}: org `{}` is not in the example",
                d.title,
                d.org
            );
            assert!(
                STUDIO
                    .get_dir(format!("{}/Projects/{}", d.org, d.dir))
                    .is_some(),
                "{}: no committed tree at {}/Projects/{}",
                d.title,
                d.org,
                d.dir
            );
        }
    }

    #[test]
    fn every_part_is_a_directory_in_its_project_tree() {
        for d in DECLARED {
            for part in d.parts {
                assert!(
                    STUDIO
                        .get_dir(format!("{}/Projects/{}/{}", d.org, d.dir, part))
                        .is_some(),
                    "{}: part `{part}` has no session directory in the tree",
                    d.title
                );
            }
        }
    }

    /// Every audio deliverable resolves to a committed song folder —
    /// per part for a `PerPart` declaration, by the declaration's own
    /// name for a `WholeProject` one. This is the click-to-play
    /// contract: the app queues `songs/<slug>/` and the player reads
    /// its manifest. Video is exempt on purpose (generated at plant,
    /// never committed).
    #[test]
    fn every_audio_deliverable_has_a_committed_song() {
        let song = |org: &str, title: &str| {
            let slug = song_slug(title);
            STUDIO
                .get_file(format!("{org}/Resources/songs/{slug}/manifest.json"))
                .is_some()
        };
        for d in DECLARED {
            for (name, medium, scope, _) in d.deliverables {
                if *medium != project::Medium::Audio {
                    continue;
                }
                match scope {
                    project::Scope::PerPart => {
                        for part in d.parts {
                            assert!(
                                song(d.org, part),
                                "{}: no committed song for part `{part}` \
                                 (expected {}/Resources/songs/{}/manifest.json)",
                                d.title,
                                d.org,
                                song_slug(part)
                            );
                        }
                    }
                    project::Scope::WholeProject => {
                        assert!(
                            song(d.org, name),
                            "{}: no committed song for `{name}` \
                             (expected {}/Resources/songs/{}/manifest.json)",
                            d.title,
                            d.org,
                            song_slug(name)
                        );
                    }
                    project::Scope::Excerpt => {}
                }
            }
        }
    }

    #[test]
    fn every_declared_capability_is_in_the_vocabulary() {
        for d in DECLARED {
            let caps = project::Capabilities::from_names(d.capabilities.iter().copied());
            assert!(
                caps.unrecognised.is_empty(),
                "{}: capabilities outside the vocabulary: {:?}",
                d.title,
                caps.unrecognised
            );
        }
    }
}
