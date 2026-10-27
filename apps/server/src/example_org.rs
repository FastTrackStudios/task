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

/// One committed file's bytes, by its path under `examples/studio/`.
#[must_use]
pub fn studio_file(path: &str) -> Option<&'static [u8]> {
    STUDIO.get_file(path).map(include_dir::File::contents)
}

/// Where the seed keeps its bound sample: a root made through
/// `RootsService::create`, as an app makes one, and the file saved into
/// it through the upload lane, by `demo_cli` at plant time.
pub const BOUND_SAMPLE: BoundSample = BoundSample {
    org: "acme-audio",
    root_dir: "signal/samples",
    root_name: "Signal samples",
    path: "Loops/Single master.wav",
    source: "acme-audio/Resources/songs/single-master/Single master.wav",
    slug: "single-master-loop",
};

/// The seed's bound sample — see [`BOUND_SAMPLE`].
#[derive(Debug, Clone, Copy)]
pub struct BoundSample {
    pub org: &'static str,
    /// Relative to the org's files area, as `RootsService::create` takes it.
    pub root_dir: &'static str,
    pub root_name: &'static str,
    /// Root-relative path the audio is saved at.
    pub path: &'static str,
    /// The committed file whose bytes are saved.
    pub source: &'static str,
    /// The sample manifest bound to it.
    pub slug: &'static str,
}

/// The orgs the example describes, in the order a demo should boot them.
///
/// ACME first: it is the one that owns the audio work, holds the
/// deliverables a client is shown, and is the home org in every
/// arrangement here.
pub const ORGS: &[(&str, &str)] = &[
    ("acme-audio", "ACME Audio"),
    ("vnt-video", "VNT Video"),
    ("alice-personal", "Alice Personal"),
    // A jam program's library at real size — see `CHORDSHEET_LIBRARIES`.
    // Planted beside ACME, like Alice's own org, so Keyflow reaches it
    // through the one server the demo points at.
    ("rockstars-of-tomorrow", "Rockstars of Tomorrow"),
    // The owner of Crescendum, the album shared across the org line
    // with a person from Rockstars of Tomorrow (`docs/spec/sharing.md`,
    // `SEED_GRANTS`). Planted after Rockstars on ACME's data root:
    // grants in v1 cover owner and guest orgs on one server, and the
    // grant names a person whose account Rockstars already holds.
    ("fasttrackaudio", "FastTrackAudio"),
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
    let assets = org_root.assets_dir();
    let projects = org_root.projects_dir();
    let repos = repos_dir(org_root);
    let mail = mail_dir(org_root);

    // Each step a span (`plant.*`): every test world and every demo boot
    // pays for planting, so a slow step should show itself.
    let _install = tracing::info_span!("plant.install", %slug).entered();
    // `Dir::files` is one level deep, so walk the whole subtree.
    let mut planted = Planted::default();
    tracing::info_span!("plant.tree").in_scope(|| {
        plant(
            dir,
            slug,
            &vault,
            &wiki,
            &wikis,
            &files,
            &resources,
            &assets,
            &projects,
            &repos,
            &mail,
            &mut planted,
        )
    })?;
    complete_mail_accounts(&mail)?;
    tracing::info_span!("plant.inbox").in_scope(|| plant_inbox(&vault, slug, &mut planted))?;
    #[cfg(feature = "plugin-mealplan")]
    tracing::info_span!("plant.meals").in_scope(|| plant_meals(&vault, slug, &mut planted))?;
    // Before the declared collections: a setlist declared over a library
    // names songs this import creates.
    tracing::info_span!("plant.chordsheets").in_scope(|| plant_chordsheet_library(org_root, slug));
    #[cfg(feature = "plugin-fasttrackstudio")]
    tracing::info_span!("plant.collections").in_scope(|| plant_collections(org_root, slug));
    #[cfg(feature = "plugin-wiki")]
    tracing::info_span!("plant.repo_wikis").in_scope(|| plant_repo_wikis(org_root, slug));
    #[cfg(feature = "plugin-wiki")]
    declare_wiki_configs(org_root, slug)?;
    #[cfg(feature = "plugin-wiki")]
    declare_shared_sources(org_root, slug)?;
    Ok(planted)
}

/// The example's asset shelves and projects that are open to other orgs,
/// and how (`wiki.access.shelf-visibility`).
///
/// Every other shelf and project in the example is private, which is the
/// default: these are the ones the story shares. ACME's song library is
/// public, so it is in the directory a demo user browses
/// (`demo_plant::the_planted_song_shelf_is_subscribable_across_orgs`).
/// VNT's Shared Project is two orgs on one project, and unlisted: open by
/// reference, listed to nobody.
#[cfg(feature = "plugin-wiki")]
pub const SHARED_SOURCES: &[(&str, &str, &str, wiki_proto::config::Visibility)] = &[
    (
        "acme-audio",
        "assets",
        "songs",
        wiki_proto::config::Visibility::Public,
    ),
    (
        "vnt-video",
        "projects",
        "shared-project",
        wiki_proto::config::Visibility::Unlisted,
    ),
];

/// Write each shared source's `.shelf.toml` where none exists yet.
///
/// Only where none exists, for the reason [`declare_wiki_configs`] gives:
/// somebody who made a planted shelf private again must not find it open
/// after a replant.
#[cfg(feature = "plugin-wiki")]
fn declare_shared_sources(org_root: &org_proto::OrgRoot, slug: &str) -> std::io::Result<()> {
    for (org, tier, source, visibility) in SHARED_SOURCES {
        if *org != slug {
            continue;
        }
        let root = org_root.path().join(tier).join(source);
        if !root.is_dir() || wiki_live::source_access::config_path(&root).exists() {
            continue;
        }
        wiki_live::source_access::set_visibility(&root, *visibility)?;
    }
    Ok(())
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

/// Where the example's mail accounts are planted: `<org>/mail/`.
///
/// Beside the vault, not inside it. A mailbox inside `vault/` is inside
/// a File Root, and the next materialise deletes anything there that is
/// not in the canonical tree — which on prod silently took a configured
/// account away twice on 2026-09-17. `AppState` reads this same
/// location (see its `mail_root`), so the seed and the server agree on
/// where a mailbox lives.
#[must_use]
pub fn mail_dir(org_root: &org_proto::OrgRoot) -> std::path::PathBuf {
    org_root.path().join("mail")
}

/// Give every planted account the maildir directories git cannot carry.
///
/// A maildir is `{cur,new,tmp}`, and git stores no empty directory —
/// `tmp` holds nothing in a fixture, so it cannot be committed, and
/// `email_maildir::is_maildir` wants `cur` and `new` both present
/// before it will treat the directory as an account at all. Creating
/// them here keeps the committed mailbox to the messages themselves.
fn complete_mail_accounts(mail: &Path) -> std::io::Result<()> {
    let Ok(entries) = std::fs::read_dir(mail) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let account = entry.path();
        if !account.is_dir() {
            continue;
        }
        for sub in ["cur", "new", "tmp"] {
            std::fs::create_dir_all(account.join(sub))?;
        }
    }
    Ok(())
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
    assets: &Path,
    projects: &Path,
    repos: &Path,
    mail: &Path,
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
            // `AssetGroups/<kind>/…` → `<org>/assets/<kind>/…` — ADR
            // 0004's Assets root, one directory per group.
            //
            // The committed area is `AssetGroups/` and not `Assets/`
            // because `Assets/` is already taken, by the *files* tier's
            // own name for an org's loose material
            // (`files_domain::layout`, and the RPP template committed
            // under it). Two different things called Assets is a fact
            // about this repository rather than a design, and the
            // committed tree spells them apart so that nobody has to
            // work out which one a path means.
            Some("AssetGroups") => assets.join(rel.strip_prefix("AssetGroups").unwrap_or(rel)),
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
            // `Mail/<account>/…` → `<org>/mail/<account>/…`: one
            // directory per account, each a maildir, each optionally
            // carrying the `account.json` the server reads for the
            // address and folder aliases. Beside the vault rather than
            // in it — see `mail_dir`.
            Some("Mail") => mail.join(rel.strip_prefix("Mail").unwrap_or(rel)),
            // `Projects/<Dir>/…` → `<org>/projects/<slug>/…` — ADR
            // 0004's fourth root, one directory per project, holding
            // the whole of it.
            //
            // The committed area keeps the name `Projects/` and the
            // human directory names ("Example Album") because that tree
            // is read by other suites as a picture of a studio's disk
            // (`tests/integration/src/archive.rs`). What the seeder does
            // with it is slug the top-level name and plant it on the
            // tier, so the committed tree stays legible and the planted
            // one matches the layout every other reader expects.
            Some("Projects") => {
                let inner = rel.strip_prefix("Projects").unwrap_or(rel);
                let mut parts = inner.iter();
                match parts.next().and_then(|s| s.to_str()) {
                    Some(name) => projects.join(wiki_slug(name)).join(parts.as_path()),
                    None => continue,
                }
            }
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
            child, slug, vault, wiki, wikis, files, resources, assets, projects, repos, mail,
            planted,
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
    // A second employee, newer than Sam: the person attribution is for —
    // work Atlas made for Alice and Alice handed on lands on Priya's
    // own Tasks list (`attribution.seed`).
    Member {
        email: "priya@acme.test",
        name: "Priya",
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
    // Runs the jam program: the person who signs into Keyflow and finds
    // the Adult Jam library, its setlists and its shows.
    Member {
        email: "riley@rockstars.test",
        name: "Riley",
        org: "rockstars-of-tomorrow",
        holds: Holds::Owner,
        scope: "",
    },
    // Runs FastTrackAudio, owns Crescendum, and is the one who granted
    // Jordan into it (`SEED_GRANTS`).
    Member {
        email: "taylor@fasttrackaudio.test",
        name: "Taylor",
        org: "fasttrackaudio",
        holds: Holds::Owner,
        scope: "",
    },
    // The guest: a person of Rockstars of Tomorrow, and only of it. No
    // account in FastTrackAudio's store and no membership row there —
    // what Jordan reaches of Crescendum is the Sharing Grant and nothing
    // else (`sharing.grant.membership-free`). A person, never a bot:
    // there is no guest bot (Captain decision 2).
    Member {
        email: "jordan@rockstars.test",
        name: "Jordan",
        org: "rockstars-of-tomorrow",
        holds: Holds::Employee,
        scope: "",
    },
];

/// The password every example account is created with.
///
/// One password, printed on every boot, protecting nothing. A demo whose
/// credentials have to be looked up is a demo nobody runs.
pub const PASSWORD: &str = "correct-horse-battery-staple";

/// Who holds which seeded task, as `(task title, cast email)` — the
/// team context a production board shows ("who is on Master review?").
/// Some tasks are left unowned on purpose: an unclaimed task is part of
/// the picture too.
pub const TASK_OWNERS: &[(&str, &str)] = &[
    ("Comp lead vocals — Track One", "alice@acme.test"),
    ("Mix revisions — Track Two", "alice@acme.test"),
    ("Master review with the label", "alice@acme.test"),
    // Atlas made it for Alice; Alice handed it to Priya.
    ("Collect streaming metadata", "priya@acme.test"),
    ("Cut the visualiser to the album mix", "sam@acme.test"),
    ("Rough cut review", "victor@vnt.test"),
    // Alice's own next actions — the Tasks list is what is assigned to
    // you (`task_proto::is_mine`), so the where/when work and the
    // maintenance mode's errands and chores are hers to see.
    ("Restring the studio guitars", "alice@acme.test"),
    ("Pick up the test pressings", "alice@acme.test"),
    ("Listen back to the rough mixes", "alice@acme.test"),
    ("Return the hired mic stands", "alice@acme.test"),
    ("Vacuum the live room", "alice@acme.test"),
    ("Take the studio recycling out", "alice@acme.test"),
    // FastTrackAudio's board, on both sides of the grant's line.
    ("Sequence Crescendum", "taylor@fasttrackaudio.test"),
    ("Mix First Light", "taylor@fasttrackaudio.test"),
    ("Write the Northbeam stings", "taylor@fasttrackaudio.test"),
    // The due answer's share (`task_proto::due`): one late, so Home,
    // Tasks, the sidebar, `task task list --due` and `task brief` have
    // overdue work to agree on; and a call that waits for the phone.
    ("Send the label the stems", "alice@acme.test"),
    ("Ring the piano tuner", "alice@acme.test"),
];

/// Where or when a seeded task can be done, as `(task title, contexts)`
/// — what the board's Relevant filter gates on. A place (`@studio`,
/// `@out`, matched by "I'm at") and a time window (`@evening`), so the
/// demo shows Relevant holding work back and the list saying so.
pub const TASK_CONTEXTS: &[(&str, &[&str])] = &[
    ("Restring the studio guitars", &["@studio"]),
    ("Pick up the test pressings", &["@out"]),
    ("Listen back to the rough mixes", &["@evening"]),
    // The maintenance hour's work. Neither is a place Alice says she is
    // or a time window; both are opened by the committed day template's
    // 16:00 maintenance block (`BlockCategory::surfaces`), which is what
    // `every_task_context_can_be_met` checks.
    ("Return the hired mic stands", &["@errands"]),
    ("Vacuum the live room", &["@chores"]),
    // A device gate (`process.context.device`): the app fills in what
    // it runs on, so the call shows on Alice's phone and waits on her
    // computer, without her saying where she is.
    ("Ring the piano tuner", &["@phone"]),
];

/// Tags a seeded task carries, as `(task title, tags)` — no `#`, the
/// way `task_proto::capture` stores them. `#chore` is the spelling
/// people write as often as `@chores`, so the seed carries both and a
/// maintenance hour brings forward each (`matches_focus` reads tags as
/// well as contexts).
pub const TASK_TAGS: &[(&str, &[&str])] = &[("Take the studio recycling out", &["chore"])];

/// One line of history the seed plants in an org's activity feed
/// (`docs/spec/activity.md`), so `/activity` and a task's page are not
/// empty on a fresh plant. Dated relative to the plant, like due dates.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredActivity {
    pub org: &'static str,
    /// Fixed, so a replant finds the entry it wrote and keeps it.
    pub id: &'static str,
    /// A declared task's title.
    pub task: &'static str,
    pub kind: notify_proto::ActivityKind,
    /// A cast member's email — the byline.
    pub actor: &'static str,
    pub field: &'static str,
    pub from: &'static str,
    pub to: &'static str,
    pub hours_ago: i64,
}

/// The board getting to where the declarations leave it: Alice took the
/// vocal comp, started it, and wrapped the drums.
pub const SEED_ACTIVITY: &[DeclaredActivity] = &[
    DeclaredActivity {
        org: "acme-audio",
        id: "5e6a1c00-7a10-4c2b-9d3e-0a1b2c3d4e01",
        task: "Comp lead vocals — Track One",
        kind: notify_proto::ActivityKind::Assigned,
        actor: "alice@acme.test",
        field: "assignee",
        from: "",
        to: "Alice",
        hours_ago: 50,
    },
    DeclaredActivity {
        org: "acme-audio",
        id: "5e6a1c00-7a10-4c2b-9d3e-0a1b2c3d4e02",
        task: "Comp lead vocals — Track One",
        kind: notify_proto::ActivityKind::StatusChanged,
        actor: "alice@acme.test",
        field: "status",
        from: "open",
        to: "in-progress",
        hours_ago: 26,
    },
    DeclaredActivity {
        org: "acme-audio",
        id: "5e6a1c00-7a10-4c2b-9d3e-0a1b2c3d4e03",
        task: "Re-track drums — Track Three",
        kind: notify_proto::ActivityKind::Completed,
        actor: "alice@acme.test",
        field: "status",
        from: "in-progress",
        to: "done",
        hours_ago: 30,
    },
    DeclaredActivity {
        org: "acme-audio",
        id: "5e6a1c00-7a10-4c2b-9d3e-0a1b2c3d4e04",
        task: "Master review with the label",
        kind: notify_proto::ActivityKind::PriorityChanged,
        actor: "alice@acme.test",
        field: "priority",
        from: "normal",
        to: "high",
        hours_ago: 4,
    },
];

/// A seeded comment on a declared task: the thread a task's page opens
/// with, and a `commented` line in the feed.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredComment {
    pub org: &'static str,
    /// The feed entry's fixed id (the message's own id is the threads
    /// store's to mint; a replant matches it by author and body).
    pub id: &'static str,
    pub task: &'static str,
    /// A cast member's email.
    pub author: &'static str,
    pub body: &'static str,
    pub hours_ago: i64,
}

pub const SEED_COMMENTS: &[DeclaredComment] = &[
    DeclaredComment {
        org: "acme-audio",
        id: "5e6a1c00-7a10-4c2b-9d3e-0a1b2c3d4e11",
        task: "Comp lead vocals — Track One",
        author: "sam@acme.test",
        body: "Take three has the best chorus — the verse on take one is tighter though. Worth a look before you commit the comp.",
        hours_ago: 20,
    },
    DeclaredComment {
        org: "acme-audio",
        id: "5e6a1c00-7a10-4c2b-9d3e-0a1b2c3d4e12",
        task: "Comp lead vocals — Track One",
        author: "alice@acme.test",
        body: "Agreed. Comping verse 1 from take one, everything else from three.",
        hours_ago: 19,
    },
];

