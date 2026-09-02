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
//! | `Wikis/<Name>/` | `wikis/<slug>/` |
//! | `Repos/<name>/` | `repos/<name>/`, then `git init` + one commit |
//! | everything else | `files/` |
//!
//! `Wiki/` and `Wikis/` are both here because an org has one
//! long-standing curated tier (`wiki/Knowledge/`, which predates
//! multi-wiki and is the default wiki's home) and any number of named
//! wikis beside it. The slug is the directory name lowercased and
//! hyphenated — `Music Theory` plants to `wikis/music-theory/` and is
//! referenced as `acme.test/music-theory::Page`.
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
pub const ORGS: &[(&str, &str)] = &[
    ("acme-audio", "ACME Audio"),
    ("vnt-video", "VNT Video"),
    ("alice-personal", "Alice Personal"),
];

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
    let wikis = org_root.wikis_dir();
    let files = org_root.path().join("files");
    let resources = org_root.resources_dir();
    let repos = repos_dir(org_root);

    // `Dir::files` is one level deep, so walk the whole subtree.
    let mut planted = Planted::default();
    plant(
        dir,
        slug,
        &vault,
        &wiki,
        &wikis,
        &files,
        &resources,
        &repos,
        &mut planted,
    )?;
    #[cfg(feature = "plugin-wiki")]
    plant_repo_wikis(org_root, slug);
    #[cfg(feature = "plugin-wiki")]
    declare_wiki_configs(org_root, slug)?;
    Ok(planted)
}

/// Write each declared wiki's `_state/wiki.json` — its title and
/// visibility — where none exists yet (`wiki.access.visibility`).
///
/// Only where none exists: a config is the wiki's own declaration, and
/// somebody who narrowed Music Theory to unlisted on a planted root
/// must not find it public again after a replant. Editors are not
/// written here — they are account ids, and accounts exist only once
/// the cast is created (`demo_cli`), or hired by the suite.
#[cfg(feature = "plugin-wiki")]
fn declare_wiki_configs(org_root: &org_proto::OrgRoot, slug: &str) -> std::io::Result<()> {
    for declared in wikis_of(slug) {
        let wiki_slug = wiki_slug(declared.title);
        let root = org_root.named_wiki_dir(&wiki_slug);
        if !root.is_dir() || wiki_live::config::config_path(&root).exists() {
            continue;
        }
        let mut config = wiki_proto::config::WikiConfig::implicit(&wiki_slug);
        config.title = declared.title.to_owned();
        config.visibility = declared.visibility.into();
        wiki_live::config::save(&root, &config).map_err(std::io::Error::other)?;
    }
    Ok(())
}

#[cfg(feature = "plugin-wiki")]
impl From<Visibility> for wiki_proto::config::Visibility {
    fn from(v: Visibility) -> Self {
        match v {
            Visibility::Public => Self::Public,
            Visibility::Unlisted => Self::Unlisted,
            Visibility::Private => Self::Private,
        }
    }
}

/// Where the example's repositories are planted: `<org>/repos/`.
///
/// Org-local and outside `wikis/`, `files/` and `vault/`: a repository
/// the seed *creates* is neither a File Root somebody adopted nor a
/// wiki, and a directory of its own says so.
#[must_use]
pub fn repos_dir(org_root: &org_proto::OrgRoot) -> std::path::PathBuf {
    org_root.path().join("repos")
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
    wikis: &Path,
    files: &Path,
    resources: &Path,
    repos: &Path,
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
            // `Wikis/<Name>/…` → `wikis/<slug>/…`: one directory per
            // named wiki, slugged so the on-disk name matches the one
            // a reference carries.
            Some("Wikis") => {
                let inner = rel.strip_prefix("Wikis").unwrap_or(rel);
                let mut parts = inner.iter();
                match parts.next().and_then(|s| s.to_str()) {
                    Some(name) => wikis.join(wiki_slug(name)).join(parts.as_path()),
                    None => continue,
                }
            }
            // Deliverable media (and anything else the org serves over
            // `GET /org/{slug}/media/…`): the route reads the org's
            // `resources/` tree, so that is where these belong.
            Some("Resources") => resources.join(rel.strip_prefix("Resources").unwrap_or(rel)),
            // `Repos/<name>/…` → `repos/<name>/…`: plain files here;
            // `plant_repo_wikis` makes each a git repository afterwards.
            Some("Repos") => repos.join(rel.strip_prefix("Repos").unwrap_or(rel)),
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
        plant(
            child, slug, vault, wiki, wikis, files, resources, repos, planted,
        )?;
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
            (
                "Cut the lyric video to the final master",
                "in-progress",
                None,
            ),
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
            (
                "Sync the recap cut to the live recording",
                "in-progress",
                None,
            ),
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

/// A wiki's slug from its display name — the same slugging, named for
/// what it identifies.
///
/// `Music Theory` → `music-theory`, which is both the directory under
/// `<org>/wikis/` and the middle of every reference into it
/// (`acme.test/music-theory::Ionian`). Those two must agree, so they
/// come from one function rather than from a convention people
/// remember.
#[must_use]
pub fn wiki_slug(title: &str) -> String {
    song_slug(title)
}

// ── The wikis ────────────────────────────────────────────────────────

/// A wiki the seed DECLARES, and what it is there to demonstrate.
///
/// `features/wiki/spec/wiki.md` says an org holds a *set* of wikis and
/// that a vault is not one of them. A seed with a single wiki cannot
/// show the difference between those claims and the one-wiki world
/// that preceded them, so the example carries four across three orgs —
/// two owned by the studio, two personal, each at a different
/// visibility.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredWiki {
    /// The org that owns it. Personal wikis belong to a person's own
    /// org (`wiki.boundary.role`); there is no second ownership path.
    pub org: &'static str,
    /// Directory name under `<org>/Wikis/` in the example tree, and
    /// the wiki's display title.
    pub title: &'static str,
    /// Who may find it and who may subscribe (`wiki.access.visibility`).
    pub visibility: Visibility,
    /// One line on what this wiki exists in the seed to prove.
    pub demonstrates: &'static str,
}

