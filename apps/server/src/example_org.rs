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
    #[cfg(feature = "plugin-fasttrackstudio")]
    plant_collections(org_root, slug);
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
/// that preceded them, so the example carries five across three orgs —
/// three owned by the studio, two personal, spanning all three
/// visibilities.
///
/// Two of the studio's three are a *pair*: `Audio Production` is
/// curated and `Studio Research` is the agent's working wiki that feeds
/// it. A promotion between them is the seam, and it is only testable
/// from a planted world that holds both — see [`PROMOTION_PAIR`].
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
        org: "acme-audio",
        title: "Studio Research",
        visibility: Visibility::Private,
        demonstrates: "the working half of a curated/working pair: an agent's unvetted \
                       wiki whose pages reach Audio Production only by promotion, which \
                       copies rather than moves and refuses a type the target's schema \
                       does not declare",
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

/// The seed's curated/working pair, as `(working, curated)` slugs.
///
/// `wiki.promote.*` is a rule about two wikis at once, so unlike every
/// other wiki rule it cannot be exercised against a single planted
/// wiki. This names the pair once, so the suite and the demo agree on
/// which two wikis the story is told on.
pub const PROMOTION_PAIR: (&str, &str) = ("studio-research", "audio-production");

/// The page on the working wiki that is ready to promote: its type
/// (`concept`) is one the curated wiki declares, so it crosses over
/// with no override.
pub const SEED_PROMOTABLE_PAGE: &str = "Concepts/Dynamic Range.md";

/// The page on the working wiki that must NOT promote as it stands: its
/// type is `question`, which the curated wiki does not declare. The
/// refusal is the feature, so the seed carries the case that triggers
/// it.
pub const SEED_UNPROMOTABLE_PAGE: &str = "Questions/Do small speakers need a different master.md";

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

// ── The resource-tier assets (ADR 0003) ──────────────────────────────

/// One asset the example plants under `<org>/resources/`.
///
/// ADR 0003 gives four kinds a home there — a Keyflow chart, a Signal
/// patch, a Signal sample, an Ignition lighting document — and says a
/// *library* of any of them is an ordinary `Collection` of kind
/// `Library` over `<kind>:<slug>` references. That claim is only
/// checkable from a planted world if the planted world actually holds
/// one of each, so it does.
///
/// Planting needs no code: [`plant`] copies `Resources/**` into the
/// org's `resources/` tier verbatim. What this declaration buys is the
/// contract — [`declared_tests`] fails if a declared asset has no
/// committed tree, or if its directory name drifts from the one
/// `node_homes::library_of` resolves cross-org references through.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredAsset {
    /// The org that holds it.
    pub org: &'static str,
    /// The directory under `resources/` — and therefore the
    /// subscription slug a cross-org reader names. Fixed by
    /// `node_homes::library_of`: `charts`, `patches`, `samples`,
    /// `lighting`.
    pub library: &'static str,
    /// The node id: `chart:<slug>`, `patch:<slug>`, and so on.
    pub slug: &'static str,
    /// Path of the manifest under `<org>/Resources/<library>/` in the
    /// committed tree. A chart is flat (`<slug>.md`); the other kinds
    /// own a directory (`<slug>/patch.md`).
    pub manifest: &'static str,
    /// The file holding the asset's own document, beside the manifest.
    pub body: &'static str,
    /// One line on what this asset exists in the seed to prove.
    pub demonstrates: &'static str,
}