/// Plant `slug`'s declared history: [`SEED_ACTIVITY`] into the org's
/// activity feed (`notify.sqlite`), and [`SEED_COMMENTS`] into each
/// task's comment thread (`threads.sqlite`) with their `commented`
/// lines. Run after the cast and the tasks exist — bylines are cast
/// accounts, entries name declared tasks.
///
/// Idempotent: feed entries carry fixed ids, and a comment already in
/// its thread (same body) is not posted again.
///
/// # Errors
///
/// Opening or migrating either database.
// t[impl activity.seed]
pub async fn plant_activity(
    org_root: &org_proto::OrgRoot,
    slug: &str,
    auth: &crate::AuthState,
) -> eyre::Result<usize> {
    use notify_proto::{Activity, ActivityKind};
    use sea_orm_migration::MigratorTrait as _;
    use task::TaskService as _;
    use threads::ThreadsService as _;

    let decls: Vec<&DeclaredActivity> = SEED_ACTIVITY.iter().filter(|a| a.org == slug).collect();
    let comments: Vec<&DeclaredComment> = SEED_COMMENTS.iter().filter(|c| c.org == slug).collect();
    if decls.is_empty() && comments.is_empty() {
        return Ok(0);
    }
    let open = |path: std::path::PathBuf| async move {
        sea_orm::Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
            .await
            .map_err(|e| eyre::eyre!("open {}: {e}", path.display()))
    };
    let notify_conn = open(org_root.path().join("notify.sqlite")).await?;
    notify::Migrator::up(&notify_conn, None)
        .await
        .map_err(|e| eyre::eyre!("migrate notify: {e}"))?;
    let feed = notify::ActivityStore::new(notify_conn);
    let tasks = task::TaskBackend::new(org_root.vault_dir())
        .list()
        .unwrap_or_default();
    let task_of = |title: &str| tasks.iter().find(|t| t.title == title).cloned();
    let now = chrono::Utc::now();
    let mut written = 0;

    let entry = |id: &str,
                 kind: ActivityKind,
                 t: &task::TaskInfo,
                 (user_id, name): (uuid::Uuid, Option<String>),
                 hours: i64| Activity {
        id: id.parse().unwrap_or_else(|_| uuid::Uuid::new_v4()),
        at: now - chrono::Duration::hours(hours),
        actor_id: user_id.to_string(),
        actor_name: name.unwrap_or_else(|| "Someone".to_owned()),
        kind,
        entity_type: "task".to_owned(),
        entity_id: t.id.to_string(),
        entity_title: t.title.clone(),
        href: format!("/tasks/{}", t.id),
        project_id: t.project_id,
        field: String::new(),
        from: String::new(),
        to: String::new(),
        body: String::new(),
    };

    for d in decls {
        let (Some(t), Ok(Some(user))) =
            (task_of(d.task), auth.auth.find_user_by_email(d.actor).await)
        else {
            continue;
        };
        let mut a = entry(d.id, d.kind, &t, (user.id, user.name), d.hours_ago);
        d.field.clone_into(&mut a.field);
        d.from.clone_into(&mut a.from);
        d.to.clone_into(&mut a.to);
        if feed
            .insert_if_absent(a)
            .await
            .map_err(|e| eyre::eyre!("seed activity: {e}"))?
        {
            written += 1;
        }
    }

    if !comments.is_empty() {
        let threads_conn = open(org_root.threads_db()).await?;
        threads::Migrator::up(&threads_conn, None)
            .await
            .map_err(|e| eyre::eyre!("migrate threads: {e}"))?;
        let store = threads::Store::new(threads_conn);
        for c in comments {
            let (Some(t), Ok(Some(user))) = (
                task_of(c.task),
                auth.auth.find_user_by_email(c.author).await,
            ) else {
                continue;
            };
            let existing = store
                .list_threads("task".to_owned(), t.id)
                .await
                .map_err(|e| eyre::eyre!("list threads: {e}"))?;
            let thread = match existing.into_iter().last() {
                Some(th) => th,
                None => store
                    .create_thread(threads::CreateThreadRequest {
                        org_id: uuid::Uuid::nil(),
                        entity_type: "task".to_owned(),
                        entity_id: t.id,
                        title: "Comments".to_owned(),
                        kind: String::new(),
                        created_by: user.id,
                        source_kind: "native".to_owned(),
                        source_ref: None,
                        source_url: None,
                    })
                    .await
                    .map_err(|e| eyre::eyre!("create thread: {e}"))?,
            };
            let at = now - chrono::Duration::hours(c.hours_ago);
            let posted = store
                .list_messages(thread.id)
                .await
                .map_err(|e| eyre::eyre!("list messages: {e}"))?;
            if !posted.iter().any(|m| m.body == c.body) {
                store
                    .post_message(threads::PostMessageRequest {
                        thread_id: thread.id,
                        org_id: uuid::Uuid::nil(),
                        author_id: Some(user.id),
                        author_label: user.name.clone().unwrap_or_default(),
                        body: c.body.to_owned(),
                        reply_to: None,
                        source_kind: "native".to_owned(),
                        external_id: None,
                        original_text: None,
                        source_url: None,
                        posted_at: Some(at),
                    })
                    .await
                    .map_err(|e| eyre::eyre!("post comment: {e}"))?;
            }
            let mut a = entry(
                c.id,
                ActivityKind::Commented,
                &t,
                (user.id, user.name.clone()),
                c.hours_ago,
            );
            c.body.clone_into(&mut a.body);
            if feed
                .insert_if_absent(a)
                .await
                .map_err(|e| eyre::eyre!("seed comment activity: {e}"))?
            {
                written += 1;
            }
        }
    }
    Ok(written)
}

/// A seeded task's description, as `(task title, markdown body)` — a body
/// with the structures the document editor writes (headings, a
/// checklist, a quote, a code block, a folded section), so opening the
/// issue shows the editor on real content and `/` has something to sit
/// beside. Planted only into an empty description, so a demo edit stays.
// t[impl doc.editor.one]
pub const TASK_DETAILS: &[(&str, &str)] = &[(
    "Master review with the label",
    "The label hears the masters on Thursday. Go in with every track signed off and the notes from the last pass answered.\n\n## Before the call\n\n- [x] Bounce the masters at -14 LUFS integrated\n- [ ] Print the loudness report for each track\n- [ ] Send the private link the evening before\n\n## What they asked for last time\n\n> The low end on Track Two still feels a little shy next to the singles.\n\n1. Revisit the Track Two bass with the mix engineer\n2. A/B against the reference playlist\n3. Note the change in the delivery sheet\n\n### Delivery\n\n```\nAlbum master/\n  01 Track One.wav\n  02 Track Two.wav\n  03 Track Three.wav\n```\n\n> [!note]- What happens after sign-off\n> Masters go to the distributor and the vinyl cut is booked.\n",
)];

/// One capture sitting in an org's inbox at plant time — the input to
/// processing, which is the centre of the app
/// (`docs/spec/capture-and-process.md`).
///
/// Dates are relative to the plant, like task due dates: a fresh plant
/// always has captures from the last few days and one snoozed into the
/// future, whatever the date it runs on.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredCapture {
    pub org: &'static str,
    /// Fixed, so a replant finds the file it wrote and keeps it.
    pub id: &'static str,
    /// The capture, verbatim — quick-add syntax included, which is what
    /// "make it a task" parses (`task_proto::capture`).
    pub body: &'static str,
    /// Where it came from: `ui`, `cli`, `watch`, or an agent's lane.
    pub source: &'static str,
    /// `open`, `suggested` for something an agent proposed, or
    /// `processed` for one already decided.
    pub status: &'static str,
    /// Captured this many days before the plant.
    pub captured_days_ago: i64,
    /// Snoozed: resurfaces this many days after the plant. `None` =
    /// due now.
    pub resurface_in_days: Option<i64>,
    /// What a processed capture became (`processed_into`): a declared
    /// task's path (`Task/<slug>.md`, where the seeder files it) or a
    /// committed note's vault path. `None` on a processed capture is
    /// "already done" — it became nothing.
    pub processed_into: Option<&'static str>,
}

/// What is waiting in the inbox: plain thoughts, captures carrying
/// quick-add syntax a maintenance hour can find once processed, one
/// suggestion an agent made from mail, and one put off into the future
/// that is NOT due today.
pub const INBOX: &[DeclaredCapture] = &[
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a01",
        body: "Call the piano tuner about Thursday",
        source: "ui",
        status: "open",
        captured_days_ago: 2,
        resurface_in_days: None,
        processed_into: None,
    },
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a02",
        body: "Pick up gaff tape and 9V batteries @errands",
        source: "cli",
        status: "open",
        captured_days_ago: 1,
        resurface_in_days: None,
        processed_into: None,
    },
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a03",
        body: "Descale the espresso machine #chore",
        source: "watch",
        status: "open",
        captured_days_ago: 1,
        resurface_in_days: None,
        processed_into: None,
    },
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a04",
        body: "Idea: a stems-only bonus edition of the album\n\nSell the multitracks of each song as a remix pack — Track Two first, it has the cleanest stems.",
        source: "ui",
        status: "open",
        captured_days_ago: 3,
        resurface_in_days: None,
        processed_into: None,
    },
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a05",
        body: "Reply to Example Client about the lyric video deadline",
        source: "email",
        status: "suggested",
        captured_days_ago: 0,
        resurface_in_days: None,
        processed_into: None,
    },
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a06",
        body: "Look into a second pair of monitors for the B room",
        source: "ui",
        status: "open",
        captured_days_ago: 10,
        resurface_in_days: Some(14),
        processed_into: None,
    },
    // Marked up the way people capture: a person by @handle, a #tag
    // and an @context — the inbox shows Sam's avatar, a badge, a chip.
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a07",
        body: "Ask @sam to check the B-room patchbay before Friday's session #studio @office",
        source: "ui",
        status: "open",
        captured_days_ago: 0,
        resurface_in_days: None,
        processed_into: None,
    },
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a08",
        body: "Casey sent notes on the second verse — fold them into the mix #client @chamber",
        source: "email",
        status: "open",
        captured_days_ago: 1,
        resurface_in_days: None,
        processed_into: None,
    },
    // Someone outside the org: Jordan is a contact, not a member, and
    // the mention still opens their profile.
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a09",
        body: "Send the final mixes to @jordan for mastering #album",
        source: "ui",
        status: "open",
        captured_days_ago: 0,
        resurface_in_days: None,
        processed_into: None,
    },
    // Already decided — the other side of the loop, so the walk back
    // from a durable thing to the thought that started it exists in the
    // planted world (`process.capture.verbatim`): one capture that became
    // a task on the board, one rewritten as a note in Alice's words (the
    // committed note quotes it verbatim), and one that was already done.
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a10",
        body: "Return the hired mic stands @errands",
        source: "watch",
        status: "processed",
        captured_days_ago: 4,
        resurface_in_days: None,
        processed_into: Some("Task/return-the-hired-mic-stands.md"),
    },
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a11",
        body: "B room hums whenever the fridge compressor kicks in?? ground loop?",
        source: "ui",
        status: "processed",
        captured_days_ago: 5,
        resurface_in_days: None,
        processed_into: Some("The B room hum is the fridge on the studio circuit.md"),
    },
    DeclaredCapture {
        org: "acme-audio",
        id: "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a12",
        body: "Text Sam the door code for Saturday",
        source: "ui",
        status: "processed",
        captured_days_ago: 2,
        resurface_in_days: None,
        processed_into: None,
    },
];

/// The declared captures of one org.
pub fn inbox_of(slug: &str) -> impl Iterator<Item = &'static DeclaredCapture> + '_ {
    INBOX.iter().filter(move |c| c.org == slug)
}

/// The inbox item a declaration plants, dated against `now`.
#[must_use]
pub fn capture_item(c: &DeclaredCapture, now: chrono::DateTime<chrono::Utc>) -> inbox::InboxItem {
    let created = (now - chrono::Duration::days(c.captured_days_ago))
        .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    let mut item = inbox::InboxItem::capture(c.id, c.body, c.source, created);
    c.status.clone_into(&mut item.status);
    item.processed_into = c.processed_into.map(str::to_owned);
    if let Some(days) = c.resurface_in_days {
        item.resurface_on = Some(
            (now + chrono::Duration::days(days))
                .date_naive()
                .to_string(),
        );
        // Put off once already — that is how an item comes to have a
        // resurface date (`inbox_proto::review`, "later").
        item.reviews = 1;
        item.interval = days;
        item.ease = inbox::schedule::BASE_EASE;
    }
    item
}

/// Write `slug`'s declared captures into `vault/Records/inbox/`, in
/// the inbox backend's own format (`inbox::serialize_inbox_item`).
///
/// Idempotent by omission, like the tree: a capture already on disk is
/// left as it is — Alice may have processed it since, and a replant
/// that reopened it would be undoing her work.
///
/// # Errors
///
/// Any filesystem or serialisation error.
pub fn plant_inbox(vault: &Path, slug: &str, planted: &mut Planted) -> std::io::Result<()> {
    let now = chrono::Utc::now();
    let dir = vault.join("Records").join("inbox");
    for c in inbox_of(slug) {
        let dest = dir.join(format!("{}.md", c.id));
        if dest.exists() {
            planted.kept += 1;
            continue;
        }
        let page =
            inbox::serialize_inbox_item(&capture_item(c, now)).map_err(std::io::Error::other)?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&dest, page)?;
        planted.written += 1;
    }
    Ok(())
}

/// A bot in the example: an assistant with an account of its own
/// (`crate::bots`), so what it captured or made reads "from Atlas".
#[derive(Debug, Clone, Copy)]
pub struct DeclaredBot {
    pub org: &'static str,
    pub name: &'static str,
    /// Under [`crate::bots::BOT_DOMAIN`] — which is what makes the
    /// account a bot to attribution (`crate::attribution::Creator`).
    pub email: &'static str,
    /// The cast member a **personal** bot works for (by email): what it
    /// made reads "from Atlas (for Alice)". `None` for a team bot
    /// (`crate::bots::Kind`).
    pub acts_for: Option<&'static str>,
}

/// The example's bots: Atlas, Alice's personal assistant, who files
/// suggestions from mail and makes tasks when asked — and the Studio
/// Desk, a team bot that acts for nobody in particular.
pub const BOTS: &[DeclaredBot] = &[
    DeclaredBot {
        org: "acme-audio",
        name: "Atlas",
        email: "atlas@bots.task.invalid",
        acts_for: Some("alice@acme.test"),
    },
    DeclaredBot {
        org: "acme-audio",
        name: "Studio Desk",
        email: "studio-desk@bots.task.invalid",
        acts_for: None,
    },
];

/// The creator a seeded record made by account `email` is attributed
/// to: the account, and — for a [`BOTS`] entry that is someone's
/// personal assistant — whom it acted for. `None` when the account was
/// not made.
pub async fn seeded_creator(
    auth: &crate::AuthState,
    email: &str,
) -> Option<crate::attribution::Creator> {
    let user = auth.auth.find_user_by_email(email).await.ok()??;
    let creator = crate::attribution::Creator::of_account(&user);
    let Some(person) = BOTS
        .iter()
        .find(|b| b.email == email)
        .and_then(|b| b.acts_for)
    else {
        return Some(creator);
    };
    match auth.auth.find_user_by_email(person).await {
        Ok(Some(p)) => Some(creator.acting_for(crate::attribution::Creator::of_account(&p))),
        _ => Some(creator),
    }
}

/// The declared bots of one org.
pub fn bots_of(slug: &str) -> impl Iterator<Item = &'static DeclaredBot> + '_ {
    BOTS.iter().filter(move |b| b.org == slug)
}

// t[impl attribution.seed]
/// Who captured each declared capture, as `(capture id, account email)`
/// — a cast member or a [`BOTS`] entry. Mixed on purpose: Alice's own
/// thoughts read "from you", Atlas's suggestions "from Atlas", Sam's
/// "from Sam", and the captures left out stand for the ones made before
/// attribution existed, which say nothing.
pub const CAPTURE_CREATORS: &[(&str, &str)] = &[
    ("6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a01", "alice@acme.test"),
    ("6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a02", "alice@acme.test"),
    ("6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a03", "alice@acme.test"),
    ("6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a04", "sam@acme.test"),
    (
        "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a05",
        "atlas@bots.task.invalid",
    ),
    ("6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a07", "alice@acme.test"),
    (
        "6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a08",
        "atlas@bots.task.invalid",
    ),
    ("6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a09", "alice@acme.test"),
    ("6f1d2c3a-0b7e-4c51-9a2f-1e8d4b5c6a10", "alice@acme.test"),
];

/// Who made each seeded task, as `(task title, account email)`, the
/// same mix as [`CAPTURE_CREATORS`]. Tasks not listed have no creator.
pub const TASK_CREATORS: &[(&str, &str)] = &[
    ("Comp lead vocals — Track One", "alice@acme.test"),
    ("Mix revisions — Track Two", "alice@acme.test"),
    ("Master review with the label", "atlas@bots.task.invalid"),
    ("Collect streaming metadata", "atlas@bots.task.invalid"),
    ("Cut the visualiser to the album mix", "sam@acme.test"),
    ("Restring the studio guitars", "alice@acme.test"),
    ("Pick up the test pressings", "atlas@bots.task.invalid"),
    // The team bot's: a chore it files for whoever is in, so for nobody
    // in particular — no "(for …)".
    ("Vacuum the live room", "studio-desk@bots.task.invalid"),
];

/// Who made the seeded task titled `title`, if the seed says.
#[must_use]
pub fn task_creator(title: &str) -> Option<&'static str> {
    TASK_CREATORS
        .iter()
        .find(|(t, _)| *t == title)
        .map(|(_, email)| *email)
}

/// Stamp `slug`'s declared captures with who captured them
/// ([`CAPTURE_CREATORS`]), once the accounts exist. Run after the cast
/// and the bots are made: the planted files are written before there is
/// anyone to attribute them to.
///
/// Write-once like every creator (`attribution.immutable`): a capture
/// that already names a creator is left alone, and one whose account is
/// missing stays unattributed. Returns how many were stamped.
///
/// # Errors
///
/// Reading or writing a capture file.
pub async fn stamp_capture_creators(
    vault: &Path,
    slug: &str,
    auth: &crate::AuthState,
) -> eyre::Result<usize> {
    use inbox::Inbox as _;
    let backend = inbox::VaultInbox::new(vault).map_err(|e| eyre::eyre!("open inbox: {e}"))?;
    let mut stamped = 0;
    for c in inbox_of(slug) {
        let Some((_, email)) = CAPTURE_CREATORS.iter().find(|(id, _)| *id == c.id) else {
            continue;
        };
        let Ok(mut item) = backend.get_inbox_item(c.id) else {
            continue;
        };
        if item.created_by.is_some() {
            continue;
        }
        let Some(creator) = seeded_creator(auth, email).await else {
            continue;
        };
        crate::attribution::attribute_inbox(&mut item, Some(&creator));
        // Straight to the file, as the planting itself was: the backend
        // keeps an item's first creator, and this one had none.
        let page = inbox::serialize_inbox_item(&item).map_err(|e| eyre::eyre!("{e}"))?;
        let dest = vault
            .join("Records")
            .join("inbox")
            .join(format!("{}.md", c.id));
        std::fs::write(&dest, page).map_err(|e| eyre::eyre!("write {}: {e}", dest.display()))?;
        stamped += 1;
    }
    Ok(stamped)
}

/// A meal on the plan, dated against the plant.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredMeal {
    pub org: &'static str,
    /// The meal block it fills — `breakfast`, `lunch`, `dinner`.
    pub slot: &'static str,
    /// The recipe's name; its file is `Cookbook/<name>.cook` in the
    /// org's Cooking wiki.
    pub recipe: &'static str,
    /// Planned for this many days after the plant.
    pub in_days: i64,
}

/// What the demo has planned to eat: today's three meals and
/// tomorrow's lunch, so the Planner's food blocks open onto a plan.
pub const MEALS: &[DeclaredMeal] = &[
    DeclaredMeal {
        org: "acme-audio",
        slot: "breakfast",
        recipe: "Overnight Oats",
        in_days: 0,
    },
    DeclaredMeal {
        org: "acme-audio",
        slot: "lunch",
        recipe: "Red Lentil Soup",
        in_days: 0,
    },
    DeclaredMeal {
        org: "acme-audio",
        slot: "dinner",
        recipe: "Studio Chilli",
        in_days: 0,
    },
    DeclaredMeal {
        org: "acme-audio",
        slot: "lunch",
        recipe: "Session Grain Bowl",
        in_days: 1,
    },
];