/// Who may find a wiki, and who may subscribe to it.
///
/// The distinction between `Unlisted` and `Private` is a refusal, not
/// an absence — see `wiki.access.visibility`. Conflating them is the
/// mistake this enum exists to make impossible.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Visibility {
    /// Listed in discovery; anyone may subscribe.
    Public,
    /// Listed to nobody; anyone holding the reference may subscribe.
    Unlisted,
    /// Not listed, and a subscription from outside the owning org is
    /// refused.
    Private,
}

/// Every wiki the example plants.
pub const DECLARED_WIKIS: &[DeclaredWiki] = &[
    DeclaredWiki {
        org: "acme-audio",
        title: "Music Theory",
        visibility: Visibility::Public,
        demonstrates: "the target of a cross-wiki reference, and a block anchor \
                       (`Harmonic Series#^partials`) referenced from another wiki",
    },
    DeclaredWiki {
        org: "acme-audio",
        title: "Audio Production",
        visibility: Visibility::Public,
        demonstrates: "two wikis in one org referencing each other both ways, so the \
                       web is one web while each page keeps one owning wiki",
    },
    DeclaredWiki {
        org: "alice-personal",
        title: "Bible Study",
        visibility: Visibility::Private,
        demonstrates: "a wiki that annotates a Resource without writing into it — every \
                       page anchors to a VerseId, so it survives a translation swap",
    },
    DeclaredWiki {
        org: "alice-personal",
        title: "Cooking",
        visibility: Visibility::Unlisted,
        demonstrates: "a personal wiki in a person's own org, unlisted rather than \
                       private: absent from discovery, subscribable with the reference",
    },
];

/// The wiki the seed's Edit lane story is told on: the owner holds
/// Editor here, one request is open from a cast member without the
/// role, and one Editor change went through the lane
/// (`wiki.edit.request`, `wiki.edit.auto-approve`).
pub const EDIT_LANE_WIKI: &str = "music-theory";

/// The open request the employee has against [`EDIT_LANE_WIKI`].
/// Matched by title on a replant, so the seed never opens it twice.
pub const SEED_EDIT_REQUEST_TITLE: &str = "A way to hear the leading tone";

/// The owner's own change to [`EDIT_LANE_WIKI`], approved within the
/// lane. Matched by title on a replant.
pub const SEED_EDITOR_CHANGE_TITLE: &str = "Where the mode names come from";

/// The wikis this org declares.
pub fn wikis_of(slug: &str) -> impl Iterator<Item = &'static DeclaredWiki> + '_ {
    DECLARED_WIKIS.iter().filter(move |w| w.org == slug)
}

// ── The repo-sourced wikis ───────────────────────────────────────────

/// A wiki the seed declares over a repository (`wiki.source.repo`).
///
/// Unlike a [`DeclaredWiki`], its pages are not committed under
/// `Wikis/`: they are committed under `Repos/<repo>/<path>/`, the
/// seeder makes that folder a git repository at plant time, and the
/// wiki is *created over it* — the same call a person makes, so the
/// planted world exercises the real path rather than a copy of its
/// result.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredRepoWiki {
    /// The org that owns it.
    pub org: &'static str,
    /// Display title; the slug derives from it.
    pub title: &'static str,
    /// Directory under `Repos/` in the example tree, and under
    /// `<org>/repos/` on disk.
    pub repo: &'static str,
    /// Path inside the repository the wiki mirrors.
    pub path: &'static str,
    /// Who may find it and who may subscribe.
    pub visibility: Visibility,
    /// One paragraph on what it is for; becomes `purpose.md`.
    pub purpose: &'static str,
    /// One line on what this wiki exists in the seed to prove.
    pub demonstrates: &'static str,
}

