//! [`DataRoot`] + [`OrgRoot`] — the layout resolver.
//!
//! ```text
//! <data_root>/                        # `DataRoot`
//! ├── server-key.ed25519              # cross-org blob signing keypair
//! └── orgs/
//!     ├── codywright/                 # `OrgRoot` (one per org)
//!     │   ├── org.toml                # `OrgManifest`
//!     │   ├── auth.sqlite
//!     │   ├── identity.sqlite         # only when `is_home = true`
//!     │   ├── timer.sqlite
//!     │   ├── finance.sqlite
//!     │   ├── vault/                  # personal: Journal/, Projects/, Operations/, …
//!     │   ├── wiki/                   # knowledge + LLM scratch
//!     │   │   ├── Knowledge/          # curated wiki (schema/log/concepts/Cookbook/…)
//!     │   │   └── LLM/                # loose LLM-owned space
//!     │   │       ├── Memories/
//!     │   │       └── Journals/
//!     │   └── attachments/
//!     ├── fasttrackstudios/
//!     └── ...
//! ```
//!
//! `vault/`, `wiki/`, `assets/` and `projects/` are ADR 0004's four
//! roots, and each named directory under them is a
//! [`crate::Shelf`] — registered for sync, the link graph and per-file
//! CRDT by one loop, and (except the vault) publishable for another
//! organisation to subscribe to. `resources/` is not a fifth root: it
//! is an asset group that happens to be immutable and leaf-only.
//!
//! All four have moved. `projects/` was the last of them, and it is
//! the one where "a shelf" and "the thing a person means" come apart
//! most: a project is a *note* in the vault and a *tree* of bytes on
//! this tier, and only the tree is here.
//! [`OrgRoot::project_shelves`] is the argument for that split, for
//! why a subproject is a shelf beside its parent rather than a subtree
//! inside it, and for why these roots may not overlap even though the
//! Files layer is happy to nest its own.
//!
//! Default data root is `$HOME/.task/` (override with
//! `TASK_DATA_ROOT`). Per-org databases live under
//! `<data_root>/orgs/<slug>/{auth,timer,finance}.sqlite`.
//! Client-side vault checkouts default to
//! `$HOME/Documents/Task/` ([`default_client_vault_root`]) so
//! a thin client can mount a slice of content separately from
//! the server's full state tree. See
//! the federated-platform design for the full federation
//! model.

use std::path::{Path, PathBuf};

use crate::manifest::{OrgManifest, ParseError};

/// The slug the org's long-standing curated tier (`wiki/Knowledge/`)
/// appears under in [`OrgRoot::named_wikis`].
///
/// It predates named wikis and keeps its own directory, but it is not
/// privileged: it is one member of the set, and code that enumerates
/// wikis must not special-case it.
pub const DEFAULT_WIKI: &str = "knowledge";