/// A declared meal's recipe path, relative to the cookbook root.
#[must_use]
pub fn meal_recipe_path(m: &DeclaredMeal) -> String {
    format!("Cookbook/{}.cook", m.recipe)
}

/// Plant [`MEALS`] for `slug` into its vault — each once, at the meal
/// plan's own path, dated against now. A replant on a later day adds
/// that day's, keeping what was there.
#[cfg(feature = "plugin-mealplan")]
pub fn plant_meals(vault: &Path, slug: &str, planted: &mut Planted) -> std::io::Result<()> {
    let today = chrono::Local::now().date_naive();
    for m in MEALS.iter().filter(|m| m.org == slug) {
        let date = today + chrono::Duration::days(m.in_days);
        let path = mealplan::default_meal_path(date, m.slot, None, None);
        if vault.join(&path).exists() {
            planted.kept += 1;
            continue;
        }
        let mut meal = mealplan::model::Meal {
            path,
            id: uuid::Uuid::new_v4(),
            name: m.recipe.to_owned(),
            scheduled_for: date,
            slot: m.slot.to_owned(),
            servings: 1,
            recipe_paths: mealplan::model::StringList(vec![meal_recipe_path(m)]),
            status: mealplan::model::Status::Planned.as_str().to_owned(),
            pantry_deductions: mealplan::model::PantryDeductions::default(),
            tags: mealplan::model::StringList::default(),
            date_created: None,
            date_modified: None,
            details: String::new(),
        };
        mealplan::write_meal(vault, &mut meal, false).map_err(std::io::Error::other)?;
        planted.written += 1;
    }
    Ok(())
}

/// The example's members of one org.
#[must_use]
pub fn cast_of(slug: &str) -> Vec<Member> {
    CAST.iter().filter(|m| m.org == slug).copied().collect()
}

/// Where a project of this org's example tree sits on disk:
/// `<org>/projects/<slug>/`.
///
/// `name` is the committed tree's directory name ("Example Album"); the
/// planted directory is its slug, which is the spelling every reference
/// into the project carries.
#[must_use]
pub fn project_path(org_root: &org_proto::OrgRoot, name: &str) -> std::path::PathBuf {
    org_root.projects_dir().join(project_rel(name))
}

/// A declared project's directory, relative to the Projects tier.
///
/// **Only the first segment is slugged**, and that asymmetry is the
/// rule rather than an oversight. The first segment names a *shelf*,
/// and a shelf's name travels: it is the middle of every reference into
/// the project and the directory a subscriber materialises, so it has
/// to be spelled one way — the same argument `wiki_slug` makes for a
/// wiki. Everything below it is an ordinary path inside that shelf,
/// where "Track Two" and "Audio Files" are what the tools that made
/// them wrote, and renaming them to please a slug rule would break
/// every session file that points at them.
///
/// `Example Album` → `example-album`;
/// `Example Album/Track Two` → `example-album/Track Two`.
#[must_use]
pub fn project_rel(dir: &str) -> String {
    match dir.split_once('/') {
        Some((top, rest)) => format!("{}/{rest}", wiki_slug(top)),
        None => wiki_slug(dir),
    }
}

/// A project the seeder DECLARES — a `project.md` at the root of the
/// project's own directory, which is what the app's Projects view
/// lists.
///
/// Planting the trees alone left the app's Projects page empty, which
/// read as broken rather than as "adoption is your first move". After
/// ADR 0004's fourth root the two acts have converged: a project's page
/// lives *inside* the project's directory, so declaring one is writing
/// a file into the tree it describes — which is what
/// `ProjectService::adopt` always did, now generalised. The tree is
/// still adopted as a File Root separately, because that is a Files
/// act and it is what makes the deliverables reviewable.
pub struct DeclaredProject {
    pub org: &'static str,
    /// Directory name under the committed tree's `Projects/`, and —
    /// slugged — the project's directory on the tier. What
    /// `project_path` resolves, and where the parts (if any) are read
    /// from.
    ///
    /// A **sub-project** carries its parent's directory in front of its
    /// own: `Example Album/Track Two`. That is the on-disk nesting and
    /// nothing else; [`Self::parent`] is what makes it a subproject.
    pub dir: &'static str,
    pub title: &'static str,
    /// The `dir` of this project's parent, for a sub-project.
    ///
    /// Two things follow from it and they are deliberately separate.
    /// The seeder sets `parent_id` on the page, which is the parentage
    /// (`project.nesting.explicit` — a declared link, never a directory
    /// name). And `dir` puts the child's files inside the parent's,
    /// which is where promotion would have left them.
    ///
    /// The seed carries one because the album/song pair is the case the
    /// whole part-versus-project distinction was written for, and a
    /// planted world with no subproject in it cannot show a demo user —
    /// or a test — the difference between a roster entry and a project.
    pub parent: Option<&'static str>,
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
        parent: None,
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
            ("Restring the studio guitars", "open", None),
            ("Listen back to the rough mixes", "open", None),
            // The maintenance hour's share of the board — see
            // `TASK_CONTEXTS` / `TASK_TAGS` and the committed
            // `Projects/Scheduling/templates/weekday.md`.
            ("Return the hired mic stands", "open", Some(3)),
            // Due today, and a chore: during the maintenance mode it
            // leads the due work; outside it, the late stems lead.
            ("Vacuum the live room", "open", Some(0)),
            ("Take the studio recycling out", "open", None),
            // The due queue's late one, and the phone's call — see
            // `TASK_OWNERS` / `TASK_CONTEXTS`.
            ("Send the label the stems", "open", Some(-1)),
            ("Ring the piano tuner", "open", None),
        ],
    },
    // The subproject, and the reason the seed has one: an album's song
    // that earned a project of its own is the case
    // `project.part.promotion` was written for, and the difference
    // between a roster entry and a project is not demonstrable from a
    // world containing only top-level projects.
    //
    // It stays in the album's `parts` roster above — three tracks
    // before and after, in playing order — which is `project.part
    // .listing`: "a roster of its pieces, not a list of the ones that
    // are not projects". A test below pins that both are true at once.
    //
    // Its deliverable is VIDEO on purpose. Video is the medium that
    // exercises the review platform end to end — renditions, frame
    // comments, the version switcher — and it is generated at plant
    // time into the subproject's own `Deliverables/`, so the surface a
    // client reaches for a subproject is the same one they reach for
    // its parent, reached through the same File Root machinery.
    DeclaredProject {
        org: "acme-audio",
        dir: "Example Album/Track Two",
        parent: Some("Example Album"),
        title: "Track Two",
        clients: "",
        form: Some(project::Form::Song),
        capabilities: &["music-production"],
        parts: &[],
        deliverables: &[(
            "Track Two visualiser",
            project::Medium::Video,
            project::Scope::WholeProject,
            project::Audience::Client,
        )],
        tasks: &[
            ("Cut the visualiser to the album mix", "in-progress", None),
            ("Colour pass on the visualiser", "open", Some(4)),
        ],
    },
    DeclaredProject {
        org: "acme-audio",
        dir: "First Single - Example Client",
        parent: None,
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
            ("Pick up the test pressings", "open", None),
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
        parent: None,
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
        parent: None,
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
    // ── FastTrackAudio: one project shared across the org line, one not ──
    //
    // Crescendum is the album a Rockstars of Tomorrow person is granted
    // into (`SEED_GRANTS`). Its committed pages carry FIXED ids — the
    // grant, the docs and the leak tests all name Crescendum by
    // `CRESCENDUM_ID` — and the album's own page carries a client and a
    // rate, so the guest projection has billing to strip
    // (`sharing.grant.exclusions`). Its subtree is two songs, one of
    // which has a sub-project one level further down, each with parts,
    // tasks and notes, and the album's directory is adopted as a File
    // Root like every declared project's.
    DeclaredProject {
        org: "fasttrackaudio",
        dir: "Crescendum",
        parent: None,
        title: "Crescendum",
        clients: "",
        form: Some(project::Form::Album),
        capabilities: &["music-production"],
        parts: &["Overture", "First Light", "Undertow", "Coda"],
        deliverables: &[],
        tasks: &[
            ("Sequence Crescendum", "open", Some(6)),
            ("Print the Overture motif for the Coda", "done", None),
        ],
    },
    DeclaredProject {
        org: "fasttrackaudio",
        dir: "Crescendum/First Light",
        parent: Some("Crescendum"),
        title: "First Light",
        clients: "",
        form: Some(project::Form::Song),
        capabilities: &["music-production"],
        parts: &["Lead Vocal", "Drums"],
        deliverables: &[],
        tasks: &[
            ("Mix First Light", "in-progress", Some(2)),
            ("Choose the First Light bridge", "open", None),
        ],
    },
    DeclaredProject {
        org: "fasttrackaudio",
        dir: "Crescendum/Undertow",
        parent: Some("Crescendum"),
        title: "Undertow",
        clients: "",
        form: Some(project::Form::Song),
        capabilities: &["music-production"],
        parts: &["Bass", "Strings"],
        deliverables: &[],
        tasks: &[("Comp the Undertow bass", "open", Some(4))],
    },
    // The nested one: a part of Undertow that became a project, so a
    // grant on the album has to reach two levels down.
    DeclaredProject {
        org: "fasttrackaudio",
        dir: "Crescendum/Undertow/Strings",
        parent: Some("Crescendum/Undertow"),
        title: "Undertow Strings",
        clients: "",
        form: None,
        capabilities: &["music-production"],
        parts: &["Violins", "Cello"],
        deliverables: &[],
        tasks: &[("Book the string session", "in-progress", Some(1))],
    },
    // The project the grant must keep out: FastTrackAudio's own client
    // work, with tasks, a note, a rate, a client contact, billed time
    // and an invoice (`SEED_BILLING`). Shared with nobody.
    DeclaredProject {
        org: "fasttrackaudio",
        dir: "Northbeam Jingles - Northbeam Records",
        parent: None,
        title: "Northbeam Jingles",
        clients: "Northbeam Records",
        form: None,
        capabilities: &["music-production"],
        parts: &[],
        deliverables: &[],
        tasks: &[
            ("Write the Northbeam stings", "in-progress", Some(3)),
            ("Invoice Northbeam for the first batch", "done", None),
        ],
    },
];

/// Crescendum's id: fixed, so the grant, the docs and every leak test
/// name the same project on every plant. Committed in its `project.md`.
pub const CRESCENDUM_ID: uuid::Uuid =
    uuid::Uuid::from_u128(0x9880_bdc8_307f_4ed6_b6cb_6e05_ca73_3571);

/// A billable client of an example org — a `timer_clients` row, planted
/// by `admin demo` under a fixed id because a project page's `clientId`
/// points at it.
pub struct DeclaredClient {
    pub org: &'static str,
    pub id: uuid::Uuid,
    pub name: &'static str,
    /// The committed contact (`Vault/Records/contacts/<id>.md`) of the
    /// person at the client, when there is one.
    pub contact: Option<uuid::Uuid>,
}

/// The example's billable clients.
pub const DECLARED_CLIENTS: &[DeclaredClient] = &[
    // Crescendum's client — on the album's page and therefore exactly
    // what a guest's projection of it must not carry.
    DeclaredClient {
        org: "fasttrackaudio",
        id: uuid::Uuid::from_u128(0x7c2e_4b1d_6a3f_4e8b_9d20_1b5a_7c9e_3f01),
        name: "Rockstars of Tomorrow",
        contact: None,
    },
    DeclaredClient {
        org: "fasttrackaudio",
        id: uuid::Uuid::from_u128(0x7c2e_4b1d_6a3f_4e8b_9d20_1b5a_7c9e_3f02),
        name: "Northbeam Records",
        contact: Some(uuid::Uuid::from_u128(
            0x8d4f_2a6c_1e3b_4a7d_b5c9_0e2f_4a6c_8b01,
        )),
    },
];

/// The example's billable clients of one org.
pub fn clients_of(slug: &str) -> impl Iterator<Item = &'static DeclaredClient> + '_ {
    DECLARED_CLIENTS.iter().filter(move |c| c.org == slug)
}

/// Time billed on a declared project, and the invoice raised from it:
/// `admin demo` sets the member's rate on the project, logs one
/// billable session of `hours`, and generates the invoice to `client`
/// from it — once, however often it is planted.
pub struct DeclaredBilling {
    pub org: &'static str,
    /// The declared project, by title.
    pub project: &'static str,
    /// Who did the work (a cast member of `org`).
    pub member: &'static str,
    /// The member's rate on this project, in cents an hour.
    pub hourly_cents: i64,
    pub hours: i64,
    /// A declared client of `org`, by name: who the invoice is to.
    pub client: &'static str,
}

/// The seed's billed work. Northbeam Jingles only: the money the
/// Crescendum grant must never reach lives next to it, in the same org.
pub const SEED_BILLING: &[DeclaredBilling] = &[DeclaredBilling {
    org: "fasttrackaudio",
    project: "Northbeam Jingles",
    member: "taylor@fasttrackaudio.test",
    hourly_cents: 15_000,
    hours: 3,
    client: "Northbeam Records",
}];

/// The seed's billed work in one org.
pub fn billing_of(slug: &str) -> impl Iterator<Item = &'static DeclaredBilling> + '_ {
    SEED_BILLING.iter().filter(move |b| b.org == slug)
}

/// A Sharing Grant the seed makes (`docs/spec/sharing.md`): one person
/// of `guest_org`, read + comment on one project's subtree of
/// `owner_org`, with no membership in `owner_org`.
pub struct DeclaredGrant {
    pub owner_org: &'static str,
    /// The declared project at the grant's root, by `dir`.
    pub root: &'static str,
    /// The person granted — a cast member of `guest_org`, never a bot.
    pub subject: &'static str,
    pub guest_org: &'static str,
    /// The owner or admin of `owner_org` who granted it.
    pub granted_by: &'static str,
    pub capabilities: share_proto::GrantCapabilities,
}

/// The seed's grants: Jordan of Rockstars of Tomorrow on Crescendum.
/// Live (no expiry, never revoked), so the demo shows the shared album
/// in both orgs and the lane cards have a guest to admit.
pub const SEED_GRANTS: &[DeclaredGrant] = &[DeclaredGrant {
    owner_org: "fasttrackaudio",
    root: "Crescendum",
    subject: "jordan@rockstars.test",
    guest_org: "rockstars-of-tomorrow",
    granted_by: "taylor@fasttrackaudio.test",
    capabilities: share_proto::GrantCapabilities::ReadComment,
}];

/// The grants one org owns.
pub fn grants_of(slug: &str) -> impl Iterator<Item = &'static DeclaredGrant> + '_ {
    SEED_GRANTS.iter().filter(move |g| g.owner_org == slug)
}

/// The declared projects of one org.
pub fn declared_of(slug: &str) -> impl Iterator<Item = &'static DeclaredProject> {
    DECLARED.iter().filter(move |p| p.org == slug)
}

/// A machine an example org has paired, and what it keeps of one project.
///
/// The seed's half of `files.device.identity`: a demo user opening the
/// devices list should see more than "this device", and should see two
/// machines that took *different* halves of the same album — which is
/// the whole point of subscribing per device. Each is enrolled the way
/// the app enrols one (by the endpoint id it holds) and its subscription
/// recorded under its own device row.
pub struct DeclaredDevice {
    pub org: &'static str,
    pub name: &'static str,
    /// The device's endpoint key is derived from this byte, so a replant
    /// enrols the same machine rather than a new one (enrolment is keyed
    /// by endpoint) and the seed stays reproducible. Distinct per device.
    pub key_seed: u8,
    /// The declared project, by title, whose File Root it subscribes to.
    pub project: &'static str,
    /// The facets it takes — disjoint between the two, on purpose.
    pub facets: &'static [&'static str],
}

/// The example's paired machines: a mix room that keeps the sessions, and
/// a producer's laptop that keeps only the mixes.
pub const DECLARED_DEVICES: &[DeclaredDevice] = &[
    DeclaredDevice {
        org: "acme-audio",
        name: "Mix Room iMac",
        key_seed: 0xA1,
        project: "Example Album",
        facets: &["sessions"],
    },
    DeclaredDevice {
        org: "acme-audio",
        name: "Producer's Laptop",
        key_seed: 0xA2,
        project: "Example Album",
        facets: &["mixes"],
    },
];

/// The paired machines of one org.
pub fn devices_of(slug: &str) -> impl Iterator<Item = &'static DeclaredDevice> {
    DECLARED_DEVICES.iter().filter(move |d| d.org == slug)
}

impl DeclaredDevice {
    /// The endpoint id this machine presents — a real ed25519 public key,
    /// so anything that parses it as one (the device sweep dials admitted
    /// peers) gets an address, not an error. Nobody holds the machine, so
    /// a dial finds nothing, which is the ordinary state of a laptop that
    /// is shut.
    #[must_use]
    pub fn endpoint(&self) -> String {
        architect::iroh_link::iroh::SecretKey::from_bytes(&[self.key_seed; 32])
            .public()
            .to_string()
    }
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
/// that preceded them, so the example carries six across all three orgs
/// — three owned by the audio studio, one by the video company, two
/// personal, spanning all three visibilities.
///
/// The video company's is the one that is not on the reader's disk. In
/// the demo ACME and VNT are two servers, so `vnt.test/post-production`
/// is the seed's cross-server subscription: reaching it goes through a
/// grant, a peer table and a dial rather than a sibling directory.
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
        org: "vnt-video",
        title: "Post Production",
        visibility: Visibility::Unlisted,
        demonstrates: "the source of a **cross-server** subscription: the one declared \
                       wiki owned by the other company, which in the demo runs as its \
                       own server — so ACME reaching it exercises the peer table, the \
                       grant and the dialler rather than a sibling directory",
    },
    DeclaredWiki {
        org: "alice-personal",
        title: "Bible Study",
        visibility: Visibility::Private,
        demonstrates: "a person's own study: a wiki that annotates a Resource without \
                       writing into it — every page anchors to a VerseId, so it survives a \
                       translation swap — holding only what she has studied, written by her \
                       or taken from the Bible Study Library once she stands behind it",
    },
    DeclaredWiki {
        org: "alice-personal",
        title: "Bible Study Library",
        visibility: Visibility::Private,
        demonstrates: "the working half of a person's study: an agent-written library of \
                       sources, summaries, topics, words and paths, from which a page is \
                       taken into Bible Study (a promotion — copied, recorded on both ends) \
                       once it has been studied enough",
    },
    DeclaredWiki {
        org: "acme-audio",
        title: "Cooking",
        visibility: Visibility::Private,
        demonstrates: "the studio's cookbook — the recipes the Planner's meal blocks plan \
                       from (`MEALS`), so a demo day has breakfast, lunch and dinner on it",
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

/// Alice's study pair, as `(library, study)` slugs in `alice-personal`:
/// the agent-written Bible Study Library, and her own Bible Study that
/// a page is taken into once she has studied it. The same promotion as
/// [`PROMOTION_PAIR`], told in a person's own org.
pub const STUDY_PAIR: (&str, &str) = ("bible-study-library", "bible-study");

/// A library page ready to take into the study: a `passage` (a type
/// the study declares) that links library pages the study does not
/// hold, so the taken copy's links have to point back at the library.
pub const SEED_STUDY_PAGE: &str = "Passages/Psalm 82.md";

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

/// Which tier an example asset is planted on — ADR 0004's first
/// decision, in the seed.
///
/// The two are not variants of one storage scheme; they are the whole
/// distinction the ADR draws. A **Resource** is an import: it sits
/// outside the vault tree, it does not change, and it is a leaf in the
/// graph. An **Asset** is a vault item on the `Assets/` shelf: it is
/// edited, by more than one person, and it inherits collaboration,
/// tags, wikilinks and search from being a vault file rather than
/// having any of them built.
///
/// Charts are the kind that moved, because they are the kind people
/// edit. Patches, samples and lighting stay on the resources tier for
/// now: they are binary payloads nobody types into, and ADR 0004 does
/// not move them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetTier {
    /// `<org>/resources/<library>/` — planted from `Resources/**`.
    Resources,
    /// `<org>/assets/<group>/` — planted from `AssetGroups/**`, where
    /// `library` is the group's name (`charts`, `songs`).
    ///
    /// Its own root, a sibling of `vault/` rather than a directory
    /// inside it. An earlier draft filed these at
    /// `<vault>/Assets/<Kind>/` to obtain per-file CRDT, which the
    /// vault does not in fact confer — registration does
    /// (`org_proto::shelf`) — and which cost the thing that actually
    /// matters here: a vault is never subscribable, so a song library
    /// inside one is a song library no other organisation can take.
    Assets,
}