/// Every resource-tier asset the example plants. One of each kind, in
/// the studio org, tied to the album the rest of the seed is about.
pub const DECLARED_ASSETS: &[DeclaredAsset] = &[
    DeclaredAsset {
        org: "acme-audio",
        library: "charts",
        slug: "track-one",
        manifest: "track-one.md",
        body: "track-one.kf",
        demonstrates: "a Keyflow chart kept in Task between sessions — the `.kf` is the \
                       chart, an outside editor opens it, and `chart:track-one#chorus` \
                       addresses a section; it is also `song:track-one`'s *default* \
                       arrangement, the one a caller gets when it names no arrangement",
    },
    DeclaredAsset {
        org: "acme-audio",
        library: "charts",
        slug: "track-one-condensed-live",
        manifest: "track-one-condensed-live.md",
        body: "track-one-condensed-live.kf",
        demonstrates: "one chart is one arrangement: a second reading of the *same* song, \
                       joined to it by `song: song:track-one` and told apart by its \
                       `arrangement` label — not a revision of the chart beside it, and not \
                       the default, so a demo user can see both halves of the invariant in \
                       the planted world",
    },
    DeclaredAsset {
        org: "acme-audio",
        library: "charts",
        slug: "track-two",
        manifest: "track-two.md",
        body: "track-two.kf",
        demonstrates: "the second chart, because one chart is a file and two are a \
                       library — `Chart Library` in the seed collects both, and a demo \
                       user opening the planted org finds a list rather than an orphan",
    },
    DeclaredAsset {
        org: "acme-audio",
        library: "lighting",
        slug: "track-one-lights",
        manifest: "track-one-lights/show.md",
        body: "track-one-lights/show.json",
        demonstrates: "lighting scoped to a single song, whose cues follow that song's \
                       chart sections — the ADR's join, planted: `song:track-one` carries \
                       `chart:track-one` and `lighting:track-one-lights`, and neither app \
                       knows the other exists",
    },
    DeclaredAsset {
        org: "acme-audio",
        library: "patches",
        slug: "warm-analog-pad",
        manifest: "warm-analog-pad/patch.md",
        body: "warm-analog-pad/patch.json",
        demonstrates: "a Signal patch as a directory, because a patch grows sidecars — and \
                       one with nothing bound to a File Root, which is the ordinary state \
                       of a declared asset",
    },
    DeclaredAsset {
        org: "acme-audio",
        library: "samples",
        slug: "room-kick-48k",
        manifest: "room-kick-48k/sample.md",
        body: "room-kick-48k/sample.json",
        demonstrates: "a sample whose audio is deliberately NOT here: the manifest says what \
                       the sample is, and the bytes belong in a File Root — which is what \
                       keeps subscribing to a sample library cheap",
    },
    DeclaredAsset {
        org: "acme-audio",
        library: "lighting",
        slug: "album-launch-show",
        manifest: "album-launch-show/show.md",
        body: "album-launch-show/show.json",
        demonstrates: "an Ignition show, scoped `show` rather than `song` or `setlist`, \
                       whose declared cues are the only things \
                       `lighting:album-launch-show#cue:12` can address",
    },
];

/// The resource-tier assets this org declares.
pub fn assets_of(slug: &str) -> impl Iterator<Item = &'static DeclaredAsset> + '_ {
    DECLARED_ASSETS.iter().filter(move |a| a.org == slug)
}

// ── The collections that gather them (ADR 0003) ──────────────────────

/// One ordered collection the example plants.
///
/// ADR 0003's third decision is that *nothing new is built for
/// libraries*: a library is a `Collection` of kind `Library` over node
/// references, and a setlist is the same primitive with a different
/// kind. A seed that planted four assets and no collection would leave
/// that claim unillustrated — a demo user would find four orphans and
/// no way to see them as a library — so the planted world holds both.
///
/// Unlike the assets, these are not files in the committed tree: a
/// collection lives in the org's `collections.jsonl`, written through
/// the real store at plant time by [`plant_collections`], which is also
/// what keeps a replant from planting a second copy.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredCollection {
    /// The org that holds it.
    pub org: &'static str,
    /// Display title, and the identity a replant matches on.
    pub title: &'static str,
    /// `library`, `setlist`, `show` or `playlist` —
    /// `CollectionKind::as_str`'s own spelling.
    pub kind: &'static str,
    /// The nodes it holds, in order: `(kind, id)` — the two halves of a
    /// `kind:id` reference. No domain: every item here is this org's
    /// own, and a cross-org item is what the suite's `setlist` chapter
    /// builds rather than something a single planted org can show.
    pub items: &'static [(&'static str, &'static str)],
    /// One line on what this collection exists in the seed to prove.
    pub demonstrates: &'static str,
}