/// A wiki's slug from a display name — lowercase, non-alphanumerics
/// collapsed to single hyphens, no leading or trailing hyphen.
///
/// The same slug names the directory under `<org>/wikis/` and sits in
/// the middle of every reference into the wiki
/// (`acme.test/music-theory::Ionian`). Those two must agree, so both
/// come from here.
#[must_use]
pub fn wiki_slug(title: &str) -> String {
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

#[derive(Debug, thiserror::Error)]
pub enum RootError {
    #[error("resolve data root: {0}")]
    Resolve(String),
    #[error("create {path}: {source}")]
    Create {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("scan {path}: {source}")]
    Scan {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("org `{slug}` already exists at {path}")]
    AlreadyExists { slug: String, path: String },
    #[error("invalid slug `{slug}`: {reason}")]
    InvalidSlug { slug: String, reason: &'static str },
    #[error(transparent)]
    Manifest(#[from] ParseError),
}

/// Top-level data root. One per task-server process. Holds
/// `orgs/` plus any cross-org artifacts (server keypair,
/// future federation discovery cache).
#[derive(Debug, Clone)]
pub struct DataRoot {
    path: PathBuf,
}

impl DataRoot {
    /// Wrap an explicit path. Caller is responsible for
    /// ensuring it exists (or calling [`Self::ensure`]).
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    /// Resolve the canonical default:
    /// `$TASK_DATA_ROOT` → `$HOME/.task`. Picks an env-var
    /// override path first so tests (and per-org server
    /// instances) can point at temp dirs.
    ///
    /// `~/.task/` keeps the server-side state (orgs, identity,
    /// timer/finance DBs, blob attachments) together under one
    /// hidden dir. Client-side vault checkouts live separately
    /// — see [`default_client_vault_root`] — so a thin client
    /// can hold just the slice of content it cares about
    /// without dragging the full server data root along.
    pub fn from_env() -> Result<Self, RootError> {
        if let Ok(explicit) = std::env::var("TASK_DATA_ROOT") {
            if !explicit.is_empty() {
                return Ok(Self::new(PathBuf::from(explicit)));
            }
        }
        let home = std::env::var("HOME")
            .map_err(|_| RootError::Resolve("neither TASK_DATA_ROOT nor HOME is set".into()))?;
        Ok(Self::new(PathBuf::from(home).join(".task")))
    }

    /// Path to the root itself (`<data_root>`).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// `<data_root>/orgs/`.
    #[must_use]
    pub fn orgs_dir(&self) -> PathBuf {
        self.path.join("orgs")
    }

    /// `<data_root>/server-key.ed25519` — the blob signing
    /// keypair, shared across hosted orgs.
    #[must_use]
    pub fn server_keypair_path(&self) -> PathBuf {
        self.path.join("server-key.ed25519")
    }

    /// Create `<data_root>/` + `orgs/` if missing. Idempotent.
    pub fn ensure(&self) -> Result<(), RootError> {
        for p in [&self.path, &self.orgs_dir()] {
            std::fs::create_dir_all(p).map_err(|source| RootError::Create {
                path: p.display().to_string(),
                source,
            })?;
        }
        Ok(())
    }

    /// Resolver for a single org by slug. Does **not** check
    /// that the org exists on disk — use [`OrgRoot::manifest`]
    /// or [`Self::load_org`] for that.
    #[must_use]
    pub fn org(&self, slug: impl Into<String>) -> OrgRoot {
        let slug = slug.into();
        OrgRoot {
            path: self.orgs_dir().join(&slug),
            slug,
        }
    }

    /// Scaffold a fresh org dir. Refuses to overwrite an
    /// existing one — that's a federation-breaking change a
    /// human should confirm.
    pub fn init_org(
        &self,
        slug: &str,
        display_name: &str,
        is_home: bool,
    ) -> Result<OrgRoot, RootError> {
        validate_slug(slug)?;
        self.ensure()?;
        let org = self.org(slug);
        if org.path().exists() {
            return Err(RootError::AlreadyExists {
                slug: slug.to_owned(),
                path: org.path().display().to_string(),
            });
        }
        std::fs::create_dir_all(org.path()).map_err(|source| RootError::Create {
            path: org.path().display().to_string(),
            source,
        })?;
        let manifest = OrgManifest::new(slug, display_name, is_home);
        manifest.write_to_dir(org.path())?;
        // Sub-dirs the downstream features will use. Created
        // up-front so a fresh `OrgRoot` is immediately usable
        // — no "first write creates the dir" surprises.
        // `vault/` is personal; `wiki/Knowledge/` is curated
        // (LLM-Wiki shape); `wiki/LLM/` is loose scratch the
        // agents own (memories, journals, run logs).
        // `assets/<kind>/` is created up-front for the same reason the
        // rest of these are: Task's own lanes (charts, songs) write
        // there, and a shelf that does not exist is a shelf
        // `LocalOrgs::admits` refuses — so a fresh org would publish an
        // empty song library as "no such shelf" rather than as an empty
        // one, which is a different and worse answer.
        let asset_dirs: Vec<String> = crate::DEFAULT_ASSET_KINDS
            .iter()
            .map(|k| format!("assets/{k}"))
            .collect();
        // `projects/` is the tier directory and not a shelf — a fresh
        // org holds no projects. It is created anyway so that
        // `project_shelves` reads an empty directory rather than a
        // missing one, and so declaring the org's first project is a
        // write rather than a `NotFound`. Exactly the reason the asset
        // *kinds* are scaffolded; the difference is that Task itself
        // writes those two kinds and nobody can predict a project's
        // name.
        for sub in [
            "vault",
            "attachments",
            "projects",
            "wiki/Knowledge",
            "wiki/LLM/Memories",
            "wiki/LLM/Journals",
        ]
        .into_iter()
        .map(str::to_owned)
        .chain(asset_dirs)
        {
            let p = org.path().join(sub);
            std::fs::create_dir_all(&p).map_err(|source| RootError::Create {
                path: p.display().to_string(),
                source,
            })?;
        }
        Ok(org)
    }

    /// Load an existing org by slug. Returns
    /// `RootError::Manifest(ParseError::Read{..})` when the
    /// org doesn't exist on disk.
    pub fn load_org(&self, slug: &str) -> Result<(OrgRoot, OrgManifest), RootError> {
        let org = self.org(slug);
        let manifest = OrgManifest::load_from_dir(org.path())?;
        Ok((org, manifest))
    }

    /// Enumerate every org dir under `orgs/` that has a
    /// loadable `org.toml`. Dirs without a manifest are
    /// silently skipped — they may be partial scaffolds or
    /// unrelated files.
    pub fn scan_orgs(&self) -> Result<Vec<(OrgRoot, OrgManifest)>, RootError> {
        let dir = self.orgs_dir();
        if !dir.exists() {
            return Ok(Vec::new());
        }
        let entries = std::fs::read_dir(&dir).map_err(|source| RootError::Scan {
            path: dir.display().to_string(),
            source,
        })?;
        let mut out = Vec::new();
        for entry in entries.flatten() {
            if !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let Some(slug) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            let org = self.org(&slug);
            match OrgManifest::load_from_dir(org.path()) {
                Ok(m) => out.push((org, m)),
                Err(_) => continue,
            }
        }
        out.sort_by(|a, b| a.0.slug().cmp(b.0.slug()));
        Ok(out)
    }
}

/// One org's on-disk root. Pure path resolver — no I/O until
/// you call a method that actually touches the file system.
#[derive(Debug, Clone)]
pub struct OrgRoot {
    slug: String,
    path: PathBuf,
}

impl OrgRoot {
    #[must_use]
    pub fn slug(&self) -> &str {
        &self.slug
    }

    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    #[must_use]
    pub fn manifest_path(&self) -> PathBuf {
        self.path.join("org.toml")
    }

    #[must_use]
    pub fn auth_db(&self) -> PathBuf {
        self.path.join("auth.sqlite")
    }

    #[must_use]
    pub fn identity_db(&self) -> PathBuf {
        self.path.join("identity.sqlite")
    }

    /// Which orgs on this server a principal belongs to, and with what
    /// role in each. Only the HOME org's copy is consulted — it is the
    /// server's identity authority — so this file is absent in every
    /// other org, and a non-home org keeps its own `auth.sqlite` as the
    /// local identity it would fall back to if detached onto its own
    /// server (one account per server).
    #[must_use]
    pub fn memberships_db(&self) -> PathBuf {
        self.path.join("memberships.sqlite")
    }

    #[must_use]
    pub fn timer_db(&self) -> PathBuf {
        self.path.join("timer.sqlite")
    }

    #[must_use]
    pub fn finance_db(&self) -> PathBuf {
        self.path.join("finance.sqlite")
    }

    #[must_use]
    pub fn threads_db(&self) -> PathBuf {
        self.path.join("threads.sqlite")
    }

    #[must_use]
    pub fn prefs_db(&self) -> PathBuf {
        self.path.join("prefs.sqlite")
    }

    #[must_use]
    pub fn vault_dir(&self) -> PathBuf {
        self.path.join("vault")
    }

    /// `<org>/issuer.toml` — billing identity for invoices.
    /// Sibling of `org.toml`; see [`crate::issuer`] for why
    /// it's not part of the federated manifest.
    #[must_use]
    pub fn issuer_path(&self) -> PathBuf {
        self.path.join("issuer.toml")
    }

    /// `<org>/wiki/` — sibling of `vault/`. The wiki is its
    /// own tree so highly-curated knowledge (Knowledge/) and
    /// loose LLM scratch space (LLM/) don't pollute the
    /// vault's user-facing files.
    ///
    /// **Layered access rule** (enforced by convention; lint
    /// is a future follow-up):
    ///
    /// - `vault/`     ← can link → `wiki/Knowledge/`, `wiki/LLM/`
    /// - `wiki/LLM/`  ← can link → `wiki/Knowledge/`
    /// - `wiki/Knowledge/` ← stays self-contained; no
    ///   outbound links to `vault/` or `wiki/LLM/`. It's the
    ///   curated, generalizable tier (even if the knowledge
    ///   itself is private).
    ///
    /// One-directional dependency keeps Knowledge clean —
    /// you can rebuild the whole vault and the wiki/LLM
    /// scratch from scratch without re-curating Knowledge.
    #[must_use]
    pub fn wiki_dir(&self) -> PathBuf {
        self.path.join("wiki")
    }

    /// `<org>/wiki/Knowledge/` — the curated knowledge base.
    /// Mirrors the LLM-Wiki project layout (`schema.md`,
    /// `purpose.md`, `log.md`, `concepts/`, `entities/`,
    /// `Cookbook/`, …). This is what the structured wiki
    /// backend (`wiki-live::WikiLive`) is rooted at.
    ///
    /// By convention, Knowledge stays self-contained — no
    /// outbound `[[…]]` links to anything outside itself. The
    /// other tiers (vault, LLM scratch) link IN to Knowledge,
    /// never the other way.
    #[must_use]
    pub fn wiki_knowledge_dir(&self) -> PathBuf {
        self.wiki_dir().join("Knowledge")
    }

    /// `<org>/wikis/` — the named wikis an org holds, one directory
    /// (see [`Self::named_wikis`] for the set as a whole)
    /// per wiki, keyed by the slug a reference carries
    /// (`acme.test/music-theory::Ionian` → `wikis/music-theory/`).
    ///
    /// A sibling of `wiki/` rather than a child of it, so a wiki can
    /// be named anything without colliding with the reserved
    /// `Knowledge/` and `LLM/` tiers. Each subtree has the same shape
    /// [`Self::wiki_knowledge_dir`] does — `wiki-live` is rooted at
    /// one of these exactly as it is rooted at Knowledge.
    #[must_use]
    pub fn wikis_dir(&self) -> PathBuf {
        self.path.join("wikis")
    }

    /// `<org>/wikis/<slug>/` — one named wiki's root.
    #[must_use]
    pub fn named_wiki_dir(&self, slug: &str) -> PathBuf {
        self.wikis_dir().join(slug)
    }

    /// Every wiki this org holds, as `(slug, root)`.
    ///
    /// t[impl wiki.many.set] — the set is what is on disk, so creating
    /// a wiki is creating its directory and no wiki is privileged. The
    /// default wiki (`wiki/Knowledge/`) is in the set under the slug
    /// `knowledge`, which is what makes the one-wiki case a set of
    /// size one rather than a separate code path.
    ///
    /// Sorted by slug, so a caller enumerating wikis gets a stable
    /// order rather than the filesystem's. Missing directories are
    /// absent rather than an error: an org with no `wikis/` holds
    /// whatever `wiki/Knowledge/` is, which may itself be nothing.
    #[must_use]
    pub fn named_wikis(&self) -> Vec<(String, PathBuf)> {
        let mut out = Vec::new();
        let knowledge = self.wiki_knowledge_dir();
        if knowledge.is_dir() {
            out.push((DEFAULT_WIKI.to_string(), knowledge));
        }
        if let Ok(entries) = std::fs::read_dir(self.wikis_dir()) {
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let Some(slug) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                // A slug that is not the directory name would make the
                // reference in a page and the folder on disk disagree,
                // and the disagreement would only surface as an
                // unresolved link. Skip rather than guess.
                if slug != wiki_slug(&slug) {
                    continue;
                }
                out.push((slug, path));
            }
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// `<org>/wiki/LLM/` — LLM scratch space (memories,
    /// journals, agent logs). No enforced schema; tools write
    /// here freely without spilling into curated `Knowledge/`.
    /// LLM/ pages MAY link into `wiki/Knowledge/`; Knowledge
    /// never links back.
    #[must_use]
    pub fn wiki_llm_dir(&self) -> PathBuf {
        self.wiki_dir().join("LLM")
    }

    #[must_use]
    pub fn attachments_dir(&self) -> PathBuf {
        self.path.join("attachments")
    }

    /// `<org>/resources/` — the **Resources Library**: a sibling tree of
    /// `vault/` and `wiki/` holding large primary-source material (books
    /// in markdown / epub / txt, and the Bible). It is the third
    /// knowledge tier and the link-in target:
    ///
    /// - `vault/` ← can link → `wiki/Knowledge/`, `resources/`
    /// - `wiki/`  ← can link → `resources/` (its primary sources)
    /// - `resources/` ← self-contained; never links out.
    ///
    /// Corpora are too large for the vault (they'd drown the user's
    /// files) and never live in the git repo. They install here and
    /// sync to the server like the rest of the org tree.
    ///
    /// A resource is a typed subtree; the Bible lives at
    /// `resources/bible/<TRANSLATION>/` as per-book USFM, located by a
    /// `scripture_proto::VerseId`.
    #[must_use]
    pub fn resources_dir(&self) -> PathBuf {
        self.path.join("resources")
    }

    /// `<org>/resources/bible/<translation>/` — one Bible edition's
    /// per-book USFM (e.g. `WEB/JHN.usfm`).
    #[must_use]
    pub fn bible_dir(&self, translation: &str) -> PathBuf {
        self.resources_dir().join("bible").join(translation)
    }

    /// `<org>/assets/` — the **Assets tier**, a sibling of `vault/`,
    /// `wiki/`, `wikis/` and `resources/`.
    ///
    /// ADR 0004 decision 1. Assets are the things that will be useful
    /// later rather than the inner workings of a knowledge base: a
    /// chart, a song, a session file. Any file, any directory, any
    /// size.
    ///
    /// A sibling and not a subtree of `vault/`, which an earlier draft
    /// of the ADR had it be. The draft's reason was that being in the
    /// vault is what makes a file collaborative, and that is simply not
    /// how it works — see [`crate::shelf`]. Collaboration follows
    /// registration; `wiki/` is outside the vault and collaborative
    /// already. The cost of the draft was real: a shelf of songs
    /// sitting in the middle of somebody's notes, and — because a vault
    /// is never subscribable — no way for another organisation to
    /// reach it.
    ///
    /// One directory per **kind** ([`Self::asset_shelf_dir`]), because
    /// a kind is the unit somebody publishes and subscribes to.
    #[must_use]
    pub fn assets_dir(&self) -> PathBuf {
        self.path.join("assets")
    }

    /// `<org>/assets/<kind>/` — one asset kind's shelf root.
    #[must_use]
    pub fn asset_shelf_dir(&self, kind: &str) -> PathBuf {
        self.assets_dir().join(kind)
    }

    /// `<org>/projects/` — the **Projects tier**, ADR 0004's fourth
    /// root and the last of them to move.
    ///
    /// One directory per top-level project, and **everything a project
    /// is** lives inside it: its declaring page, its sub-projects, its
    /// working files, its deliverables. Nothing about a project is kept
    /// anywhere else, and other things reach it by reference.
    /// [`Self::project_shelves`] is the argument for both halves of
    /// that — why the page moved out of the vault, and why a
    /// sub-project is a subtree rather than a shelf of its own.
    #[must_use]
    pub fn projects_dir(&self) -> PathBuf {
        self.path.join("projects")
    }

    /// `<org>/projects/<slug>/` — one top-level project's directory,
    /// which is the whole of that project.
    ///
    /// The slug is [`crate::wiki_slug`] of the project's title — the
    /// same slugging every other named thing in the org uses, so a
    /// reference into a project spells its name the way a reference
    /// into a wiki spells its.
    #[must_use]
    pub fn project_shelf_dir(&self, slug: &str) -> PathBuf {
        self.projects_dir().join(slug)
    }

    /// `<org>/projects/<path>/project.md` — a project's declaring page,
    /// wherever in the tier it sits.
    ///
    /// `path` is tier-relative, so a top-level project is `crescendum`
    /// and one of its sub-projects is `crescendum/track-two`. That is
    /// the same string [`crate::PROJECT_PAGE`] is appended to, and the
    /// same string a reference carries.
    #[must_use]
    pub fn project_page(&self, rel: &str) -> PathBuf {
        self.projects_dir().join(rel).join(crate::PROJECT_PAGE)
    }

    /// The **legacy** home of a project's tree: `<org>/files/Projects/`.
    ///
    /// Where every project on every deployment sits until the migration
    /// has run. Kept as a resolver rather than as a string in three
    /// places, because the migration, the mount layout and the seeder
    /// all have to agree on it, and a fourth spelling of it is how one
    /// of them ends up looking in the wrong directory.
    #[must_use]
    pub fn legacy_projects_dir(&self) -> PathBuf {
        self.path.join("files").join("Projects")
    }

    /// Every project shelf this org holds, as `(slug, root)`.
    ///
    /// # Why this is a set on disk, like the wikis and the asset kinds
    ///
    /// Same rule, third time (`wiki.many.set`): the set is what the
    /// directory holds. A project created while the server runs needs
    /// no code change to be registered, published and subscribable, and
    /// a project restored from a backup by `cp -r` is simply there.
    ///
    /// # A project is self-contained, page included
    ///
    /// The project's declaring page moved here too. It used to be a
    /// vault note at `vault/Projects/<slug>.md` while its bytes were a
    /// File Root somewhere else, and the split cost more than it
    /// bought.
    ///
    /// What it cost is best said as a question a person actually asks:
    /// *where is this project?* Under the split there were two honest
    /// answers and no way to give one. Copying a project to another
    /// machine meant copying two things from two places and hoping the
    /// join survived. Archiving one meant remembering the page. Handing
    /// one to another organisation meant a subscription to the tree and
    /// a separate story for the declaration. Every one of those is the
    /// same defect: a project had no single location, so nothing could
    /// be done to a project as a whole.
    ///
    /// Now it does. `<org>/projects/crescendum/` is the project — its
    /// `project.md`, its sub-projects, its sessions, its
    /// `Deliverables/`. `cp -r` of that directory is a copy of the
    /// project. `rm -r` is a deletion of it. A subscription to the
    /// shelf is a share of it. And `project.identity.stable`'s promise
    /// — *"a project carried to another machine by `cp -r` arrives
    /// intact"* — becomes true of the whole project rather than of the
    /// half of it that was in the vault.
    ///
    /// **The page is still an ordinary markdown note.** It is
    /// `project.md` at the root of the project's own directory, with
    /// the same `type: project` frontmatter it always had, and
    /// `project.identity.declaration` reads the same way: *"an ordinary
    /// note — greppable, and editable in any editor — so nothing
    /// outside the file is needed to interpret it."* What changed is
    /// which registered root it sits in, and per `crate::shelf` that
    /// changes nothing about it: a shelf is registered on
    /// `vault::Backend`, `GraphBackend` and `VaultCollab` exactly as
    /// the vault is, so the page keeps its CRDT document, its
    /// wikilinks, its search and its live editor. Collaboration follows
    /// registration, not location — the whole point of that module.
    ///
    /// **What it costs, stated plainly.** A project is no longer in the
    /// vault's page index, so a vault-wide search or a `.base` view
    /// filtering `type: project` no longer finds one. That is the
    /// trade: a project is its own thing, reached by reference, and
    /// references are what `links` is for. `links_proto::NodeKind`
    /// resolves a `project:` reference to this tier the same way it
    /// resolves `song:` to the assets tier after ADR 0004, which is why
    /// "referenced as needed" is a mechanism here and not a hope.
    ///
    /// # A sub-project is a submodule, and this list is not there yet
    ///
    /// The model a sub-project is meant to follow is git's submodule: a
    /// sub-project is **its own shelf at any depth**, and its parent
    /// holds a *reference* to it rather than swallowing it. Somebody
    /// taking a copy of the album chooses — take everything, or take
    /// the surface and know from a named, unresolved reference that
    /// `track-two` exists and has not been materialised. That second
    /// state is the ordinary one ADR 0004 describes for any reference
    /// to something not resident locally, and it is emphatically not an
    /// error and not a silent absence.
    ///
    /// **This method returns only top-level directories today, and
    /// that is a limitation rather than the design.** What is missing is
    /// one specific mechanism, and it is worth naming precisely,
    /// because everything else about the submodule model is already
    /// expressible.
    ///
    /// Shelf roots may not overlap. A shelf is registered on
    /// `vault::Backend`, `GraphBackend` and `VaultCollab`, and **none
    /// of those three prunes**. Register both `projects/crescendum` and
    /// `projects/crescendum/track-two` as things stand and every file
    /// under the child sits in two roots at once: two link graphs
    /// claiming it, two watchers announcing each write, and two CRDT
    /// document ids over one file — which is the silent clobbering
    /// `crate::shelf`'s module docs exist to prevent. So until the
    /// prune exists, enumerating a nested project here would not
    /// deliver submodules; it would deliver corruption.
    ///
    /// The *Files* layer already solved exactly this and is the
    /// template. `files::registry::Registry::conflicting_root`
    /// deliberately allows a root inside a root — "a song inside an
    /// album" is its own example — because `files::scan::walk_live_tree`
    /// prunes the inner root's directory out of the outer root's walk,
    /// and its doc says in as many words: *"the prune is what makes
    /// relaxing this safe, so the two must stay together."* The
    /// collaboration layer needs the same pair. Concretely: the vault
    /// walk (`vault_live::walker::walk_vault`) and the manifest walk
    /// (`vault_live::sync::collect`) must stop at a directory that
    /// declares itself a shelf, the way `sync::is_root_internal`
    /// already stops at a File Root's own bookkeeping. On this tier the
    /// declaration is [`crate::PROJECT_PAGE`].
    ///
    /// Once that lands, this method changes to
    /// [`crate::project_page::walk_projects`] and every project at
    /// every depth is a shelf. Nothing else has to move: the vault id
    /// is already `project:<rel>`-shaped, the trait already answers
    /// every question a nested shelf asks, and
    /// [`crate::project_page::FoundProject`] already carries the
    /// tier-relative path a nested shelf would be keyed by.
    ///
    /// # The depth question, and what `Selection` cannot say
    ///
    /// "Take everything" versus "take the surface and know the
    /// sub-project is there" is a subscription decision, and ADR 0004
    /// decision 1a's [`crate::Selection`] is where subscription
    /// decisions belong. It cannot express this one, and the reason is
    /// exact rather than incidental:
    ///
    /// [`crate::Selection::admits`] is a **predicate over paths inside
    /// one root**. Under the submodule model a sub-project's files are
    /// not paths inside its parent's root — they are pruned out of it —
    /// so no predicate of that shape can reach them, whatever facets it
    /// names. `Facets` narrows *within* a shelf; depth chooses *which
    /// shelves*. Those are different questions and one type answering
    /// both would be the "second selection system with its own rules"
    /// the ADR warns against, arrived at from the other direction.
    ///
    /// So the extension is a second field on the subscription beside
    /// `Selection`, not a third variant inside it, and the stated
    /// default is **surface-only**: a subscription that says nothing
    /// takes the shelf it named and leaves its sub-shelves as visible,
    /// named, unresolved references. That is the safe direction — it
    /// cannot pull gigabytes nobody asked for, and the reference stays
    /// on screen so a person can choose — and it is the same
    /// conservative stance [`crate::Selection::admits`] already takes
    /// about unmapped content.
    ///
    /// **A cycle becomes expressible** the moment a parent references a
    /// sub-project by name: A may reference B may reference A, and a
    /// person can write that by hand. It is tolerated rather than
    /// refused at write time — refusing would mean every write
    /// validating a graph it may hold only part of, which
    /// `project.location.degraded` says must still work — and stopped
    /// at materialisation, by a visited-set over shelf ids. The same
    /// stance `ProjectService::get` already takes for merge chains,
    /// which are the identical hazard one field over.
    ///
    /// `project.nesting.explicit` is not violated by any of this. It
    /// forbids *inferring* parentage from directory containment, and
    /// nothing here infers: a child's `project.md` declares `parentId`,
    /// and that declaration is the parentage. The spec's own next
    /// sentence grants the layout — *"A project's files usually live
    /// under its parent's directory and need not."*
    ///
    /// A directory that is not its own slug is skipped for the same
    /// reason a wiki's is: the reference and the folder would disagree,
    /// and the disagreement would surface only as a link that does not
    /// resolve.
    ///
    /// # A directory is a shelf before it is a project
    ///
    /// This method does **not** check for a `project.md`, and
    /// [`crate::project_page::walk_projects`] does. That is not an
    /// inconsistency; the two answer different questions.
    ///
    /// "Is this a project?" is `project.identity.declaration`, and the
    /// answer is the page. "Should this directory be registered for
    /// sync, the link graph and per-file CRDT?" is a question about
    /// bytes, and the answer is yes for anything sitting on the tier.
    /// They come apart in exactly one situation and it is a common one:
    /// a project arriving by sync, whose files land before its page
    /// does. A shelf that refused to register until the page appeared
    /// would be a directory whose files are silently not collaborative
    /// — no error, no warning, and two people editing one file
    /// clobbering each other, which is the failure `crate::shelf`'s
    /// module docs exist to prevent. Registering a directory that turns
    /// out to hold no project costs a watcher and an empty graph root.
    ///
    /// Sorted by slug, so a caller enumerating projects gets a stable
    /// order rather than the filesystem's.
    #[must_use]
    pub fn project_shelves(&self) -> Vec<(String, PathBuf)> {
        let mut slugs: Vec<String> = Vec::new();
        if let Ok(entries) = std::fs::read_dir(self.projects_dir()) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let Some(slug) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                if slug != wiki_slug(&slug) || slugs.contains(&slug) {
                    continue;
                }
                slugs.push(slug);
            }
        }
        slugs.sort();
        slugs
            .into_iter()
            .map(|s| {
                let root = self.project_shelf_dir(&s);
                (s, root)
            })
            .collect()
    }

    /// Every asset shelf this org holds, as `(kind, root)`.
    ///
    /// The set is what is on disk — exactly the rule
    /// [`Self::named_wikis`] follows for wikis (`wiki.many.set`) —
    /// unioned with [`crate::DEFAULT_ASSET_KINDS`], the kinds Task's
    /// own lanes write and which therefore have to be registered
    /// before anybody has used them.
    ///
    /// Reading the set from disk rather than from an enum is what lets
    /// an application put a shelf here that Task has never heard of and
    /// have it registered, published and subscribable on the same terms
    /// as a chart library. ADR 0004 decision 2 is the same rule for
    /// collections; this is it for storage.
    ///
    /// Sorted by kind, so a caller enumerating shelves gets a stable
    /// order rather than the filesystem's.
    #[must_use]
    pub fn asset_shelves(&self) -> Vec<(String, PathBuf)> {
        let mut kinds: Vec<String> = crate::DEFAULT_ASSET_KINDS
            .iter()
            .map(|k| (*k).to_owned())
            .collect();
        if let Ok(entries) = std::fs::read_dir(self.assets_dir()) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let Some(kind) = entry.file_name().to_str().map(str::to_owned) else {
                    continue;
                };
                // A kind whose directory name is not its own slug would
                // make the reference in a page and the folder on disk
                // disagree, and the disagreement would surface only as
                // an unresolved link. Skip rather than guess — the same
                // rule `named_wikis` applies, for the same reason.
                if kind != wiki_slug(&kind) || kinds.contains(&kind) {
                    continue;
                }
                kinds.push(kind);
            }
        }
        kinds.sort();
        kinds
            .into_iter()
            .map(|k| {
                let root = self.asset_shelf_dir(&k);
                (k, root)
            })
            .collect()
    }

    /// Every shelf this org holds — its vault, each named wiki, each
    /// asset shelf — as one list.
    ///
    /// **This is the list the server's registration loop consumes**,
    /// and having exactly one such list is the point of
    /// [`crate::Shelf`]. Before it, the vault was registered by one
    /// piece of code and the wikis by another, and a third tier meant a
    /// third; see that module for why two spellings of one act is a
    /// defect rather than a style.
    ///
    /// The vault comes first because it is the one shelf an org always
    /// has, and the order is otherwise the stable order of
    /// [`Self::named_wikis`], then [`Self::asset_shelves`], then
    /// [`Self::project_shelves`].
    #[must_use]
    pub fn shelves(&self) -> Vec<Box<dyn crate::Shelf>> {
        let mut out: Vec<Box<dyn crate::Shelf>> =
            vec![Box::new(crate::VaultShelf::new(self.vault_dir()))];
        out.extend(self.named_wikis().into_iter().map(|(slug, root)| {
            Box::new(crate::WikiShelf::new(slug, root)) as Box<dyn crate::Shelf>
        }));
        out.extend(self.asset_shelves().into_iter().map(|(kind, root)| {
            Box::new(crate::AssetShelf::new(kind, root)) as Box<dyn crate::Shelf>
        }));
        out.extend(self.project_shelves().into_iter().map(|(slug, root)| {
            Box::new(crate::ProjectShelf::new(slug, root)) as Box<dyn crate::Shelf>
        }));
        out
    }

    pub fn manifest(&self) -> Result<OrgManifest, ParseError> {
        OrgManifest::load_from_dir(&self.path)
    }
}