/// One asset the example plants, on whichever tier ADR 0004 gives its
/// kind.
///
/// ADR 0003 gave four kinds a home under `resources/` — a Keyflow
/// chart, a Signal patch, a Signal sample, an Ignition lighting
/// document — and said a *library* of any of them is an ordinary
/// `Collection` of kind `Library` over `<kind>:<slug>` references. ADR
/// 0004 kept the library claim and moved charts to the vault. Both
/// claims are only checkable from a planted world if the planted world
/// actually holds one of each, so it does.
///
/// Planting needs no code: [`plant`] copies `Resources/**` into the
/// org's `resources/` tier and `Vault/**` into its vault, verbatim.
/// What this declaration buys is the contract — [`declared_tests`]
/// fails if a declared asset has no committed tree, or if a
/// resource-tier directory name drifts from the one
/// `node_homes::library_of` resolves cross-org references through.
#[derive(Debug, Clone, Copy)]
pub struct DeclaredAsset {
    /// The org that holds it.
    pub org: &'static str,
    /// Which tier it is planted on.
    pub tier: AssetTier,
    /// On [`AssetTier::Resources`]: the directory under `resources/` —
    /// and therefore the subscription slug a cross-org reader names,
    /// fixed by `node_homes::library_of` (`patches`, `samples`,
    /// `lighting`).
    ///
    /// On [`AssetTier::Assets`]: the asset group's name (`charts`,
    /// `songs`) — which is *also* the subscription slug another
    /// organisation names, because each group is its own shelf. Fixed
    /// by `resources_proto::assets`.
    pub library: &'static str,
    /// The node id: `chart:<slug>`, `patch:<slug>`, and so on.
    pub slug: &'static str,
    /// Path of the manifest under the tier's committed directory. A
    /// chart is flat (`<slug>.md`); the resource kinds own a directory
    /// (`<slug>/patch.md`).
    pub manifest: &'static str,
    /// The file holding the asset's own document, beside the manifest —
    /// empty for an asset that *is* one document, which is every vault
    /// asset. A chart carries its source in a ` ```keyflow ` fence in
    /// its own body, because a `.kf` beside it would be a file the
    /// vault walker never collects and therefore a file nobody could
    /// search, link or collaborate on.
    pub body: &'static str,
    /// One line on what this asset exists in the seed to prove.
    pub demonstrates: &'static str,
}