/// The branch every planted repository commits on and every wiki
/// follows.
pub const REPO_BRANCH: &str = "main";

/// Every repo-sourced wiki the example plants.
pub const DECLARED_REPO_WIKIS: &[DeclaredRepoWiki] = &[DeclaredRepoWiki {
    org: "acme-audio",
    title: "Docs",
    repo: "task-docs",
    path: "docs",
    visibility: Visibility::Public,
    purpose: "The documentation for ACME's tooling, mirrored from the `docs/` folder of \
              the `task-docs` repository. The repository is the source of truth; this \
              wiki follows its `main` branch.",
    demonstrates: "a repo-sourced wiki over a small committed repository: the mirror \
                   tracks the branch, says which commit it reflects, and is a wiki in \
                   every other respect",
}];

/// The repo-sourced wikis this org declares.
pub fn repo_wikis_of(slug: &str) -> impl Iterator<Item = &'static DeclaredRepoWiki> + '_ {
    DECLARED_REPO_WIKIS.iter().filter(move |w| w.org == slug)
}

/// The slug a declared repo-sourced wiki plants under.
#[must_use]
pub fn repo_wiki_slug(w: &DeclaredRepoWiki) -> String {
    wiki_slug(w.title)
}

/// Where a declared repository is planted on disk.
#[must_use]
pub fn repo_path(org_root: &org_proto::OrgRoot, w: &DeclaredRepoWiki) -> std::path::PathBuf {
    repos_dir(org_root).join(w.repo)
}

/// Run `git` in `dir`; the error is its stderr.
#[cfg(feature = "plugin-wiki")]
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args([
            "-c",
            "user.name=ACME Seed",
            "-c",
            "user.email=seed@acme.test",
        ])
        .args(args)
        .current_dir(dir)
        .env("GIT_TERMINAL_PROMPT", "0")
        .output()
        .map_err(|e| format!("git {args:?}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_owned())
    }
}