/// Default client-side vault root:
/// `$TASK_VAULT_ROOT` → `$HOME/Documents/Task`.
///
/// The client checkout is intentionally separate from the
/// server data root ([`DataRoot`]) so a thin client (laptop,
/// phone) can mount a small slice of content under a
/// user-visible folder without holding the full
/// `<data_root>/orgs/<slug>/vault` tree the server keeps.
/// Per-machine [`mount-proto::MountRegistry`] entries point
/// at sub-paths under this root by default.
pub fn default_client_vault_root() -> Result<PathBuf, RootError> {
    if let Ok(explicit) = std::env::var("TASK_VAULT_ROOT") {
        if !explicit.is_empty() {
            return Ok(PathBuf::from(explicit));
        }
    }
    let home = std::env::var("HOME")
        .map_err(|_| RootError::Resolve("neither TASK_VAULT_ROOT nor HOME is set".into()))?;
    Ok(PathBuf::from(home).join("Documents").join("Task"))
}

/// Slug rules: lowercase ASCII, digits, `-`. Non-empty,
/// max 64 chars. Matches the `/org/<slug>/...` URL contract;
/// also keeps directory names portable across filesystems.
fn validate_slug(slug: &str) -> Result<(), RootError> {
    if slug.is_empty() {
        return Err(RootError::InvalidSlug {
            slug: slug.to_owned(),
            reason: "must not be empty",
        });
    }
    if slug.len() > 64 {
        return Err(RootError::InvalidSlug {
            slug: slug.to_owned(),
            reason: "must be ≤ 64 chars",
        });
    }
    for c in slug.chars() {
        if !(c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-') {
            return Err(RootError::InvalidSlug {
                slug: slug.to_owned(),
                reason: "only [a-z0-9-] allowed",
            });
        }
    }
    if slug.starts_with('-') || slug.ends_with('-') {
        return Err(RootError::InvalidSlug {
            slug: slug.to_owned(),
            reason: "must not start or end with `-`",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_scan_load_cycle() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        root.init_org("codywright", "Cody Wright", true).unwrap();
        root.init_org("fasttrackstudios", "FastTrackStudios", false)
            .unwrap();
        let scanned = root.scan_orgs().unwrap();
        assert_eq!(scanned.len(), 2);
        assert_eq!(scanned[0].0.slug(), "codywright");
        assert_eq!(scanned[1].0.slug(), "fasttrackstudios");
        assert!(scanned[0].1.is_home);
        let (_, m) = root.load_org("fasttrackstudios").unwrap();
        assert_eq!(m.display_name, "FastTrackStudios");
    }

    #[test]
    fn init_twice_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        root.init_org("dup", "x", false).unwrap();
        let err = root.init_org("dup", "y", false).unwrap_err();
        assert!(matches!(err, RootError::AlreadyExists { .. }));
    }

    #[test]
    fn invalid_slugs_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        for bad in [
            "",
            "UPPER",
            "with space",
            "starts-",
            "-ends",
            "tooo".repeat(20).as_str(),
        ] {
            assert!(
                root.init_org(bad, "x", false).is_err(),
                "expected reject: {bad:?}"
            );
        }
    }

    #[test]
    fn paths_compose_correctly() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        let org = root.init_org("cody", "Cody", false).unwrap();
        assert_eq!(org.auth_db().file_name().unwrap(), "auth.sqlite");
        assert_eq!(org.timer_db().file_name().unwrap(), "timer.sqlite");
        assert_eq!(org.prefs_db().file_name().unwrap(), "prefs.sqlite");
        assert!(org.vault_dir().exists());
        assert!(org.attachments_dir().exists());
    }

    /// t[verify wiki.many.set] — an org's wikis are the set on disk,
    /// the default tier is one member of it rather than a privileged
    /// case, and adding a wiki leaves the others exactly as they were.
    #[test]
    fn named_wikis_are_a_set_with_no_privileged_member() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        let org = root.init_org("acme-audio", "ACME Audio", true).unwrap();

        // A fresh org is scaffolded with `wiki/Knowledge/`, so it
        // starts as a set of size one rather than as a special case.
        assert_eq!(
            org.named_wikis(),
            vec![(DEFAULT_WIKI.to_string(), org.wiki_knowledge_dir())]
        );

        for slug in ["music-theory", "audio-production"] {
            std::fs::create_dir_all(org.named_wiki_dir(slug)).unwrap();
        }
        let wikis = org.named_wikis();
        let slugs: Vec<&str> = wikis.iter().map(|(s, _)| s.as_str()).collect();
        // Sorted, and the default tier sits among them by name rather
        // than at the front.
        assert_eq!(slugs, ["audio-production", DEFAULT_WIKI, "music-theory"]);

        // Each resolves to its own root: no two wikis share a
        // directory, which is what keeps their state separate.
        let roots: std::collections::HashSet<&std::path::Path> =
            wikis.iter().map(|(_, p)| p.as_path()).collect();
        assert_eq!(roots.len(), wikis.len());

        // Adding one did not disturb the others.
        assert!(org.wiki_knowledge_dir().is_dir());
    }

    /// A directory whose name is not already a slug would make the
    /// folder on disk and the reference in a page disagree, and the
    /// disagreement would surface only as a link that does not
    /// resolve. It is skipped rather than guessed at.
    #[test]
    fn a_directory_that_is_not_a_slug_is_not_a_wiki() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        let org = root.init_org("acme-audio", "ACME Audio", true).unwrap();
        std::fs::create_dir_all(org.wikis_dir().join("Music Theory")).unwrap();
        std::fs::create_dir_all(org.wikis_dir().join("music-theory")).unwrap();
        let slugs: Vec<String> = org.named_wikis().into_iter().map(|(s, _)| s).collect();
        assert_eq!(slugs, [DEFAULT_WIKI, "music-theory"]);
    }

    /// t[verify project.nesting.uniform] — an album and a song promoted
    /// out of it are two shelves side by side, and the set says nothing
    /// about which is which. Depth is not a property a shelf has.
    #[test]
    fn a_subproject_is_a_shelf_beside_its_parent_not_inside_it() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        let org = root.init_org("acme-audio", "ACME Audio", true).unwrap();

        // A fresh org has the tier and no projects on it.
        assert!(org.projects_dir().is_dir());
        assert!(org.project_shelves().is_empty());

        for slug in ["crescendum", "track-two", "Not A Slug"] {
            std::fs::create_dir_all(org.project_shelf_dir(slug)).unwrap();
        }
        let slugs: Vec<String> = org.project_shelves().into_iter().map(|(s, _)| s).collect();
        assert_eq!(
            slugs,
            ["crescendum", "track-two"],
            "a directory that is not its own slug is skipped, like a wiki's"
        );

        // The property the whole design turns on: no shelf root
        // contains another, so no file is ever in two roots at once.
        let roots: Vec<PathBuf> = org.project_shelves().into_iter().map(|(_, p)| p).collect();
        for a in &roots {
            for b in &roots {
                assert!(
                    a == b || !b.starts_with(a),
                    "{} contains {} — two vault roots over one file",
                    a.display(),
                    b.display()
                );
            }
        }
    }

    /// The one list the registration loop consumes holds all four
    /// tiers, each under its own namespaced id. Before the projects
    /// tier moved this list stopped at three, and the fourth was a
    /// File Root nobody registered for CRDT.
    #[test]
    fn every_tier_is_in_the_one_shelf_list() {
        let tmp = tempfile::tempdir().unwrap();
        let root = DataRoot::new(tmp.path().to_owned());
        let org = root.init_org("acme-audio", "ACME Audio", true).unwrap();
        std::fs::create_dir_all(org.named_wiki_dir("music-theory")).unwrap();
        std::fs::create_dir_all(org.project_shelf_dir("crescendum")).unwrap();

        let shelves = org.shelves();
        let ids: Vec<String> = shelves.iter().map(|s| s.vault_id()).collect();
        assert!(ids.contains(&crate::VAULT_ID.to_string()));
        assert!(ids.contains(&"wiki:music-theory".to_string()));
        assert!(ids.contains(&"assets:songs".to_string()));
        assert!(
            ids.contains(&"project:crescendum".to_string()),
            "the projects tier is registered by the same loop as the rest: {ids:?}"
        );

        // Every tier is represented, and no id is claimed twice.
        let tiers: std::collections::HashSet<crate::Tier> =
            shelves.iter().map(|s| s.tier()).collect();
        assert_eq!(tiers.len(), 4, "four roots, four tiers");
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len(), "two shelves share an id: {ids:?}");
    }

    #[test]
    fn wiki_slug_matches_what_a_reference_carries() {
        assert_eq!(wiki_slug("Music Theory"), "music-theory");
        assert_eq!(wiki_slug("Bible Study"), "bible-study");
        assert_eq!(wiki_slug("  Cooking!  "), "cooking");
        assert_eq!(wiki_slug(""), "");
    }
}