/// Every resource-tier asset the example plants. One of each kind, in
/// the studio org, tied to the album the rest of the seed is about.
pub const DECLARED_ASSETS: &[DeclaredAsset] = &[
    DeclaredAsset {
        org: "acme-audio",
        tier: AssetTier::Assets,
        library: resources_proto::assets::SONGS_KIND,
        slug: "track-one",
        manifest: "track-one.md",
        body: "",
        demonstrates: "a song as a shelf document — the thing charts are arrangements *of*, \
                       with its two arrangements joined to it by their own `song:` key rather \
                       than nested inside it; the audio it names stays on the resources tier, \
                       which is the ADR 0004 tier rule visible in one directory listing",
    },
    DeclaredAsset {
        org: "acme-audio",
        tier: AssetTier::Assets,
        library: resources_proto::assets::SONGS_KIND,
        slug: "track-two",
        manifest: "track-two.md",
        body: "",
        demonstrates: "the ordinary case beside it: one song, one arrangement, and nothing \
                       extra to express it",
    },
    DeclaredAsset {
        org: "acme-audio",
        tier: AssetTier::Assets,
        library: resources_proto::assets::CHARTS_KIND,
        slug: "track-one",
        manifest: "track-one.md",
        body: "",
        demonstrates: "ADR 0004 decision 1, planted: a Keyflow chart as an ordinary vault \
                       document on the `Assets/` shelf, so a demo user can open it in two \
                       tabs and watch it converge — collaboration inherited from being a \
                       vault file rather than built for charts. It is also \
                       `song:track-one`'s *default* arrangement, and \
                       `chart:track-one#chorus` addresses the section in its fence",
    },
    DeclaredAsset {
        org: "acme-audio",
        tier: AssetTier::Assets,
        library: resources_proto::assets::CHARTS_KIND,
        slug: "track-one-condensed-live",
        manifest: "track-one-condensed-live.md",
        body: "",
        demonstrates: "one chart is one arrangement: a second reading of the *same* song, \
                       joined to it by `song: song:track-one` and told apart by its \
                       `arrangement` label — not a revision of the chart beside it, and not \
                       the default, so a demo user can see both halves of the invariant in \
                       the planted world",
    },
    DeclaredAsset {
        org: "acme-audio",
        tier: AssetTier::Assets,
        library: resources_proto::assets::CHARTS_KIND,
        slug: "track-two",
        manifest: "track-two.md",
        body: "",
        demonstrates: "the second chart, because one chart is a file and two are a \
                       library — `Chart Library` in the seed collects both, and a demo \
                       user opening the planted org finds a list rather than an orphan",
    },
    DeclaredAsset {
        org: "acme-audio",
        tier: AssetTier::Resources,
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
        tier: AssetTier::Resources,
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
        tier: AssetTier::Resources,
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
        tier: AssetTier::Resources,
        library: "samples",
        slug: "single-master-loop",
        manifest: "single-master-loop/sample.md",
        body: "single-master-loop/sample.json",
        demonstrates: "the bound twin: at plant time the seed makes a root through \
                       `RootsService::create`, saves the audio through the upload lane with \
                       `files_client`, and pins the manifest to exactly those bytes — Task \
                       as an app's store, reachable from the planted world",
    },
    DeclaredAsset {
        org: "acme-audio",
        tier: AssetTier::Resources,
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
/// libraries*: a library is a `Collection` of kind `"library"` over node
/// references, and a setlist is the same primitive with a different
/// kind string. ADR 0004 sharpened that — the kind is a word this
/// example supplies, not one the store enumerates — which makes the
/// seed the demonstration that an application can name its own domain
/// without a Task release. A seed that planted four assets and no collection would leave
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
    /// The kind label, which this example defines and Task does not.
    ///
    /// It reads `library` and `setlist` because that is what a studio
    /// calls these; after ADR 0004 those are strings the seed picked,
    /// not members of a vocabulary the store publishes. Written in the
    /// normal form [`collection::CollectionKind::new`] produces —
    /// lowercase, trimmed — so a declaration matches what the store
    /// holds.
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
        demonstrates: "a chart library is a `Collection` of kind `\"library\"` over \
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
    DeclaredCollection {
        org: "acme-audio",
        title: "Sunday Songs",
        // Keyflow's word, exactly: `SONGLIST_KIND` in keyflow's
        // `apps/web/src/library/vox.rs`. Its library page lists
        // collections of this kind and nothing else, so without this row
        // a demo user signing into Keyflow against the seed would see a
        // library with no lists in it and nothing to build a setlist from.
        kind: "songlist",
        items: &[("song", "track-one"), ("song", "track-two")],
        demonstrates: "Keyflow's library organised the way a worship leader keeps one — \
                       songs gathered into named lists, by reference, so a song sits in \
                       as many lists as it belongs to and opening it from any of them \
                       opens its default chart. `tests/integration/tests/it/\
                       keyflow_library.rs` builds setlists by drawing on lists like this",
    },
    // Rockstars of Tomorrow: setlists drawn from the Adult Jam library,
    // then shows made of setlists. Every song here is one the chordsheet
    // import plants (`CHORDSHEET_LIBRARIES`); `declared_tests` holds that.
    DeclaredCollection {
        org: "rockstars-of-tomorrow",
        title: "Opening Set",
        kind: "setlist",
        items: &[
            ("song", "mr-brightside"),
            ("song", "livin-on-a-prayer"),
            ("song", "jessies-girl"),
            ("song", "uptown-funk"),
        ],
        demonstrates: "a setlist drawn from a song list: every entry is a song the Adult \
                       Jam list already holds, referenced rather than copied",
    },
    DeclaredCollection {
        org: "rockstars-of-tomorrow",
        title: "Grunge Set",
        kind: "setlist",
        items: &[
            ("song", "come-as-you-are"),
            ("song", "basket-case"),
            ("song", "kryptonite"),
            ("song", "somebody-told-me"),
        ],
        demonstrates: "a second setlist from the same library — a song list is where \
                       setlists are picked from, not a setlist itself",
    },
    DeclaredCollection {
        org: "rockstars-of-tomorrow",
        title: "Closing Set",
        kind: "setlist",
        items: &[
            ("song", "rolling-in-the-deep"),
            ("song", "africa"),
            ("song", "dont-stop-believin"),
            ("song", "thunderstruck"),
        ],
        demonstrates: "the set a show ends on, and one that more than one show uses",
    },
    DeclaredCollection {
        org: "rockstars-of-tomorrow",
        title: "Spring Showcase",
        kind: "show",
        // By title: a collection's id is minted when it is created — see
        // `plant_collections`.
        items: &[
            ("collection", "Opening Set"),
            ("collection", "Grunge Set"),
            ("collection", "Closing Set"),
        ],
        demonstrates: "a show is a collection of setlists, in running order — the \
                       `collection:` node kind, and nothing else new: the same ordering, \
                       the same references, one level up",
    },
    DeclaredCollection {
        org: "rockstars-of-tomorrow",
        title: "Summer Block Party",
        kind: "show",
        items: &[("collection", "Opening Set"), ("collection", "Closing Set")],
        demonstrates: "two shows sharing setlists: a setlist tightened for the spring \
                       showcase is tightened for the block party too, because neither show \
                       holds a copy of it",
    },
    DeclaredCollection {
        org: "acme-audio",
        title: "Thursday Rehearsal",
        // The point of this row is the word. `rehearsal-pool` is not a
        // kind Task has ever enumerated, and after ADR 0004 there is no
        // list for it to be missing from — the seed just picked it.
        kind: "rehearsal-pool",
        items: &[
            ("song", "track-two"),
            ("song", "track-three"),
            ("chart", "track-two"),
        ],
        demonstrates: "ADR 0004's test of decision 2, in the planted world rather than \
                       only in a test: an application naming something in its own domain \
                       — a rehearsal pool, which no version of Task has ever heard of — \
                       and getting ordering, membership and reference resolution anyway. \
                       If a demo user can build this, so can a seventh app, without a \
                       release of the store it is built on",
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

/// An app document the demo org holds (`crdt::docs`, served by
/// [`crate::app_docs`]), as a client app such as Studio would have made
/// it. The server never reads one, so the content planted here is only
/// a starting state, shaped the way the demo's Studio expects: a `meta`
/// map with a `title`, and for a setlist a `songs` list of song slugs.
pub struct DeclaredAppDoc {
    pub org: &'static str,
    /// Fixed, so a replant registers the same doc.
    pub id: &'static str,
    /// The app's word for it.
    pub kind: &'static str,
    pub name: &'static str,
    /// The declared doc this one is a branch of.
    pub parent: Option<&'static str>,
    /// Song slugs, for a setlist.
    pub songs: &'static [&'static str],
    /// `Merge` for a doc every device makes under one derived id (a
    /// rig's library); `Strict` for one made once and adopted.
    pub lineage: crdt::docs::Lineage,
    /// The project it belongs to, by its path on the Projects tier
    /// ([`project_rel`]): planted as the `project:` → `doc:` link a
    /// project page lists (`crate::app_docs::attach`).
    pub project: Option<&'static str>,
}

pub static DECLARED_APP_DOCS: &[DeclaredAppDoc] = &[
    DeclaredAppDoc {
        org: "acme-audio",
        id: "6f1d3a52-6e0b-4f6e-9a51-1c0e7d2a9b01",
        kind: "setlist",
        name: "Album Launch Set",
        parent: None,
        songs: &["track-one", "track-two"],
        lineage: crdt::docs::Lineage::Strict,
        project: Some("example-album"),
    },
    DeclaredAppDoc {
        org: "acme-audio",
        id: "6f1d3a52-6e0b-4f6e-9a51-1c0e7d2a9b02",
        kind: "session",
        name: "Track One",
        parent: None,
        songs: &[],
        lineage: crdt::docs::Lineage::Strict,
        project: Some("example-album"),
    },
    DeclaredAppDoc {
        org: "acme-audio",
        id: "6f1d3a52-6e0b-4f6e-9a51-1c0e7d2a9b03",
        kind: "session",
        name: "Track One (acoustic)",
        parent: Some("6f1d3a52-6e0b-4f6e-9a51-1c0e7d2a9b02"),
        songs: &[],
        lineage: crdt::docs::Lineage::Strict,
        project: Some("example-album"),
    },
    // A rig's library, as Studio keeps it: one id every device derives,
    // made on each offline and merged when they meet.
    DeclaredAppDoc {
        org: "acme-audio",
        id: "6f1d3a52-6e0b-4f6e-9a51-1c0e7d2a9b04",
        kind: "rig",
        name: "Bass rig",
        parent: None,
        songs: &[],
        lineage: crdt::docs::Lineage::Merge,
        project: None,
    },
];

pub fn app_docs_of(slug: &str) -> impl Iterator<Item = &'static DeclaredAppDoc> + '_ {
    DECLARED_APP_DOCS.iter().filter(move |d| d.org == slug)
}

fn app_doc_id(id: &str) -> eyre::Result<uuid::Uuid> {
    id.parse()
        .map_err(|e| eyre::eyre!("declared app doc id `{id}`: {e}"))
}

/// Plant `slug`'s declared app docs into `<org>/app_docs.sqlite`: each
/// registered (parents first, as the list is ordered), attached to its
/// project (a link in `<org>/links.jsonl`), and an empty doc given its
/// starting content.
///
/// Idempotent: registering is idempotent on the fixed id, and a doc that
/// already holds history is left alone.
///
/// # Errors
///
/// Opening or migrating the database, or a declared row the catalog
/// refuses.
pub async fn plant_app_docs(org_root: &org_proto::OrgRoot, slug: &str) -> eyre::Result<usize> {
    use crdt::docs::{CreateDoc, Docs as _};
    use sea_orm_migration::MigratorTrait as _;

    let declared: Vec<&DeclaredAppDoc> = app_docs_of(slug).collect();
    if declared.is_empty() {
        return Ok(0);
    }
    let path = org_root.app_docs_db();
    let db = sea_orm::Database::connect(format!("sqlite://{}?mode=rwc", path.display()))
        .await
        .map_err(|e| eyre::eyre!("open {}: {e}", path.display()))?;
    crdt_seaorm::Migrator::up(&db, None)
        .await
        .map_err(|e| eyre::eyre!("migrate app docs: {e}"))?;
    let host = crate::app_docs::open(db).await?;
    let links = links::Store::open(org_root.path().join("links.jsonl"));
    let mut planted = 0;
    for d in declared {
        let doc_id = app_doc_id(d.id)?;
        host.create(CreateDoc {
            doc_id,
            kind: d.kind.into(),
            name: d.name.into(),
            parent: d.parent.map(app_doc_id).transpose()?,
            lineage: d.lineage,
        })
        .await
        .map_err(|e| eyre::eyre!("register \"{}\": {e}", d.name))?;
        // Its project's link; attaching again returns the one held.
        if let Some(project) = d.project {
            let project = crate::app_docs::project_node(project).map_err(|e| eyre::eyre!(e))?;
            crate::app_docs::attach(&host, &links, &project, doc_id, "seed")
                .await
                .map_err(|e| eyre::eyre!("attach \"{}\": {e}", d.name))?;
        }
        let doc = host
            .registry()
            .doc(doc_id)
            .await
            .map_err(|e| eyre::eyre!("open \"{}\": {e}", d.name))?;
        if !doc.loro().oplog_vv().is_empty() {
            continue;
        }
        let meta = doc.loro().get_map("meta");
        meta.insert("title", d.name)
            .map_err(|e| eyre::eyre!("seed \"{}\": {e}", d.name))?;
        if !d.songs.is_empty() {
            let songs = doc.loro().get_list("songs");
            for slug in d.songs {
                songs
                    .push(*slug)
                    .map_err(|e| eyre::eyre!("seed \"{}\": {e}", d.name))?;
            }
        }
        doc.loro().commit();
        // Written straight through: the update writer is asynchronous.
        doc.compact(doc_id)
            .await
            .map_err(|e| eyre::eyre!("persist \"{}\": {e}", d.name))?;
        planted += 1;
    }
    Ok(planted)
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
    // Title → id, for a declared item that names another collection. A
    // collection's id is minted when it is created, so a declaration
    // cannot know it; it names the sibling by title instead, and a show
    // is declared after the setlists it holds so they exist by then.
    let mut ids: std::collections::HashMap<String, String> = held
        .iter()
        .map(|c| (c.title.clone(), c.id.clone()))
        .collect();
    for d in declared {
        if held.iter().any(|c| c.title == d.title) {
            continue;
        }
        // No match arm any more: a kind is the declaring app's own word,
        // and this seeder is one such app (ADR 0004). `library` and
        // `setlist` are strings the example chose, not a vocabulary Task
        // offers.
        let kind = CollectionKind::new(d.kind);
        let made = match store.create(slug.to_owned(), d.title.to_owned(), kind) {
            Ok(made) => made,
            Err(e) => {
                tracing::warn!(org.slug = %slug, collection = d.title, "not created: {e}");
                continue;
            }
        };
        ids.insert(made.title.clone(), made.id.clone());
        for (kind, id) in d.items {
            let Some(kind) = NodeKind::parse(kind) else {
                tracing::warn!(collection = d.title, "`{kind}` is not a node kind");
                continue;
            };
            // A collection is named by title in a declaration; see `ids`.
            let id = if kind == NodeKind::Collection {
                match ids.get(*id) {
                    Some(real) => real.clone(),
                    None => {
                        tracing::warn!(
                            collection = d.title,
                            item = id,
                            "names no collection declared before it"
                        );
                        continue;
                    }
                }
            } else {
                (*id).to_owned()
            };
            if let Err(e) = store.add_item(Placement {
                collection_id: made.id.clone(),
                node: NodeRef::new(kind, &id),
                after: None,
            }) {
                tracing::warn!(collection = d.title, item = %id, "not collected: {e}");
            }
        }
    }
}

// ── Chordsheet libraries ─────────────────────────────────────────────

/// A chord-chart backup the seed imports into an org's library.
///
/// The backup is committed as the tool that made it left it — a
/// `manifest.json` and a `kf/` folder of Keyflow source — and lands in the
/// org's files like any other folder on a disk. Planting then turns it into
/// what Keyflow would have made of it had somebody imported it by hand: a
/// song per entry, a chart per song, and one song list holding them all.
#[derive(Debug, Clone, Copy)]
pub struct ChordsheetLibrary {
    pub org: &'static str,
    /// The backup folder, relative to the org's directory in the example.
    pub backup: &'static str,
    /// The song list every imported song goes into.
    pub songlist: &'static str,
    /// One line on what this library exists in the seed to prove.
    pub demonstrates: &'static str,
}

/// Every chordsheet library the example plants.
pub const CHORDSHEET_LIBRARIES: &[ChordsheetLibrary] = &[ChordsheetLibrary {
    org: "rockstars-of-tomorrow",
    backup: "Library/Chordsheet Backup",
    songlist: "Adult Jam",
    demonstrates: "a real working library at a real size — 204 charts from a jam program's \
                   chordsheet.com backup, in one song list, with setlists drawn from it \
                   and a show that holds those setlists. The one-song examples elsewhere in \
                   the seed prove each mechanism; this is what they are for",
}];

/// One entry of a backup's `manifest.json` — the fields the import reads.
#[derive(Debug, serde::Deserialize)]
struct BackupEntry {
    title: String,
    artist: String,
    source_file: String,
}

/// One imported song: its document, its chart, and both slugs.
#[derive(Debug, Clone)]
pub struct ImportedSong {
    pub song_slug: String,
    pub song: resources_proto::SongDoc,
    pub chart_slug: String,
    pub chart: resources_proto::ChartDoc,
}

/// What a backup imports as, in manifest order — pure, so the planter and
/// the seed's own tests derive the same slugs.
///
/// Slugs come from the lane's own rules (`resources::song::slug_for`,
/// `resources::chart::slug_for`) and are derived **from the manifest
/// alone**, never from what is already on disk. That is what makes a
/// re-plant name the same songs: seeding the taken-list from disk would
/// see this function's own earlier output and mint `-2` duplicates of
/// every song. Two entries that share a title (two different "Holiday"s)
/// are told apart exactly as they would be by an import through the
/// lane.
///
/// # Errors
///
/// A backup that is not in the example, a manifest that will not parse, or
/// an entry whose `.kf` is missing.
pub fn chordsheet_songs(lib: &ChordsheetLibrary) -> Result<Vec<ImportedSong>, String> {
    let dir = format!("{}/{}", lib.org, lib.backup);
    let manifest = STUDIO
        .get_file(format!("{dir}/manifest.json"))
        .ok_or_else(|| format!("{dir}: no manifest.json"))?;
    let entries: Vec<BackupEntry> = serde_json::from_slice(manifest.contents())
        .map_err(|e| format!("{dir}/manifest.json: {e}"))?;

    let (mut songs_taken, mut charts_taken) = (Vec::new(), Vec::new());
    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        let stem = entry
            .source_file
            .rsplit_once('.')
            .map_or(entry.source_file.as_str(), |(stem, _)| stem);
        let kf = STUDIO
            .get_file(format!("{dir}/kf/{stem}.kf"))
            .ok_or_else(|| format!("{dir}: no kf/{stem}.kf for `{}`", entry.title))?;
        let source = String::from_utf8_lossy(kf.contents()).into_owned();

        let song = resources_proto::SongDoc {
            revision: String::new(),
            slug: String::new(),
            title: entry.title.trim().to_owned(),
            writers: vec![entry.artist.trim().to_owned()],
            key: String::new(),
            tags: Vec::new(),
            updated_at: "2026-09-21T00:00:00Z".to_owned(),
        };
        let song_slug = resources::song::slug_for(&songs_taken, &song);
        songs_taken.push(song_slug.clone());

        // As Keyflow saves one: the source verbatim, the song named by
        // token, and — being each song's only chart — its default.
        let chart = resources_proto::ChartDoc {
            revision: String::new(),
            title: song.title.clone(),
            source,
            song: links::NodeRef::song(&song_slug).to_token(),
            is_default: true,
            updated_at: song.updated_at.clone(),
            ..Default::default()
        };
        let chart_slug = resources::chart::slug_for(&charts_taken, &chart);
        charts_taken.push(chart_slug.clone());

        out.push(ImportedSong {
            song_slug,
            song,
            chart_slug,
            chart,
        });
    }
    Ok(out)
}

/// Import an org's chordsheet library into its song and chart shelves, and
/// gather it into its song list.
///
/// Documents are rendered by the chart and song lanes' own renderers and
/// written onto the shelves the lanes write to, which is how the rest of
/// the seed plants its charts — committed documents on the shelf — without
/// a second copy of the server's wiring. Idempotent like the rest of the
/// plant: a document already there is left alone (somebody may have edited
/// the chart since), and the song list is created only if the org has no
/// list of that title.
fn plant_chordsheet_library(org_root: &org_proto::OrgRoot, slug: &str) {
    for lib in CHORDSHEET_LIBRARIES.iter().filter(|l| l.org == slug) {
        let songs = match chordsheet_songs(lib) {
            Ok(songs) => songs,
            Err(e) => {
                tracing::warn!(org.slug = %slug, "chordsheet library not planted: {e}");
                continue;
            }
        };
        let song_shelf = org_root.asset_shelf_dir(resources_proto::assets::SONGS_KIND);
        let chart_shelf = org_root.asset_shelf_dir(resources_proto::assets::CHARTS_KIND);
        for s in &songs {
            let targets = [
                (
                    song_shelf.join(resources_proto::assets::song_path(&s.song_slug)),
                    resources::song::render_document(&s.song, &s.song_slug)
                        .map_err(|e| e.to_string()),
                ),
                (
                    chart_shelf.join(resources_proto::assets::chart_path(&s.chart_slug)),
                    resources::chart::render_document(&s.chart, &s.chart_slug)
                        .map_err(|e| e.to_string()),
                ),
            ];
            for (path, doc) in targets {
                if path.exists() {
                    continue;
                }
                let written = doc.and_then(|doc| {
                    if let Some(parent) = path.parent() {
                        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    }
                    std::fs::write(&path, doc).map_err(|e| e.to_string())
                });
                if let Err(e) = written {
                    tracing::warn!(path = %path.display(), "not planted: {e}");
                }
            }
        }
        #[cfg(feature = "plugin-fasttrackstudio")]
        plant_songlist(org_root, slug, lib.songlist, &songs);
    }
}

/// The song list a chordsheet library gathers into, created once.
#[cfg(feature = "plugin-fasttrackstudio")]
fn plant_songlist(org_root: &org_proto::OrgRoot, slug: &str, title: &str, songs: &[ImportedSong]) {
    use collection::{CollectionKind, CollectionService as _, NodeRef, Placement};

    let store = collection::Store::open(collections_path(org_root));
    let held = store.list(slug.to_owned(), None).unwrap_or_default();
    if held.iter().any(|c| c.title == title) {
        return;
    }
    // Keyflow's word for it — `SONGLIST_KIND` in keyflow's library.
    let list = match store.create(
        slug.to_owned(),
        title.to_owned(),
        CollectionKind::new("songlist"),
    ) {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!(org.slug = %slug, list = title, "not created: {e}");
            return;
        }
    };
    for s in songs {
        if let Err(e) = store.add_item(Placement {
            collection_id: list.id.clone(),
            node: NodeRef::song(&s.song_slug),
            after: None,
        }) {
            tracing::warn!(list = title, song = %s.song_slug, "not listed: {e}");
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
        let _git = tracing::info_span!("plant.repo_wiki.git").entered();
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
        drop(_git);
        let _sync = tracing::info_span!("plant.repo_wiki.first_sync").entered();
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

    /// The declared app docs plant into the org's catalog with their
    /// lineage, every parent declared before its branch, and planting
    /// again changes nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn declared_app_docs_plant_once_with_their_lineage() {
        use crdt::docs::{DocQuery, Docs as _};
        use sea_orm_migration::MigratorTrait as _;

        for (i, d) in DECLARED_APP_DOCS.iter().enumerate() {
            crdt::docs::check_kind(d.kind).expect("a valid kind");
            if let Some(parent) = d.parent {
                assert!(
                    DECLARED_APP_DOCS[..i]
                        .iter()
                        .any(|p| p.id == parent && p.org == d.org),
                    "\"{}\" names a parent declared before it",
                    d.name
                );
            }
        }
        let tmp = tempfile::tempdir().unwrap();
        let root = org_proto::DataRoot::new(tmp.path().to_owned());
        let org = root.init_org("acme-audio", "ACME Audio", true).unwrap();
        let declared = app_docs_of("acme-audio").count();
        assert_eq!(plant_app_docs(&org, "acme-audio").await.unwrap(), declared);
        assert_eq!(plant_app_docs(&org, "acme-audio").await.unwrap(), 0);

        let db = sea_orm::Database::connect(format!(
            "sqlite://{}?mode=rwc",
            org.app_docs_db().display()
        ))
        .await
        .unwrap();
        crdt_seaorm::Migrator::up(&db, None).await.unwrap();
        let host = crate::app_docs::open(db).await.unwrap();
        let rows = host.list(DocQuery::default()).await.unwrap().docs;
        assert_eq!(rows.len(), declared, "one row per declared doc");
        let branch = DECLARED_APP_DOCS
            .iter()
            .find(|d| d.parent.is_some())
            .expect("a declared branch");
        let row = rows
            .iter()
            .find(|r| r.doc_id.to_string() == branch.id)
            .unwrap();
        assert_eq!(
            row.parent.map(|p| p.to_string()).as_deref(),
            branch.parent,
            "the branch carries its parent"
        );
        let doc = host.registry().doc(row.doc_id).await.unwrap();
        assert!(
            doc.loro().get_map("meta").get("title").is_some(),
            "the planted doc holds its starting content"
        );
        for d in app_docs_of("acme-audio") {
            let row = rows.iter().find(|r| r.doc_id.to_string() == d.id).unwrap();
            assert_eq!(row.lineage, d.lineage, "\"{}\" keeps its lineage", d.name);
        }
        assert!(
            app_docs_of("acme-audio").any(|d| d.lineage == crdt::docs::Lineage::Merge),
            "the seed shows a Merge doc (a rig's library)"
        );

        // Each doc that names a project is attached to a declared one,
        // once, however often the plant runs.
        let links = links::Store::open(org.path().join("links.jsonl"));
        for d in DECLARED_APP_DOCS {
            let Some(project) = d.project else { continue };
            assert!(
                declared_of(d.org).any(|p| project_rel(p.dir) == project),
                "\"{}\" names `{project}`, which {} does not declare",
                d.name,
                d.org
            );
        }
        let album = crate::app_docs::project_node("example-album").unwrap();
        let held = crate::app_docs::project_docs(&host, &links, &album)
            .await
            .unwrap();
        let mut ids: Vec<String> = held.iter().map(|h| h.link.target.id.clone()).collect();
        ids.sort();
        let mut want: Vec<String> = app_docs_of("acme-audio")
            .filter(|d| d.project == Some("example-album"))
            .map(|d| d.id.to_owned())
            .collect();
        want.sort();
        assert_eq!(ids, want, "one link per attached doc");
        assert!(
            held.iter().all(|h| h.meta.is_some()),
            "every one in the catalog"
        );
    }

    /// The paired machines are a contract like the projects are: each
    /// names a project its org declares (so the seeder finds a root to
    /// subscribe it to), no two share a key (one machine would become two
    /// rows), and within an org their facets are disjoint — two devices
    /// taking the same thing would not show that a subscription is a
    /// device's own.
    #[test]
    fn declared_devices_are_distinct_machines_on_declared_projects() {
        for org in ORGS.iter().map(|(slug, _)| *slug) {
            let devices: Vec<_> = devices_of(org).collect();
            for d in &devices {
                assert!(
                    declared_of(org).any(|p| p.title == d.project),
                    "{}: {} is not a declared project of {org}",
                    d.name,
                    d.project
                );
                assert!(!d.facets.is_empty(), "{} keeps nothing", d.name);
            }
            for (i, a) in devices.iter().enumerate() {
                for b in &devices[i + 1..] {
                    assert_ne!(a.endpoint(), b.endpoint(), "{} and {}", a.name, b.name);
                    if a.project == b.project {
                        assert!(
                            a.facets.iter().all(|f| !b.facets.contains(f)),
                            "{} and {} take overlapping facets",
                            a.name,
                            b.name
                        );
                    }
                }
            }
        }
        assert!(
            DECLARED_DEVICES.len() >= 2,
            "the seed needs two devices to show they differ"
        );
    }

    /// The org's committed day templates, parsed the way the scheduler
    /// parses them.
    #[cfg(feature = "plugin-scheduling")]
    fn committed_templates(slug: &str) -> Vec<scheduling_proto::DayTemplate> {
        let Some(dir) = STUDIO.get_dir(format!("{slug}/Vault/Projects/Scheduling/templates"))
        else {
            return Vec::new();
        };
        dir.files()
            .filter(|f| f.path().extension().and_then(|e| e.to_str()) == Some("md"))
            .map(|f| {
                let path = f.path().display().to_string();
                let text = f.contents_utf8().expect("a template is utf-8");
                let (fm, _) = scheduling::scan::frontmatter_split(text)
                    .unwrap_or_else(|| panic!("{path}: no frontmatter"));
                scheduling::parse::parse_day_template(&path, fm)
                    .unwrap_or_else(|e| panic!("{path}: {e}"))
            })
            .collect()
    }

    /// Context / tag names (no sigil) some block of the org's committed
    /// templates brings forward — what a time block can open. `None`
    /// when this build has no scheduling to ask.
    #[allow(clippy::unnecessary_wraps)]
    fn surfaced_by_templates(slug: &str) -> Option<Vec<&'static str>> {
        #[cfg(feature = "plugin-scheduling")]
        {
            Some(
                committed_templates(slug)
                    .iter()
                    .flat_map(|t| t.blocks.iter().map(|b| b.category))
                    .flat_map(|c| c.surfaces().iter().copied())
                    .collect(),
            )
        }
        #[cfg(not(feature = "plugin-scheduling"))]
        {
            let _ = slug;
            None
        }
    }

    fn org_of_task(title: &str) -> &'static str {
        DECLARED
            .iter()
            .find(|p| p.tasks.iter().any(|(t, _, _)| *t == title))
            .unwrap_or_else(|| panic!("`{title}` is not a declared task"))
            .org
    }

    /// Every context-gated task is a declared one, and its contexts are
    /// ones the app can satisfy: an "I'm at" place, a known time
    /// window, or a name one of the org's own day templates brings
    /// forward (a maintenance hour's `@errands`) — a gate nothing can
    /// open would hide the task for good.
    #[test]
    fn every_task_context_can_be_met() {
        const PLACES_AND_WINDOWS: &[&str] = &[
            "@home",
            "@studio",
            "@out",
            "@morning",
            "@mealprep",
            "@evening",
            // Devices — every client fills in what it runs on.
            "@phone",
            "@computer",
        ];
        for (title, contexts) in TASK_CONTEXTS {
            let org = org_of_task(title);
            for c in *contexts {
                if PLACES_AND_WINDOWS.contains(c) {
                    continue;
                }
                let Some(surfaced) = surfaced_by_templates(org) else {
                    continue;
                };
                assert!(
                    surfaced.iter().any(|s| c.strip_prefix('@') == Some(*s)),
                    "`{title}`: nothing can satisfy `{c}` — not a place, not a \
                     time window, and no block in {org}'s day templates surfaces it"
                );
            }
        }
    }

    /// The same contract for tags: a seeded `#chore` is only worth
    /// carrying if some block of the org's day brings it forward.
    #[test]
    fn every_task_tag_is_brought_forward_by_a_block() {
        for (title, tags) in TASK_TAGS {
            let org = org_of_task(title);
            let Some(surfaced) = surfaced_by_templates(org) else {
                continue;
            };
            for tag in *tags {
                assert!(
                    !tag.starts_with('#'),
                    "`{title}`: tags are stored without `#`"
                );
                assert!(
                    surfaced.contains(tag),
                    "`{title}`: no block in {org}'s day templates surfaces `#{tag}`"
                );
            }
        }
    }

    // t[verify process.focus.steering]
    /// The seed has a maintenance hour, on both kinds of day, and it is
    /// for errands and chores — the block the task list leads with
    /// during it.
    #[cfg(feature = "plugin-scheduling")]
    #[test]
    fn the_committed_day_templates_carry_a_maintenance_hour() {
        use scheduling_proto::BlockCategory;
        let templates = committed_templates("acme-audio");
        for id in ["weekday", "weekend"] {
            let t = templates
                .iter()
                .find(|t| t.id.0 == id)
                .unwrap_or_else(|| panic!("acme-audio has no `{id}` template"));
            let hour = t
                .blocks
                .iter()
                .find(|b| b.category == BlockCategory::Maintenance)
                .unwrap_or_else(|| panic!("`{id}` has no maintenance block"));
            assert!(
                hour.start.minutes_since_midnight < hour.end.minutes_since_midnight,
                "`{id}`: the hour is empty"
            );
            for name in ["errands", "chores", "chore"] {
                assert!(
                    hour.category.surfaces().contains(&name),
                    "a maintenance block does not bring `{name}` forward"
                );
            }
        }
        let weekday = templates.iter().find(|t| t.id.0 == "weekday").unwrap();
        let hour = weekday
            .blocks
            .iter()
            .find(|b| b.category == BlockCategory::Maintenance)
            .unwrap();
        assert_eq!(
            (
                hour.start.minutes_since_midnight,
                hour.end.minutes_since_midnight
            ),
            (16 * 60, 17 * 60),
            "the weekday maintenance hour is 16:00–17:00"
        );
    }

    // t[verify process.focus.everywhere]
    // t[verify process.context.device]
    /// The seed gives Alice one due answer, and the committed maintenance
    /// hour reorders it: her declared board, as planted (owners,
    /// contexts, statuses, due dates relative to the plant), through the
    /// one due queue every surface calls (`task_proto::due_queue`). The
    /// work in hand leads; then outside the hour the late stems, inside
    /// the mode the live room's vacuum (a chore, due today). The phone's
    /// call waits on the computer and shows on the phone.
    #[cfg(feature = "plugin-scheduling")]
    #[test]
    fn the_seed_gives_alice_one_due_answer_that_the_hour_reorders() {
        use scheduling_proto::BlockCategory;
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 20).unwrap();
        let alice = "alice@acme.test";
        let board: Vec<task::TaskInfo> = declared_of("acme-audio")
            .flat_map(|p| p.tasks.iter())
            .map(|(title, status, due)| {
                let mut t = task::TaskInfo::new(*title);
                t.status = (*status).to_owned();
                t.due = due.map(|d| (today + chrono::Duration::days(d)).to_string());
                if let Some((_, cs)) = TASK_CONTEXTS.iter().find(|(n, _)| n == title) {
                    t.contexts = cs.iter().map(|c| (*c).to_owned()).collect();
                }
                if let Some((_, owner)) = TASK_OWNERS.iter().find(|(n, _)| n == title) {
                    t.workflow = Some(task::model::WorkflowAttrs {
                        assignees: task::model::AgentRefList(vec![
                            task::workflows_proto::AgentRef::Human {
                                user_id: (*owner).to_owned(),
                                display_name: None,
                            },
                        ]),
                        ..Default::default()
                    });
                }
                t
            })
            .collect();
        let weekday = committed_templates("acme-audio")
            .into_iter()
            .find(|t| t.id.0 == "weekday")
            .expect("a weekday template");
        let hour = weekday
            .blocks
            .iter()
            .find(|b| b.category == BlockCategory::Maintenance)
            .expect("a maintenance hour");
        let minute = hour.start.minutes_since_midnight + 5;
        // What the Suggested slot offers during the hour, and the name of
        // the session Start opens — the mode. A day with no saved plan
        // reads as its template (`scheduling_proto::merge_template`).
        let day: Vec<scheduling_proto::PlannedBlock> =
            scheduling_proto::merge_template(&[], &weekday.blocks)
                .into_iter()
                .map(|rb| rb.block)
                .collect();
        let mode = task::suggest::now_hint(&day, minute)
            .and_then(|h| h.mode)
            .expect("the maintenance hour suggests its mode");
        let at = |device: &str| {
            task::RelevanceContext::now(
                today.to_string(),
                Some(format!("{:02}:{:02}", minute / 60, minute % 60)),
                None,
                Some(device),
            )
        };
        let titles =
            |q: &[task::TaskInfo]| -> Vec<String> { q.iter().map(|t| t.title.clone()).collect() };
        let plain = task::due_queue(&board, Some(alice), &at("computer"));
        assert_eq!(
            titles(&plain),
            [
                "Comp lead vocals — Track One",
                "Send the label the stems",
                "Vacuum the live room"
            ],
            "outside the mode: the work in hand, then oldest due"
        );
        assert_eq!(task::overdue_count(&plain, &today.to_string()), 1);
        let in_mode = task::due_queue(
            &board,
            Some(alice),
            &at("computer").with_session(&mode.title, None),
        );
        assert_eq!(
            titles(&in_mode),
            [
                "Comp lead vocals — Track One",
                "Vacuum the live room",
                "Send the label the stems"
            ],
            "in the maintenance mode its chore leads the due work"
        );
        let call = board
            .iter()
            .find(|t| t.title == "Ring the piano tuner")
            .expect("the phone's call is declared");
        assert!(task::is_relevant(call, &at("phone")), "on the phone");
        assert!(
            !task::is_relevant(call, &at("computer")),
            "not on the computer"
        );
    }

    // t[verify process.capture.marks]
    /// The seed shows the inbox's marks: some capture names a person of
    /// its own org by `@handle` (an avatar), and some carry a `#tag`
    /// and an `@context` that is nobody (a badge and a chip).
    #[test]
    fn the_seeded_inbox_shows_people_tags_and_contexts() {
        let handle = |m: &Member| m.name.to_lowercase();
        let words = |c: &DeclaredCapture| {
            c.body
                .split_whitespace()
                .map(|w| {
                    w.trim_end_matches(|ch: char| !ch.is_alphanumeric())
                        .to_owned()
                })
                .collect::<Vec<_>>()
        };
        let mentions_member = INBOX.iter().any(|c| {
            words(c).iter().any(|w| {
                w.strip_prefix('@')
                    .is_some_and(|h| CAST.iter().any(|m| m.org == c.org && handle(m) == h))
            })
        });
        assert!(mentions_member, "no capture @-mentions someone in its org");
        let has_tag = INBOX
            .iter()
            .any(|c| words(c).iter().any(|w| w.starts_with('#')));
        let has_context = INBOX.iter().any(|c| {
            words(c).iter().any(|w| {
                w.strip_prefix('@')
                    .is_some_and(|h| !CAST.iter().any(|m| handle(m) == h))
            })
        });
        assert!(
            has_tag && has_context,
            "the seed needs a #tag and an @context"
        );
    }

    /// Every planned meal names a recipe the org's Cooking wiki commits,
    /// and the day templates have the meal blocks the plan fills.
    #[test]
    fn every_planned_meal_has_its_recipe_and_its_block() {
        for m in MEALS {
            let file = format!("{}/Wikis/Cooking/{}", m.org, meal_recipe_path(m));
            assert!(STUDIO.get_file(&file).is_some(), "{file}: no such recipe");
            let template = STUDIO
                .get_file(format!(
                    "{}/Vault/Projects/Scheduling/templates/weekday.md",
                    m.org
                ))
                .and_then(|f| f.contents_utf8())
                .expect("weekday template");
            assert!(
                template.contains(&format!("id: {}", m.slot)),
                "the weekday template has no {} block",
                m.slot
            );
        }
    }

    // t[verify contacts.secret.out-of-vault]
    // t[verify vault.private.node-only]
    /// The seeded CardDAV account is the shape a real one takes on disk:
    /// a reference to a test-only secret source and no password at all,
    /// in a directory the marker makes node-private.
    #[test]
    fn the_seeded_carddav_account_holds_a_reference_not_a_password() {
        let dir = STUDIO
            .get_dir("acme-audio/Vault/Records/contacts/.sync")
            .expect("the seed declares a CardDAV account");
        assert!(
            dir.get_file("acme-audio/Vault/Records/contacts/.sync/.fts-private")
                .is_some(),
            "the account directory is not marked node-private"
        );
        let accounts: Vec<_> = dir
            .files()
            .filter(|f| f.path().extension().is_some_and(|e| e == "md"))
            .collect();
        assert!(!accounts.is_empty());
        for f in accounts {
            let src = f.contents_utf8().expect("utf-8 account");
            let (fm, _) = src
                .strip_prefix("---\n")
                .and_then(|s| s.split_once("\n---"))
                .expect("frontmatter");
            let v: serde_yaml::Value = serde_yaml::from_str(fm).expect("yaml");
            assert!(
                v.get("password").is_none(),
                "{}: a password in the vault",
                f.path().display()
            );
            assert_eq!(v["password_ref"]["type"].as_str(), Some("test"));
            assert!(
                v["server_url"]
                    .as_str()
                    .is_some_and(|u| u.starts_with("https://")),
                "a seeded account must be https"
            );
        }
    }

    // t[verify process.capture.people]
    /// Everyone a seeded capture names by `@handle` who is a member has
    /// a profile — a committed contact in that org's vault — and every
    /// contact is in one context at least. One contact is outside the
    /// org, and a capture names them too.
    #[test]
    fn every_person_a_capture_names_has_a_profile() {
        let mut contacts: Vec<(&str, String)> = Vec::new();
        for (org, _) in ORGS {
            let Some(dir) = STUDIO.get_dir(format!("{org}/Vault/Records/contacts")) else {
                continue;
            };
            for f in dir.files() {
                let src = f.contents_utf8().expect("utf-8 contact");
                let (fm, _) = src
                    .strip_prefix("---\n")
                    .and_then(|s| s.split_once("\n---"))
                    .expect("frontmatter");
                let v: serde_yaml::Value = serde_yaml::from_str(fm).expect("yaml");
                let first = v["given_name"].as_str().expect("given_name").to_lowercase();
                let groups = v["groups"].as_sequence().map_or(0, Vec::len);
                assert!(groups >= 1, "{first}: a person is in one context at least");
                contacts.push((org, first));
            }
        }
        let member = |org: &str, h: &str| {
            CAST.iter()
                .any(|m| m.org == org && m.name.to_lowercase() == h)
        };
        let contact = |org: &str, h: &str| contacts.iter().any(|(o, f)| *o == org && f == h);
        let mut outsider_named = false;
        for c in INBOX {
            for w in c.body.split_whitespace() {
                let w = w.trim_end_matches(|ch: char| !ch.is_alphanumeric());
                let Some(h) = w.strip_prefix('@') else {
                    continue;
                };
                assert!(
                    !member(c.org, h) || contact(c.org, h),
                    "@{h} is a member with no profile"
                );
                outsider_named |= contact(c.org, h) && !member(c.org, h);
            }
        }
        assert!(
            outsider_named,
            "a capture should name someone outside the org"
        );
    }

    #[test]
    fn every_capture_belongs_to_a_seeded_org_once() {
        let mut ids = std::collections::HashSet::new();
        for c in INBOX {
            assert!(
                ORGS.iter().any(|(s, _)| *s == c.org),
                "{}: no such org",
                c.org
            );
            assert!(ids.insert(c.id), "{}: two captures share this id", c.id);
            assert!(
                [
                    inbox::InboxItem::STATUS_OPEN,
                    inbox::InboxItem::STATUS_SUGGESTED,
                    inbox::InboxItem::STATUS_PROCESSED,
                ]
                .contains(&c.status),
                "{}: a seeded capture is open, suggested or processed, not `{}`",
                c.id,
                c.status
            );
            assert!(
                c.processed_into.is_none() || c.status == inbox::InboxItem::STATUS_PROCESSED,
                "{}: only a processed capture names what it became",
                c.id
            );
        }
    }

    // t[verify process.capture.verbatim]
    /// The seed shows a capture in each state processing leaves it in —
    /// waiting (open), suggested, snoozed, and processed into a task,
    /// into a note, and into nothing — and every processed one walks
    /// back: its task is a declared task at the path the seeder files it
    /// under, its note is committed and quotes the capture verbatim.
    #[test]
    fn the_seed_has_a_capture_in_every_state_and_each_walks_back() {
        let acme: Vec<DeclaredCapture> = inbox_of("acme-audio").copied().collect();
        let processed = |c: &&DeclaredCapture| c.status == inbox::InboxItem::STATUS_PROCESSED;
        assert!(
            acme.iter()
                .any(|c| c.status == "open" && c.resurface_in_days.is_none())
        );
        assert!(acme.iter().any(|c| c.status == "suggested"));
        assert!(acme.iter().any(|c| c.resurface_in_days.is_some()));
        assert!(
            acme.iter()
                .filter(processed)
                .any(|c| c.processed_into.is_none()),
            "one capture already done"
        );
        let tasks: Vec<&str> = DECLARED
            .iter()
            .filter(|p| p.org == "acme-audio")
            .flat_map(|p| p.tasks.iter().map(|(t, _, _)| *t))
            .collect();
        let (mut to_task, mut to_note) = (false, false);
        for c in acme.iter().filter(processed) {
            let Some(into) = c.processed_into else {
                continue;
            };
            if into.starts_with("Task/") {
                let title = tasks
                    .iter()
                    .find(|t| task::write::default_task_path(t, None) == into)
                    .unwrap_or_else(|| panic!("{into}: no declared task is filed there"));
                assert!(c.body.starts_with(title), "{}: became `{title}`", c.body);
                to_task = true;
            } else {
                let note = STUDIO
                    .get_file(format!("acme-audio/Vault/{into}"))
                    .and_then(include_dir::File::contents_utf8)
                    .unwrap_or_else(|| panic!("{into}: the note is not committed"));
                let quoted: String = c.body.lines().map(|l| format!("> {l}\n")).collect();
                assert!(
                    note.contains(&format!("> [!quote] Original capture\n{quoted}")),
                    "{into} quotes its capture verbatim"
                );
                to_note = true;
            }
        }
        assert!(
            to_task && to_note,
            "a capture became a task, and one a note"
        );
    }

    /// The seed declares an org kind the property tools can show and
    /// hold writes to: `Schemas/session.md` parses, the seeded session
    /// note satisfies it, and `Sessions.base` is there to list it.
    #[test]
    fn the_seed_declares_a_session_schema_its_notes_satisfy() {
        use vault::property_schema::{PropertySchemaRegistry, schema_from_frontmatter};
        let fm = |path: &str| -> serde_json::Map<String, serde_json::Value> {
            let text = STUDIO
                .get_file(format!("acme-audio/Vault/{path}"))
                .and_then(include_dir::File::contents_utf8)
                .unwrap_or_else(|| panic!("{path} is not committed"));
            vault::obsidian::obsidian_parse::parse_frontmatter(text)
                .0
                .into_iter()
                .map(|e| (e.key, e.value))
                .collect()
        };
        let schema = schema_from_frontmatter(&fm("Schemas/session.md")).expect("schema parses");
        assert_eq!(schema.kind, "session");
        let mut registry = PropertySchemaRegistry::with_builtins();
        registry.insert(schema);
        let note = fm("Records/sessions/tracking-day-washed.md");
        assert_eq!(note["kind"], "session");
        for (key, value) in note {
            registry
                .validate(Some("session"), &key, value)
                .unwrap_or_else(|v| panic!("the seeded session breaks its schema: {v}"));
        }
        assert!(
            registry
                .validate(Some("session"), "room", serde_json::json!("c-room"))
                .is_err()
        );
        assert!(STUDIO.get_file("acme-audio/Vault/Sessions.base").is_some());
        // The gig recipe's playback sessions are held to the same kind.
        for path in [
            "Records/sessions/night-drive-playback.md",
            "Records/sessions/paper-moon-playback.md",
        ] {
            for (key, value) in fm(path) {
                registry
                    .validate(Some("session"), &key, value)
                    .unwrap_or_else(|v| panic!("{path} breaks the session schema: {v}"));
            }
        }
    }

    /// Every committed note of `slug`'s vault as `(vault path, text)`.
    fn committed_vault_notes(slug: &str) -> Vec<(String, String)> {
        fn walk(dir: &'static Dir<'static>, prefix: &str, out: &mut Vec<(String, String)>) {
            for f in dir.files() {
                let path = f.path().to_string_lossy().replace('\\', "/");
                if let (Some(rel), Some(text)) = (path.strip_prefix(prefix), f.contents_utf8()) {
                    out.push((rel.to_owned(), text.to_owned()));
                }
            }
            for d in dir.dirs() {
                walk(d, prefix, out);
            }
        }
        let root = format!("{slug}/Vault");
        let dir = STUDIO
            .get_dir(&root)
            .unwrap_or_else(|| panic!("{root} is not committed"));
        let mut out = Vec::new();
        walk(dir, &format!("{root}/"), &mut out);
        out.sort();
        out
    }

    /// The gig recipe (`docs/recipes/gigs.md`) is data, so the seed is
    /// its contract: the release show's three `.base` files, run over
    /// the committed vault exactly as `query_base` runs them, give the
    /// set in position order with effective keys (`+4`, an absolute
    /// key, none), flag the one use whose session is missing, and list
    /// the rehearsal and the show that link to the gig.
    #[test]
    fn the_gig_recipe_seed_runs_as_bases() {
        const GIG: &str = "Gigs/example-album-release";
        let notes = committed_vault_notes("acme-audio");
        let rows = vault::obsidian::rows_from_markdown(
            notes.iter().map(|(p, t)| (p.as_str(), t.as_str())),
        );
        fn note_text<'n>(notes: &'n [(String, String)], path: &str) -> &'n str {
            notes
                .iter()
                .find(|(p, _)| p == path)
                .map(|(_, t)| t.as_str())
                .unwrap_or_else(|| panic!("{path} is not committed"))
        }
        let text = |path: &str| note_text(&notes, path);
        let run = |base: &str, view: &str| -> Vec<(String, serde_json::Value)> {
            let path = format!("{GIG}/{base}.base");
            let parsed = vault::bases::parse(text(&path))
                .unwrap_or_else(|e| panic!("{path} doesn't parse: {e}"));
            let tables = vault::obsidian::run_base(&parsed, &rows, Some(view))
                .unwrap_or_else(|e| panic!("{path}: {e}"));
            tables[0]
                .groups
                .iter()
                .flat_map(|g| &g.rows)
                .map(|r| {
                    let mut all = serde_json::Map::new();
                    for (k, v) in &r.values {
                        all.insert(k.clone(), v.clone());
                    }
                    for (k, v) in &r.formulas {
                        all.insert(format!("formula.{k}"), v.clone());
                    }
                    (r.name.clone(), serde_json::Value::Object(all))
                })
                .collect()
        };

        // The gig note links its project.
        let gig = text(&format!("{GIG}.md"));
        assert!(gig.contains("type: gig"), "{gig}");
        assert!(gig.contains("project: project:example-album"), "{gig}");
        assert!(
            declared_of("acme-audio").any(|p| project_rel(p.dir) == "example-album"),
            "the gig's project is one the seed declares"
        );

        // Setlist: position order, not file-name order; effective keys.
        let set = run("Setlist", "Setlist");
        let got: Vec<(&str, &serde_json::Value, &serde_json::Value)> = set
            .iter()
            .map(|(name, v)| (name.as_str(), &v["position"], &v["formula.effective_key"]))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "Night Drive (release show)",
                    &serde_json::json!(1),
                    &serde_json::json!("B")
                ),
                (
                    "Paper Moon (release show)",
                    &serde_json::json!(2),
                    &serde_json::json!("E")
                ),
                (
                    "Last Call (release show)",
                    &serde_json::json!(3),
                    &serde_json::json!("A")
                ),
            ],
            "{set:?}"
        );
        assert_eq!(set[0].1["formula.song_key"], "G", "{set:?}");
        assert_eq!(set[1].1["formula.song_key"], "D", "{set:?}");

        // Readiness: Last Call's session note doesn't exist.
        let ready = run("Readiness", "Readiness");
        let flags: Vec<(&str, &serde_json::Value, &serde_json::Value)> = ready
            .iter()
            .map(|(name, v)| (name.as_str(), &v["formula.session"], &v["formula.ready"]))
            .collect();
        let (t, f) = (serde_json::json!(true), serde_json::json!(false));
        assert_eq!(
            flags,
            [
                ("Night Drive (release show)", &t, &t),
                ("Paper Moon (release show)", &t, &t),
                ("Last Call (release show)", &f, &f),
            ],
            "{ready:?}"
        );
        for (name, v) in &ready {
            for check in ["formula.chart", "formula.reference", "formula.patch_set"] {
                assert_eq!(v[check], true, "{name}: {check}");
            }
        }
        let missing: Vec<String> = run("Readiness", "Not ready")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(missing, ["Last Call (release show)"]);

        // Calendar: the rehearsal, then the show, both linked to the gig.
        let cal = run("Calendar", "Calendar");
        let got: Vec<(&str, &serde_json::Value)> = cal
            .iter()
            .map(|(name, v)| (name.as_str(), &v["formula.role"]))
            .collect();
        assert_eq!(
            got,
            [
                (
                    "example-album-release-rehearsal",
                    &serde_json::json!("rehearsal")
                ),
                ("example-album-release-show", &serde_json::json!("show")),
            ],
            "{cal:?}"
        );
        // …and each is an event the scheduler reads, linking the gig.
        #[cfg(feature = "plugin-scheduling")]
        for (name, _) in &cal {
            let path = format!("Records/events/{name}.md");
            let raw = text(&path);
            let yaml = raw
                .strip_prefix("---\n")
                .and_then(|r| r.split_once("\n---\n"))
                .map(|(fm, _)| fm)
                .unwrap_or_else(|| panic!("{path} has no frontmatter"));
            let event = scheduling::parse::parse_cal_event(&path, yaml)
                .unwrap_or_else(|e| panic!("{path}: {e:?}"));
            assert_eq!(&event.id, name);
            assert!(event.links_to(GIG), "{path}: {:?}", event.links);
        }
    }

    // t[verify process.due]
    /// What the plant leaves in Alice's inbox, read back through the
    /// inbox backend the server mounts: captures due for processing
    /// (open ones and one suggestion), one put off that is NOT due, and
    /// captures whose quick-add syntax a maintenance hour can find once
    /// they are tasks. A replant keeps what is there.
    #[test]
    fn the_planted_inbox_has_captures_to_process() {
        use inbox::Inbox as _;
        let tmp = tempfile::tempdir().expect("tempdir");
        let mut planted = Planted::default();
        plant_inbox(tmp.path(), "acme-audio", &mut planted).expect("plant");
        assert_eq!(planted.written, inbox_of("acme-audio").count());

        let backend = inbox::VaultInbox::new(tmp.path()).expect("inbox backend");
        let items = backend.list_inbox().expect("list");
        assert_eq!(items.len(), planted.written, "every capture parses back");

        let today = chrono::Utc::now().date_naive().to_string();
        // The deck's rule: open and resurfaced, or a suggestion awaiting
        // a yes or no.
        let due = |i: &inbox::InboxItem| {
            i.status == inbox::InboxItem::STATUS_SUGGESTED || i.in_review_queue(&today)
        };
        let suggested: Vec<_> = items
            .iter()
            .filter(|i| i.status == inbox::InboxItem::STATUS_SUGGESTED)
            .collect();
        assert_eq!(suggested.len(), 1, "one agent suggestion");
        assert!(due(suggested[0]), "a suggestion is due for a decision");
        assert_ne!(
            suggested[0].source, "ui",
            "a suggestion came from elsewhere"
        );

        let snoozed: Vec<_> = items
            .iter()
            .filter(|i| i.is_open() && !i.in_review_queue(&today))
            .collect();
        assert_eq!(snoozed.len(), 1, "one capture put off into the future");
        assert!(snoozed[0].resurface_on.as_deref() > Some(today.as_str()));

        let due_now: Vec<_> = items.iter().filter(|i| due(i)).collect();
        assert!(due_now.len() >= 4, "something to process: {due_now:?}");

        // Processing turns a capture into a task by parsing it the way
        // quick-add does — so these must parse into what a maintenance
        // hour brings forward.
        let errand = due_now
            .iter()
            .find(|i| i.body.contains("@errands"))
            .expect("a capture with @errands");
        let task = task::capture(&errand.body);
        assert_eq!(task.contexts.0, vec!["@errands".to_owned()]);
        let chore = due_now
            .iter()
            .find(|i| i.body.contains("#chore"))
            .expect("a capture with #chore");
        assert!(task::capture(&chore.body).tags.iter().any(|t| t == "chore"));
        #[cfg(feature = "plugin-scheduling")]
        {
            let ctx = task::RelevanceContext {
                focus: scheduling_proto::BlockCategory::Maintenance
                    .surfaces()
                    .iter()
                    .map(|s| (*s).to_owned())
                    .collect(),
                ..Default::default()
            };
            assert!(task::relevance::matches_focus(&task, &ctx));
            assert!(task::relevance::matches_focus(
                &task::capture(&chore.body),
                &ctx
            ));
        }

        // Idempotent by omission: a processed capture stays processed.
        let mut processed = (*errand).clone();
        processed.status = inbox::InboxItem::STATUS_PROCESSED.to_owned();
        backend.upsert_inbox_item(&processed).expect("process");
        let mut again = Planted::default();
        plant_inbox(tmp.path(), "acme-audio", &mut again).expect("replant");
        assert_eq!(again.written, 0, "a replant writes nothing new");
        assert_eq!(
            backend.get_inbox_item(&processed.id).expect("get").status,
            inbox::InboxItem::STATUS_PROCESSED,
            "a replant does not reopen what was processed"
        );
    }

    /// Every seeded description belongs to a declared task.
    #[test]
    fn every_task_description_is_for_a_declared_task() {
        for (title, details) in TASK_DETAILS {
            let _ = org_of_task(title);
            assert!(
                !details.trim().is_empty(),
                "`{title}`: an empty description"
            );
            assert!(
                !details.starts_with("---"),
                "`{title}`: a description is body only, never frontmatter"
            );
        }
    }

    /// The seed shows every kind of creator (`attribution.seed`): a
    /// person, a bot and none at all, on captures and on tasks — and
    /// each one names a declared record and an account of that
    /// record's own org, so the planted world can say "from Atlas".
    // t[verify attribution.seed]
    #[test]
    fn the_seed_has_mixed_creators_from_its_own_org() {
        let in_org = |org: &str, email: &str| {
            cast_of(org).iter().any(|m| m.email == email) || bots_of(org).any(|b| b.email == email)
        };
        for b in BOTS {
            assert!(
                b.email.ends_with(&format!("@{}", crate::bots::BOT_DOMAIN)),
                "{} is not a bot address",
                b.email
            );
        }
        for (id, email) in CAPTURE_CREATORS {
            let c = INBOX
                .iter()
                .find(|c| c.id == *id)
                .unwrap_or_else(|| panic!("`{id}` is not a declared capture"));
            assert!(in_org(c.org, email), "`{email}` is not in {}", c.org);
        }
        for (title, email) in TASK_CREATORS {
            let project = DECLARED
                .iter()
                .find(|p| p.tasks.iter().any(|(t, _, _)| t == title))
                .unwrap_or_else(|| panic!("`{title}` is not a declared task"));
            assert!(
                in_org(project.org, email),
                "`{email}` made `{title}` but is not in {}",
                project.org
            );
        }
        let is_bot = |e: &str| BOTS.iter().any(|b| b.email == e);
        for (what, emails) in [
            (
                "captures",
                CAPTURE_CREATORS.iter().map(|(_, e)| *e).collect::<Vec<_>>(),
            ),
            ("tasks", TASK_CREATORS.iter().map(|(_, e)| *e).collect()),
        ] {
            assert!(emails.iter().any(|e| is_bot(e)), "no bot made any {what}");
            assert!(
                emails.iter().any(|e| !is_bot(e)),
                "no person made any {what}"
            );
        }
        assert!(
            INBOX
                .iter()
                .any(|c| !CAPTURE_CREATORS.iter().any(|(id, _)| *id == c.id)),
            "one capture stays unattributed, like those made before attribution"
        );

        // A personal bot and a team bot (`attribution.on-behalf-of`):
        // the person a personal bot works for is in its org.
        for b in BOTS {
            if let Some(person) = b.acts_for {
                assert!(
                    cast_of(b.org).iter().any(|m| m.email == person),
                    "{} acts for `{person}`, who is not in {}",
                    b.name,
                    b.org
                );
            }
        }
        assert!(BOTS.iter().any(|b| b.acts_for.is_some()), "no personal bot");
        assert!(BOTS.iter().any(|b| b.acts_for.is_none()), "no team bot");
        let made_by_team_bot = TASK_CREATORS
            .iter()
            .any(|(_, e)| BOTS.iter().any(|b| b.email == *e && b.acts_for.is_none()));
        assert!(made_by_team_bot, "the team bot made nothing");

        // An employee who is not the first one, with work a personal
        // bot made for someone else and that someone handed on.
        let employees: Vec<_> = CAST
            .iter()
            .filter(|m| matches!(m.holds, Holds::Employee))
            .collect();
        assert!(employees.len() >= 2, "a second employee");
        let handed_on = TASK_OWNERS.iter().any(|(title, owner)| {
            employees[1..].iter().any(|e| e.email == *owner)
                && task_creator(title).is_some_and(|by| {
                    BOTS.iter()
                        .any(|b| b.email == by && b.acts_for.is_some_and(|p| p != *owner))
                })
        });
        assert!(
            handed_on,
            "no assistant-made task assigned to the new employee"
        );
    }

    /// Every owned task is a declared task, owned by a cast member of
    /// the org the task lives in — an owner from another company would
    /// plant an assignee the board can't name.
    #[test]
    fn every_task_owner_is_a_member_of_the_tasks_org() {
        // (The seeded history is held to the same rule below.)
        for (title, email) in TASK_OWNERS {
            let project = DECLARED
                .iter()
                .find(|p| p.tasks.iter().any(|(t, _, _)| t == title))
                .unwrap_or_else(|| panic!("`{title}` is not a declared task"));
            assert!(
                cast_of(project.org).iter().any(|m| m.email == *email),
                "`{email}` owns `{title}` but is not in {}",
                project.org
            );
        }
    }

    /// The seeded history names declared tasks in its own org, by cast
    /// members of that org, under distinct fixed ids — a line about a
    /// task the board doesn't have would link nowhere, and a byline
    /// from another company would name a stranger.
    // t[verify activity.seed]
    #[test]
    fn the_seeded_history_is_about_the_seed() {
        let mut ids = std::collections::HashSet::new();
        let lines = SEED_ACTIVITY
            .iter()
            .map(|a| (a.org, a.id, a.task, a.actor))
            .chain(
                SEED_COMMENTS
                    .iter()
                    .map(|c| (c.org, c.id, c.task, c.author)),
            );
        for (org, id, task, who) in lines {
            assert!(id.parse::<uuid::Uuid>().is_ok(), "`{id}` is not a uuid");
            assert!(ids.insert(id), "`{id}` is used twice");
            assert!(
                declared_of(org).any(|p| p.tasks.iter().any(|(t, _, _)| *t == task)),
                "`{task}` is not a declared task of {org}"
            );
            assert!(
                cast_of(org).iter().any(|m| m.email == who),
                "`{who}` is not in {org}"
            );
        }
        // Alice opens the app on something: her org has history and a
        // comment thread from someone else.
        assert!(SEED_ACTIVITY.iter().any(|a| a.org == "acme-audio"));
        assert!(
            SEED_COMMENTS
                .iter()
                .any(|c| c.org == "acme-audio" && c.author != "alice@acme.test")
        );
    }

    /// Every committed mail account is one a maildir backend will open.
    ///
    /// `email_maildir::is_maildir` wants `cur` and `new` to be present,
    /// and git stores no empty directory — so an account committed with
    /// its messages only in `cur` reads as "not a mailbox", the account
    /// vanishes from `accounts()`, and the mail chapter fails a long
    /// way from the cause. `complete_mail_accounts` makes `tmp` at
    /// plant time; the other two have to be carried, which means each
    /// needs at least one committed message.
    #[test]
    fn every_committed_mail_account_is_a_maildir() {
        for (slug, _) in ORGS {
            let Some(mail) = STUDIO.get_dir(format!("{slug}/Mail")) else {
                continue;
            };
            for account in mail.dirs() {
                let name = account.path().display();
                for sub in ["cur", "new"] {
                    let dir = account.path().join(sub);
                    assert!(
                        STUDIO.get_dir(&dir).is_some_and(|d| d.files().count() > 0),
                        "{name}: `{sub}/` is missing or empty, so git carries no such \
                         directory and the account will not open as a maildir"
                    );
                }
            }
        }
    }

    /// Every committed message carries a real Message-ID.
    ///
    /// The link store is keyed on it. A message without one gets a
    /// synthesised id derived from its bytes (`parse::synth_message_id`)
    /// — stable enough to look fine in a listing, and different on any
    /// mailbox that re-encoded the message, so a link made against it
    /// would resolve here and nowhere else.
    #[test]
    fn every_committed_message_has_a_message_id() {
        for (slug, _) in ORGS {
            let Some(mail) = STUDIO.get_dir(format!("{slug}/Mail")) else {
                continue;
            };
            for account in mail.dirs() {
                for folder in account.dirs() {
                    for msg in folder.files() {
                        let body = String::from_utf8_lossy(msg.contents());
                        assert!(
                            body.lines().any(|l| {
                                l.to_ascii_lowercase().starts_with("message-id:") && l.contains('@')
                            }),
                            "{}: no Message-ID header",
                            msg.path().display()
                        );
                    }
                }
            }
        }
    }

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

    /// A declared parent exists, the child's directory is inside it,
    /// and the child carries a committed `project.md` — which is the
    /// whole of what makes it a project rather than a folder
    /// (`project.identity.declaration`).
    ///
    /// t[verify project.nesting.uniform] — the sub-project is declared
    /// by the same struct, with the same fields, as its parent. There
    /// is no `DeclaredSubProject`, because there is no sub-project
    /// entity.
    #[test]
    fn every_declared_parent_exists_and_holds_its_child() {
        for d in DECLARED {
            let Some(parent) = d.parent else { continue };
            let p = DECLARED
                .iter()
                .find(|x| x.dir == parent && x.org == d.org)
                .unwrap_or_else(|| panic!("{}: parent `{parent}` is not declared", d.title));
            assert!(
                d.dir.starts_with(&format!("{}/", p.dir)),
                "{}: a subproject's files live inside its parent's directory, \
                 and `{}` is not inside `{}`",
                d.title,
                d.dir,
                p.dir
            );
            assert!(
                STUDIO
                    .get_file(format!("{}/Projects/{}/project.md", d.org, d.dir))
                    .is_some(),
                "{}: a subproject needs a committed `project.md`; without one \
                 the directory is unclassified content, not a project",
                d.title
            );
        }
    }

    /// t[verify project.part.listing] — *"A parent's part list is a
    /// roster of its pieces, not a list of the ones that are not
    /// projects."*
    ///
    /// The album lists three tracks and one of them is a project. Both
    /// are true at once, and the seed is where that is demonstrable: a
    /// world where promoting a song removed it from its album's running
    /// order would look correct in every unit test and wrong to anybody
    /// reading the track listing.
    #[test]
    fn a_promoted_part_stays_in_its_parents_roster() {
        let mut checked = 0;
        for d in DECLARED {
            let Some(parent_dir) = d.parent else { continue };
            let parent = DECLARED
                .iter()
                .find(|x| x.dir == parent_dir && x.org == d.org)
                .expect("declared parent");
            // The child's directory name under its parent is the part's
            // name, which is how the seed's promotion and the album's
            // roster refer to the same piece.
            let piece = d.dir.rsplit('/').next().unwrap_or(d.dir);
            assert!(
                parent.parts.contains(&piece),
                "{}: `{piece}` is a project, and it must STILL be one of \
                 {}'s parts — promotion adds a page, it does not edit the roster",
                d.title,
                parent.title
            );
            checked += 1;
        }
        assert!(
            checked > 0,
            "the seed carries no subproject, so nothing here is being checked — \
             see `DeclaredProject::parent` on why the example needs one"
        );
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
    #[cfg(feature = "plugin-wiki")]
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
            .get_file("acme-audio/Wikis/Audio Production/schema.md")
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
            // The tier is the committed directory: `Resources/**` plants
            // into `resources/`, `Vault/**` into the vault. An asset
            // declared on the wrong one is an asset a demo user reaches
            // through the wrong lane, which is exactly the confusion ADR
            // 0004 exists to end.
            let area = match a.tier {
                AssetTier::Resources => "Resources",
                AssetTier::Assets => "AssetGroups",
            };
            // An empty `body` is the vault case: a chart *is* one
            // document, source and all.
            for file in [a.manifest, a.body].into_iter().filter(|f| !f.is_empty()) {
                let path = format!("{}/{area}/{}/{}", a.org, a.library, file);
                assert!(
                    STUDIO.get_file(&path).is_some(),
                    "{}: `{path}` is not committed — {}",
                    a.slug,
                    a.demonstrates
                );
            }
        }
    }

    /// A vault asset declares the tier it is on, in its own frontmatter,
    /// and carries its document with it.
    ///
    /// This is the seed half of ADR 0004's recognition rule. A chart
    /// that still said `type: resource` would parse — the manifest
    /// grammar is shared — and would be filed on the shelf while
    /// claiming to be an import, which is the one state the tier
    /// distinction cannot survive. So the committed tree is checked
    /// against the constants the server reads, not against a habit.
    #[test]
    fn every_vault_asset_declares_the_tier_and_holds_its_own_source() {
        use resources_proto::assets;
        for a in DECLARED_ASSETS
            .iter()
            .filter(|a| a.tier == AssetTier::Assets)
        {
            let path = format!("{}/AssetGroups/{}/{}", a.org, a.library, a.manifest);
            let text = STUDIO
                .get_file(&path)
                .and_then(|f| f.contents_utf8())
                .unwrap_or_else(|| panic!("{}: `{path}` is not committed", a.slug));
            assert!(
                text.contains(&format!("{}: {}", assets::TYPE_KEY, assets::TYPE_ASSET)),
                "{}: `{path}` does not declare `type: asset`",
                a.slug
            );
            assert!(
                !text.contains(resources::chart::LEGACY_KIND_KEY),
                "{}: `{path}` still carries ADR 0003's `resource_kind`",
                a.slug
            );
            assert!(
                a.body.is_empty(),
                "{}: a vault asset is one document — its source belongs in its own body",
                a.slug
            );
            // A chart carries its source in its own body; a song is
            // prose and carries none. Which kind it is comes from the
            // shelf it is on, because that is the organising
            // convention the tier is made of.
            if a.library == assets::CHARTS_KIND {
                assert!(
                    assets::extract_fenced(text, assets::CHART_FENCE).is_some(),
                    "{}: `{path}` has no ```{} fence, so it has no chart in it",
                    a.slug,
                    assets::CHART_FENCE
                );
            }
        }
    }

    /// Every vault asset is on the shelf ADR 0004 names, spelled the
    /// way the server spells it. A drift here plants charts somewhere
    /// `list_charts` does not walk, and the seed would silently hold
    /// nothing.
    #[test]
    fn every_vault_asset_sits_on_the_assets_shelf() {
        use org_proto::Shelf as _;
        for a in DECLARED_ASSETS
            .iter()
            .filter(|a| a.tier == AssetTier::Assets)
        {
            // The group name is the shelf name, and the shelf name is
            // the subscription slug: `acme.test/songs` is the song
            // library another org takes. A group whose directory is not
            // its own slug could be planted and never subscribed to.
            assert_eq!(
                a.library,
                org_proto::wiki_slug(a.library),
                "{}: `{}` is not a slug, so no reference could name it",
                a.slug,
                a.library
            );
            let shelf = org_proto::AssetShelf::new(a.library.to_owned(), std::path::PathBuf::new());
            assert!(
                shelf.is_subscribable(),
                "{}: the point of the tier is that another org can take it",
                a.slug
            );
            let expected = if a.library == resources_proto::assets::CHARTS_KIND {
                resources_proto::assets::chart_path(a.slug)
            } else {
                resources_proto::assets::song_path(a.slug)
            };
            assert_eq!(
                a.manifest, expected,
                "{}: the committed path is not where its lane writes",
                a.slug
            );
        }
    }

    /// The library directory is not decoration: it is the subscription
    /// slug a cross-org reader names, and `node_homes::library_of` is
    /// what maps a node kind onto it. A drift here is a cross-org
    /// reference that silently stops resolving, so the two are pinned
    /// against each other rather than kept in step by hand.
    #[cfg(feature = "plugin-wiki")]
    #[test]
    fn every_asset_library_is_a_home_some_node_kind_resolves_through() {
        use links::NodeKind;
        // **Every** declared asset, on either tier, and that is the
        // shape of ADR 0004 landing. An intermediate draft filed
        // charts and songs at `<vault>/Assets/<Kind>/`, and this test
        // had to exempt them: `library_of` maps a node kind onto the
        // subdirectory a cross-org subscription names, and a shelf
        // inside a vault could not be subscribed to, so pinning it
        // against that map would have asserted a reach that did not
        // exist. An asset group's name *is* that subdirectory now —
        // `library_of(Chart) == "charts" == <org>/assets/charts` — so
        // the exemption is gone and one map answers for both tiers.
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
                "{}: `{}/` is not any node kind's home",
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
    /// wrong thing. A setlist full of references to nothing still
    /// plants, still lists, and still opens — as a list of unresolved
    /// rows, which is exactly the state ADR 0003 says is *legible* and
    /// therefore exactly the state a demo cannot be made of.
    #[test]
    fn every_chordsheet_library_imports_whole_and_names_every_song_once() {
        for lib in CHORDSHEET_LIBRARIES {
            assert!(
                ORGS.iter().any(|(slug, _)| *slug == lib.org),
                "{}: org `{}` is not one the example plants",
                lib.songlist,
                lib.org
            );
            let songs = chordsheet_songs(lib)
                .unwrap_or_else(|e| panic!("{}: the backup does not import: {e}", lib.songlist));
            let manifest: Vec<serde_json::Value> = serde_json::from_slice(
                STUDIO
                    .get_file(format!("{}/{}/manifest.json", lib.org, lib.backup))
                    .expect("manifest")
                    .contents(),
            )
            .expect("manifest parses");
            assert_eq!(
                songs.len(),
                manifest.len(),
                "{}: an entry of the backup was dropped on import",
                lib.songlist
            );
            let unique: std::collections::HashSet<&str> =
                songs.iter().map(|s| s.song_slug.as_str()).collect();
            assert_eq!(
                unique.len(),
                songs.len(),
                "{}: two entries imported as one song — a title shared across \
                 artists must still be two songs",
                lib.songlist
            );
            assert!(
                songs
                    .iter()
                    .all(|s| s.chart.is_default && !s.chart.source.trim().is_empty()),
                "{}: every song's only chart is its default, with its source",
                lib.songlist
            );
        }
    }

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
            // Not "is this one of four words" any more — there is no
            // list to be one of (ADR 0004). What is still checkable, and
            // is the thing that actually bites, is that a declaration is
            // written in the normal form the store keeps: declare
            // `Setlist` here and the seed would plant `setlist`, so the
            // replant check that matches on the declared spelling would
            // never recognise its own row and would plant a duplicate on
            // every boot.
            let planted_as = collection_proto::CollectionKind::new(c.kind);
            assert_eq!(
                planted_as.as_str(),
                c.kind,
                "{}: declare the kind as the store will hold it (lowercase, trimmed)",
                c.title
            );
            assert!(
                !planted_as.is_empty(),
                "{}: a collection with no kind",
                c.title
            );
            assert!(!c.items.is_empty(), "{}: an empty collection", c.title);
            for (kind, id) in c.items {
                let kind = NodeKind::parse(kind)
                    .unwrap_or_else(|| panic!("{}: `{kind}` is not a node kind", c.title));
                // A song is a folder with a manifest; anything else is a
                // `DECLARED_ASSETS` row, which the tests above prove is
                // committed on whichever tier it declares.
                //
                // The lookup is by slug rather than by
                // `node_homes::library_of`, because what a collection
                // actually asserts is that the reference resolves to
                // something planted, and the slug is the identity a
                // `chart:<slug>` reference carries. Which tier that
                // something is on is the previous test's question.
                let planted = match kind {
                    // A song with media is a folder with a manifest; a song
                    // from a chordsheet library is one the import plants.
                    NodeKind::Song => {
                        STUDIO
                            .get_file(format!("{}/Resources/songs/{id}/manifest.json", c.org))
                            .is_some()
                            || CHORDSHEET_LIBRARIES
                                .iter()
                                .filter(|l| l.org == c.org)
                                .filter_map(|l| chordsheet_songs(l).ok())
                                .flatten()
                                .any(|s| s.song_slug == *id)
                    }
                    // A collection is named by title, and must be declared
                    // *earlier* in the same org: that is the order the
                    // planter creates them in and resolves titles against.
                    NodeKind::Collection => DECLARED_COLLECTIONS
                        .iter()
                        .take_while(|other| !std::ptr::eq(*other, c))
                        .any(|other| other.org == c.org && other.title == *id),
                    _ => assets_of(c.org).any(|a| a.slug == *id),
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

    // ── Crescendum, the project it must keep out, and the grant ──────
    //
    // `docs/spec/sharing.md`. The seed's half of cross-org sharing: an
    // album FastTrackAudio owns and shares with one person of Rockstars
    // of Tomorrow, beside a project of FastTrackAudio's that nobody is
    // granted, carrying the money a guest must never see.

    const FTA: &str = "fasttrackaudio";
    const ROT: &str = "rockstars-of-tomorrow";

    fn declared_by_dir(org: &str, dir: &str) -> &'static DeclaredProject {
        DECLARED
            .iter()
            .find(|d| d.org == org && d.dir == dir)
            .unwrap_or_else(|| panic!("{org} declares no project at `{dir}`"))
    }

    /// The declared project's tier-relative page, as the server stores it.
    fn page_of(d: &DeclaredProject) -> String {
        format!("{}/{}", project_rel(d.dir), org_proto::PROJECT_PAGE)
    }

    /// A declared project's committed page, parsed the way the server
    /// parses one.
    fn committed_page(d: &DeclaredProject) -> project::ProjectInfo {
        let path = format!("{}/Projects/{}/project.md", d.org, d.dir);
        let raw = studio_file(&path)
            .and_then(|b| std::str::from_utf8(b).ok())
            .unwrap_or_else(|| panic!("{}: no committed page at {path}", d.title));
        project::parse_str(&page_of(d), "project", raw)
            .unwrap_or_else(|e| panic!("{}: the committed page does not parse: {e:?}", d.title))
    }

    /// Every file committed under a directory of the example, by its
    /// path on the org's Projects tier.
    fn committed_under(org: &str, dir: &str) -> Vec<String> {
        fn walk(d: &Dir<'_>, out: &mut Vec<String>) {
            for f in d.files() {
                out.push(f.path().to_string_lossy().into_owned());
            }
            for sub in d.dirs() {
                walk(sub, out);
            }
        }
        let prefix = format!("{org}/Projects/");
        let mut out = Vec::new();
        walk(
            STUDIO
                .get_dir(format!("{prefix}{dir}"))
                .unwrap_or_else(|| panic!("no committed tree at {prefix}{dir}")),
            &mut out,
        );
        out.into_iter()
            .map(|p| project_rel(p.strip_prefix(&prefix).unwrap_or(&p)))
            .collect()
    }

    /// FastTrackAudio's projects commit their ids, so a replant, the
    /// grant and every test name the same projects — Crescendum by
    /// `CRESCENDUM_ID` — and each sub-project's committed `parentId` is
    /// its declared parent's id.
    #[test]
    fn fasttrackaudios_projects_carry_fixed_ids() {
        let declared: Vec<_> = declared_of(FTA).collect();
        assert!(
            declared.len() >= 5,
            "FastTrackAudio declares {}",
            declared.len()
        );
        let mut ids = std::collections::HashSet::new();
        for d in &declared {
            let path = format!("{}/Projects/{}/project.md", d.org, d.dir);
            let raw = std::str::from_utf8(studio_file(&path).expect("committed page")).unwrap();
            assert!(
                raw.lines().any(|l| l.starts_with("id: ")),
                "{}: the page must commit its id, or a move derives another",
                d.title
            );
            let page = committed_page(d);
            assert_eq!(page.title, d.title, "{}: committed title", d.title);
            assert!(ids.insert(page.id), "{}: id {} reused", d.title, page.id);
            let want_parent = d.parent.map(|p| committed_page(declared_by_dir(FTA, p)).id);
            assert_eq!(
                page.parent_id, want_parent,
                "{}: committed parentId",
                d.title
            );
        }
        assert_eq!(
            committed_page(declared_by_dir(FTA, "Crescendum")).id,
            CRESCENDUM_ID,
            "Crescendum is 9880bdc8-307f-4ed6-b6cb-6e05ca733571 everywhere"
        );
    }

    /// Crescendum is an album with two song sub-projects, one of which
    /// has a sub-project of its own a level further down — and it carries
    /// a client and a rate, so a guest projection has billing to strip
    /// (`sharing.grant.exclusions`).
    #[test]
    fn crescendum_is_an_album_of_two_songs_one_nested_deeper_with_billing() {
        let album = declared_by_dir(FTA, "Crescendum");
        assert_eq!(album.form, Some(project::Form::Album));
        assert!(album.parent.is_none());
        let songs: Vec<_> = DECLARED
            .iter()
            .filter(|d| d.org == FTA && d.parent == Some(album.dir))
            .collect();
        assert_eq!(
            songs.len(),
            2,
            "two songs under Crescendum: {:?}",
            songs.iter().map(|s| s.title).collect::<Vec<_>>()
        );
        assert!(songs.iter().all(|s| s.form == Some(project::Form::Song)));
        let nested: Vec<_> = DECLARED
            .iter()
            .filter(|d| d.org == FTA && songs.iter().any(|s| d.parent == Some(s.dir)))
            .collect();
        assert_eq!(nested.len(), 1, "one sub-project a level below a song");
        for d in songs.iter().chain(&nested).chain([&album]) {
            assert!(!d.parts.is_empty(), "{}: parts", d.title);
            assert!(!d.tasks.is_empty(), "{}: tasks", d.title);
        }
        assert!(
            committed_under(FTA, album.dir)
                .iter()
                .any(|p| p.starts_with("crescendum/Notes/") && p.ends_with(".md")),
            "notes under Crescendum's directory"
        );

        let page = committed_page(album);
        let client = page.client_id.expect("Crescendum names a client");
        assert!(
            clients_of(FTA).any(|c| c.id == client),
            "Crescendum's client {client} is a declared client"
        );
        assert!(page.default_rate_cents > 0, "Crescendum carries a rate");
    }

    /// FastTrackAudio's other project exists to be kept out: it is not
    /// under Crescendum, and it holds tasks, a note, a rate, a client
    /// with a committed contact, and the billed time and invoice
    /// `SEED_BILLING` plants.
    #[test]
    fn the_hidden_project_holds_what_a_guest_must_never_see() {
        let hidden = declared_by_dir(FTA, "Northbeam Jingles - Northbeam Records");
        assert!(hidden.parent.is_none(), "beside Crescendum, not under it");
        assert!(!hidden.tasks.is_empty());
        assert!(
            committed_under(FTA, hidden.dir)
                .iter()
                .any(|p| p.contains("/Notes/") && p.ends_with(".md")),
            "a note in its directory"
        );
        let page = committed_page(hidden);
        assert!(page.default_rate_cents > 0, "a rate on the page");
        let client = clients_of(FTA)
            .find(|c| Some(c.id) == page.client_id)
            .expect("its clientId is a declared client");
        let contact = client.contact.expect("the client has a contact");
        let card = studio_file(&format!("{FTA}/Vault/Records/contacts/{contact}.md"))
            .and_then(|b| std::str::from_utf8(b).ok())
            .expect("the contact is committed");
        assert!(
            card.contains(&format!("organization: {}", client.name)),
            "the contact works at {}",
            client.name
        );
        let billed = billing_of(FTA)
            .find(|b| b.project == hidden.title)
            .expect("billed time and an invoice");
        assert_eq!(billed.client, client.name);
        assert!(billed.hours > 0 && billed.hourly_cents > 0);
        assert!(cast_of(FTA).iter().any(|m| m.email == billed.member));
        assert!(
            !SEED_GRANTS
                .iter()
                .any(|g| g.owner_org == FTA && g.root == hidden.dir),
            "nobody is granted the hidden project"
        );
    }

    /// The grant is one live, read + comment grant on Crescendum, to a
    /// PERSON of Rockstars of Tomorrow who holds no account in
    /// FastTrackAudio — and so, since the demo writes membership rows
    /// only where an account exists (`admin adopt-principal`), no
    /// membership row there either. Granted by FastTrackAudio's owner.
    /// No bot is ever a subject (Captain decision 2).
    // t[verify sharing.grant.membership-free]
    #[test]
    fn the_seed_grant_is_a_rockstars_person_with_no_fta_membership() {
        let crescendum: Vec<_> = SEED_GRANTS
            .iter()
            .filter(|g| g.owner_org == FTA && g.root == "Crescendum")
            .collect();
        assert_eq!(crescendum.len(), 1, "one grant on Crescendum");
        for g in SEED_GRANTS {
            assert!(ORGS.iter().any(|(s, _)| *s == g.owner_org));
            assert!(ORGS.iter().any(|(s, _)| *s == g.guest_org));
            assert_ne!(g.owner_org, g.guest_org, "a grant crosses an org line");
            let _ = declared_by_dir(g.owner_org, g.root);
            assert!(
                cast_of(g.guest_org).iter().any(|m| m.email == g.subject),
                "{} is a person of {}",
                g.subject,
                g.guest_org
            );
            assert!(
                !cast_of(g.owner_org).iter().any(|m| m.email == g.subject),
                "{} holds an account in {} — and so would get a membership row there",
                g.subject,
                g.owner_org
            );
            assert!(
                !BOTS.iter().any(|b| b.email == g.subject)
                    && !g
                        .subject
                        .ends_with(&format!("@{}", crate::bots::BOT_DOMAIN)),
                "{} is a bot; guests are people only",
                g.subject
            );
            assert!(
                cast_of(g.owner_org)
                    .iter()
                    .any(|m| m.email == g.granted_by && m.holds == Holds::Owner),
                "{} is not an owner of {}",
                g.granted_by,
                g.owner_org
            );
            assert_eq!(g.capabilities, share_proto::GrantCapabilities::ReadComment);
        }
        assert_eq!(crescendum[0].guest_org, ROT);
    }

    /// The S4 resolver over the seed's Crescendum: the album, both songs
    /// and the nested one, with their parts, tasks and notes — and none
    /// of Northbeam Jingles. Over the committed pages and the declared
    /// boards; `demo_plant.rs` asks the same of the planted disk.
    // t[verify sharing.grant.subtree]
    // t[verify sharing.grant.exclusions]
    #[test]
    fn the_grant_on_crescendum_reaches_its_subtree_and_nothing_else() {
        use crate::sharing::subtree::{Attached, OwnerState, ProjectNode};

        let mut state = OwnerState::default();
        for d in declared_of(FTA) {
            let page = committed_page(d);
            state.projects.push(ProjectNode {
                id: page.id,
                parent_id: page.parent_id,
                title: page.title.clone(),
                page: page_of(d),
                parts: d
                    .parts
                    .iter()
                    .map(|n| {
                        let key = format!("{}#{n}", d.dir);
                        (
                            uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, key.as_bytes()),
                            (*n).to_owned(),
                        )
                    })
                    .collect(),
            });
            for (title, _, _) in d.tasks {
                state.tasks.push(Attached {
                    id: uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, title.as_bytes()),
                    project_id: Some(page.id),
                    title: (*title).to_owned(),
                });
            }
        }
        let scope = crate::sharing::resolve(&state, CRESCENDUM_ID).expect("Crescendum resolves");

        let titles: std::collections::BTreeSet<&str> =
            scope.projects.values().map(String::as_str).collect();
        assert_eq!(
            titles,
            ["Crescendum", "First Light", "Undertow", "Undertow Strings"]
                .into_iter()
                .collect(),
            "the album, both songs and the nested one"
        );
        let hidden = declared_by_dir(FTA, "Northbeam Jingles - Northbeam Records");
        let hidden_id = committed_page(hidden).id;
        assert!(!scope.in_scope(&crate::sharing::ObjectRef::Project(hidden_id)));
        for (title, _, _) in hidden.tasks {
            assert!(
                !scope.tasks.values().any(|t| t == title),
                "`{title}` is Northbeam's and out of scope"
            );
        }
        let subtree_tasks: usize = declared_of(FTA)
            .filter(|d| d.dir == "Crescendum" || d.dir.starts_with("Crescendum/"))
            .map(|d| d.tasks.len())
            .sum();
        assert_eq!(
            scope.tasks.len(),
            subtree_tasks,
            "every subtree task, no other"
        );
        let subtree_parts: usize = declared_of(FTA)
            .filter(|d| d.dir == "Crescendum" || d.dir.starts_with("Crescendum/"))
            .map(|d| d.parts.len())
            .sum();
        assert_eq!(scope.parts.len(), subtree_parts);

        for note in committed_under(FTA, "Crescendum") {
            assert!(
                scope.in_scope(&crate::sharing::ObjectRef::Note(note.clone())),
                "{note} is under Crescendum"
            );
        }
        for note in committed_under(FTA, hidden.dir) {
            assert!(
                !scope.in_scope(&crate::sharing::ObjectRef::Note(note.clone())),
                "{note} is Northbeam's"
            );
        }
    }
}