/// Every collection the example plants.
pub const DECLARED_COLLECTIONS: &[DeclaredCollection] = &[
    DeclaredCollection {
        org: "acme-audio",
        title: "Chart Library",
        kind: "library",
        items: &[
            ("chart", "track-one"),
            ("chart", "track-one-condensed-live"),
            ("chart", "track-two"),
        ],
        demonstrates: "a chart library is a `Collection` of kind `Library` over \
                       `chart:<slug>` references and nothing else — no chart service, \
                       no chart store, no second vocabulary — and it holds both \
                       arrangements of Track One, because a library lists charts and one \
                       song has as many as it is played ways",
    },
    DeclaredCollection {
        org: "acme-audio",
        title: "Album Launch Set",
        kind: "setlist",
        items: &[
            ("song", "track-one"),
            ("chart", "track-one"),
            ("lighting", "track-one-lights"),
            ("song", "track-two"),
            ("chart", "track-two"),
            ("lighting", "album-launch-show"),
        ],
        demonstrates: "the sentence the whole decision exists for: a performance \
                       assembled by reference out of several libraries at once — the \
                       song, the chart it is played from, and the cues that run over it \
                       — with no app knowing the others are there",
    },
];

/// The collections this org declares.
pub fn collections_of(slug: &str) -> impl Iterator<Item = &'static DeclaredCollection> + '_ {
    DECLARED_COLLECTIONS.iter().filter(move |c| c.org == slug)
}

/// Where an org's ordered collections are stored.
///
/// **The one definition.** `AppState` opens the store here and the
/// seeder writes it here; two spellings of the same path would plant a
/// world the server then does not read, and the failure would be an
/// empty library rather than an error.
#[must_use]
pub fn collections_path(org_root: &org_proto::OrgRoot) -> std::path::PathBuf {
    std::env::var("TASK_SERVER_COLLECTIONS_PATH").map_or_else(
        |_| org_root.path().join("collections.jsonl"),
        std::path::PathBuf::from,
    )
}