/// Make the planted `Repos/` folders repositories, and create or
/// refresh the wikis declared over them.
///
/// Idempotent the way the tree planting is: a folder that is already a
/// repository gets a commit only if the top-up left it dirty, a wiki
/// that already exists is refreshed rather than recreated. Never fatal:
/// a machine without `git` plants everything else and says what it
/// skipped, the way a missing `ffmpeg` leaves video deliverables
/// outstanding.
#[cfg(feature = "plugin-wiki")]
fn plant_repo_wikis(org_root: &org_proto::OrgRoot, slug: &str) {
    use wiki_proto::service::registry::Registry as _;

    let declared: Vec<&DeclaredRepoWiki> = repo_wikis_of(slug).collect();
    if declared.is_empty() {
        return;
    }
    if !wiki_live::repo_source::git_on_path() {
        tracing::warn!(
            org.slug = %slug,
            "git is not on PATH: repo-sourced wikis not planted ({})",
            declared.iter().map(|w| w.title).collect::<Vec<_>>().join(", ")
        );
        return;
    }
    for w in declared {
        let repo = repo_path(org_root, w);
        if !repo.is_dir() {
            tracing::warn!(org.slug = %slug, repo = w.repo, "declared repository was not planted");
            continue;
        }
        let commit = if repo.join(".git").exists() {
            // A top-up may have added files; commit them so the wiki
            // sees them. An unchanged tree commits nothing.
            match git(&repo, &["status", "--porcelain"]) {
                Ok(s) if s.trim().is_empty() => Ok(()),
                Ok(_) => git(&repo, &["add", "-A"])
                    .and_then(|_| git(&repo, &["commit", "-q", "-m", "Seed top-up"]))
                    .map(drop),
                Err(e) => Err(e),
            }
        } else {
            git(&repo, &["init", "-q", "--initial-branch", REPO_BRANCH])
                .and_then(|_| git(&repo, &["add", "-A"]))
                .and_then(|_| git(&repo, &["commit", "-q", "-m", "Initial documentation"]))
                .map(drop)
        };
        if let Err(e) = commit {
            tracing::warn!(org.slug = %slug, repo = w.repo, "seed repository not committed: {e}");
            continue;
        }

        let wiki_slug = repo_wiki_slug(w);
        let wikis_dir = org_root.wikis_dir();
        let roots: std::collections::HashMap<String, std::path::PathBuf> =
            org_root.named_wikis().into_iter().collect();
        let backend = wiki_live::WikiBackend::with_roots_under(roots, wikis_dir);
        let outcome = if org_root.named_wiki_dir(&wiki_slug).is_dir() {
            backend.refresh_source(&wiki_slug).map(|s| s.commit)
        } else {
            backend
                .create_wiki(wiki_proto::config::NewWiki {
                    title: w.title.to_owned(),
                    slug: wiki_slug.clone(),
                    purpose: w.purpose.to_owned(),
                    visibility: match w.visibility {
                        Visibility::Public => wiki_proto::config::Visibility::Public,
                        Visibility::Unlisted => wiki_proto::config::Visibility::Unlisted,
                        Visibility::Private => wiki_proto::config::Visibility::Private,
                    },
                    source: Some(wiki_proto::config::RepoSource {
                        url: format!("file://{}", repo.display()),
                        branch: REPO_BRANCH.to_owned(),
                        path: w.path.to_owned(),
                        ..Default::default()
                    }),
                })
                .and_then(|_| backend.config_of(&wiki_slug))
                .map(|c| c.source.map(|s| s.commit).unwrap_or_default())
        };
        match outcome {
            Ok(commit) if !commit.is_empty() => {}
            Ok(_) => tracing::warn!(
                org.slug = %slug,
                wiki.slug = %wiki_slug,
                "repo-sourced wiki planted but its first sync did not land a commit"
            ),
            Err(e) => tracing::warn!(
                org.slug = %slug,
                wiki.slug = %wiki_slug,
                "repo-sourced wiki not planted: {e}"
            ),
        }
    }
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
    fn every_declared_wiki_has_its_tree() {
        for w in DECLARED_WIKIS {
            assert!(
                ORGS.iter().any(|(s, _)| *s == w.org),
                "{}: org `{}` is not in the example",
                w.title,
                w.org
            );
            assert!(
                STUDIO
                    .get_dir(format!("{}/Wikis/{}", w.org, w.title))
                    .is_some(),
                "{}: no committed tree at {}/Wikis/{}",
                w.title,
                w.org,
                w.title
            );
        }
    }

    /// A wiki says what it is for. `purpose.md` is the one file
    /// `wiki-proto`'s schema layer will not invent, and a wiki without
    /// it plants as an unexplained pile of pages.
    #[test]
    fn every_declared_wiki_states_its_purpose() {
        for w in DECLARED_WIKIS {
            assert!(
                STUDIO
                    .get_file(format!("{}/Wikis/{}/purpose.md", w.org, w.title))
                    .is_some(),
                "{}: no purpose.md — a wiki that cannot say what it is for is a folder",
                w.title
            );
        }
    }

    /// The slug is load-bearing in two places that must agree: the
    /// directory the seeder plants to, and the middle of every
    /// reference into the wiki. A title that slugs to nothing, or to
    /// the same thing as its neighbour, breaks both silently.
    #[test]
    fn wiki_slugs_are_distinct_within_an_org() {
        for (org, _) in ORGS {
            let mut seen: Vec<String> = Vec::new();
            for w in wikis_of(org) {
                let slug = wiki_slug(w.title);
                assert!(!slug.is_empty(), "{}: title slugs to nothing", w.title);
                assert!(
                    !seen.contains(&slug),
                    "{org}: two wikis both slug to `{slug}`"
                );
                seen.push(slug);
            }
        }
    }

    /// A repo-sourced wiki's pages are committed under `Repos/`, not
    /// `Wikis/`: the repository must exist in the tree, the mirrored
    /// path must hold markdown, and something must sit *outside* that
    /// path or the seed cannot show that only the path is mirrored.
    #[test]
    fn every_declared_repo_wiki_has_its_repository() {
        for w in DECLARED_REPO_WIKIS {
            assert!(
                ORGS.iter().any(|(s, _)| *s == w.org),
                "{}: org `{}` is not in the example",
                w.title,
                w.org
            );
            let repo = format!("{}/Repos/{}", w.org, w.repo);
            let docs = STUDIO
                .get_dir(format!("{repo}/{}", w.path))
                .unwrap_or_else(|| panic!("{}: no committed tree at {repo}/{}", w.title, w.path));
            assert!(
                docs.files()
                    .any(|f| f.path().extension().is_some_and(|e| e == "md")),
                "{}: `{repo}/{}` holds no markdown",
                w.title,
                w.path
            );
            assert!(
                STUDIO.get_file(format!("{repo}/README.md")).is_some(),
                "{}: `{repo}` needs a README outside `{}` so the subpath rule is visible",
                w.title,
                w.path
            );
            assert!(!w.purpose.trim().is_empty(), "{}: no purpose", w.title);
            let slug = repo_wiki_slug(w);
            assert!(
                !wikis_of(w.org).any(|other| wiki_slug(other.title) == slug),
                "{}: slug `{slug}` collides with a committed wiki",
                w.title
            );
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