/// Plant the declared collections, matching an existing one by title.
///
/// Idempotent the way the rest of the plant is: a collection already
/// there is left exactly as it is, items and order included, because
/// somebody may have reordered it since. Missing ones are created
/// through the real store, so what a demo opens is what the lane
/// serves.
#[cfg(feature = "plugin-fasttrackstudio")]
fn plant_collections(org_root: &org_proto::OrgRoot, slug: &str) {
    use collection::{CollectionKind, CollectionService as _, NodeKind, NodeRef, Placement};

    let declared: Vec<&DeclaredCollection> = collections_of(slug).collect();
    if declared.is_empty() {
        return;
    }
    let store = collection::Store::open(collections_path(org_root));
    let held = match store.list(slug.to_owned(), None) {
        Ok(held) => held,
        Err(e) => {
            tracing::warn!(org.slug = %slug, "collections not planted: {e}");
            return;
        }
    };
    for d in declared {
        if held.iter().any(|c| c.title == d.title) {
            continue;
        }
        let kind = match d.kind {
            "library" => CollectionKind::Library,
            "setlist" => CollectionKind::Setlist,
            "show" => CollectionKind::Show,
            "playlist" => CollectionKind::Playlist,
            other => CollectionKind::Other(other.to_owned()),
        };
        let made = match store.create(slug.to_owned(), d.title.to_owned(), kind) {
            Ok(made) => made,
            Err(e) => {
                tracing::warn!(org.slug = %slug, collection = d.title, "not created: {e}");
                continue;
            }
        };
        for (kind, id) in d.items {
            let Some(kind) = NodeKind::parse(kind) else {
                tracing::warn!(collection = d.title, "`{kind}` is not a node kind");
                continue;
            };
            if let Err(e) = store.add_item(Placement {
                collection_id: made.id.clone(),
                node: NodeRef::new(kind, *id),
                after: None,
            }) {
                tracing::warn!(collection = d.title, item = id, "not collected: {e}");
            }
        }
    }
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

    /// A promotion is checked against the TARGET wiki's declared page
    /// types, so a curated wiki with no committed `schema.md` cannot be
    /// promoted into at all — the verb would refuse before it ever
    /// reached the interesting part. The pair's two schemas are as
    /// load-bearing as its pages.
    #[test]
    fn the_promotion_pair_commits_both_schemas() {
        let (working, curated) = PROMOTION_PAIR;
        for slug in [working, curated] {
            let title = DECLARED_WIKIS
                .iter()
                .find(|w| wiki_slug(w.title) == slug)
                .unwrap_or_else(|| panic!("`{slug}` is not a declared wiki"));
            assert!(
                STUDIO
                    .get_file(format!("{}/Wikis/{}/schema.md", title.org, title.title))
                    .is_some(),
                "{slug}: no schema.md — a promotion has nothing to check against",
            );
        }
    }

    /// The two halves of the promotion story: one page whose type the
    /// curated wiki declares (it promotes), one whose type it does not
    /// (it is refused). Without the second the seed can only show the
    /// happy path, and the refusal is the feature.
    #[test]
    fn the_promotion_pair_commits_a_promotable_and_an_unpromotable_page() {
        let (working, curated) = PROMOTION_PAIR;
        let working_title = DECLARED_WIKIS
            .iter()
            .find(|w| wiki_slug(w.title) == working)
            .expect("the working wiki is declared");
        let read = |page: &str| {
            STUDIO
                .get_file(format!(
                    "{}/Wikis/{}/{page}",
                    working_title.org, working_title.title
                ))
                .unwrap_or_else(|| panic!("{working}: no committed `{page}`"))
                .contents_utf8()
                .unwrap_or_default()
                .to_owned()
        };

        let curated_schema = STUDIO
            .get_file(format!("acme-audio/Wikis/Audio Production/schema.md"))
            .expect("the curated schema is committed")
            .contents_utf8()
            .unwrap_or_default();
        let declared: Vec<String> = wiki_proto::promote::declared_types(curated_schema)
            .into_iter()
            .map(|t| t.name)
            .collect();

        let ok = read(SEED_PROMOTABLE_PAGE);
        assert!(
            ok.contains("type: concept") && declared.iter().any(|t| t == "concept"),
            "{SEED_PROMOTABLE_PAGE} must carry a type `{curated}` declares; it holds {declared:?}"
        );
        let refused = read(SEED_UNPROMOTABLE_PAGE);
        assert!(
            refused.contains("type: question") && !declared.iter().any(|t| t == "question"),
            "{SEED_UNPROMOTABLE_PAGE} must carry a type `{curated}` does NOT declare — \
             it is the seed's refusal case"
        );
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

    /// Every declared asset has its committed tree — the manifest and
    /// the document beside it — under the library directory ADR 0003
    /// names.
    #[test]
    fn every_declared_asset_has_its_committed_files() {
        for a in DECLARED_ASSETS {
            assert!(
                ORGS.iter().any(|(slug, _)| *slug == a.org),
                "{}: org `{}` is not one the example plants",
                a.slug,
                a.org
            );
            for file in [a.manifest, a.body] {
                let path = format!("{}/Resources/{}/{}", a.org, a.library, file);
                assert!(
                    STUDIO.get_file(&path).is_some(),
                    "{}: `{path}` is not committed — {}",
                    a.slug,
                    a.demonstrates
                );
            }
        }
    }

    /// The library directory is not decoration: it is the subscription
    /// slug a cross-org reader names, and `node_homes::library_of` is
    /// what maps a node kind onto it. A drift here is a cross-org
    /// reference that silently stops resolving, so the two are pinned
    /// against each other rather than kept in step by hand.
    #[test]
    fn every_asset_library_is_a_home_some_node_kind_resolves_through() {
        use links::NodeKind;
        for a in DECLARED_ASSETS {
            let known = [
                NodeKind::Song,
                NodeKind::Sermon,
                NodeKind::Video,
                NodeKind::Chart,
                NodeKind::Patch,
                NodeKind::Sample,
                NodeKind::Lighting,
            ]
            .into_iter()
            .any(|k| crate::node_homes::library_of(k) == Some(a.library));
            assert!(
                known,
                "{}: `resources/{}/` is not any node kind's home",
                a.slug, a.library
            );
        }
    }

    /// A declared asset's slug is its node id, so two of one kind may
    /// not share it — `patch:lead` must mean one patch.
    #[test]
    fn asset_slugs_are_distinct_within_a_library() {
        for a in DECLARED_ASSETS {
            assert_eq!(
                DECLARED_ASSETS
                    .iter()
                    .filter(|o| o.org == a.org && o.library == a.library && o.slug == a.slug)
                    .count(),
                1,
                "{}: two assets claim `{}:{}`",
                a.slug,
                a.library,
                a.slug
            );
        }
    }

    /// A declared collection may only hold references the planted world
    /// can answer: an asset this seed commits, or a song it commits.
    ///
    /// This is the assertion that keeps the seed from demonstrating the
    /// wrong thing. A `Setlist` full of references to nothing still
    /// plants, still lists, and still opens — as a list of unresolved
    /// rows, which is exactly the state ADR 0003 says is *legible* and
    /// therefore exactly the state a demo cannot be made of.
    #[test]
    fn every_declared_collection_references_something_the_seed_plants() {
        use links::NodeKind;
        for c in DECLARED_COLLECTIONS {
            assert!(
                ORGS.iter().any(|(slug, _)| *slug == c.org),
                "{}: org `{}` is not one the example plants",
                c.title,
                c.org
            );
            assert!(
                matches!(c.kind, "library" | "setlist" | "show" | "playlist"),
                "{}: `{}` is not a collection kind",
                c.title,
                c.kind
            );
            assert!(!c.items.is_empty(), "{}: an empty collection", c.title);
            for (kind, id) in c.items {
                let kind = NodeKind::parse(kind)
                    .unwrap_or_else(|| panic!("{}: `{kind}` is not a node kind", c.title));
                let library = crate::node_homes::library_of(kind).unwrap_or_else(|| {
                    panic!("{}: `{kind:?}` has no home under resources/", c.title)
                });
                // A song is a folder with a manifest; an asset is a
                // `DECLARED_ASSETS` row, which the test above proves is
                // committed.
                let planted = if kind == NodeKind::Song {
                    STUDIO
                        .get_file(format!("{}/Resources/songs/{id}/manifest.json", c.org))
                        .is_some()
                } else {
                    assets_of(c.org).any(|a| a.library == library && a.slug == *id)
                };
                assert!(
                    planted,
                    "{}: `{kind:?}:{id}` is not planted — {}",
                    c.title, c.demonstrates
                );
            }
        }
    }

    /// Titles are how a replant recognises a collection it already
    /// planted, so two of them sharing one would plant the second only
    /// once and then never again.
    #[test]
    fn collection_titles_are_distinct_within_an_org() {
        for c in DECLARED_COLLECTIONS {
            assert_eq!(
                DECLARED_COLLECTIONS
                    .iter()
                    .filter(|o| o.org == c.org && o.title == c.title)
                    .count(),
                1,
                "{}: two collections share this title in `{}`",
                c.title,
                c.org
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
