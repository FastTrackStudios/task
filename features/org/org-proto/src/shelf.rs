//! [`Shelf`] — a named, file-backed tier of an organisation's tree.
//!
//! # Collaboration follows registration, not location
//!
//! This is the whole idea, and it is worth stating before any code,
//! because the mistake it corrects is one the repository made twice.
//!
//! Task has exactly one machinery for collaborative editing of files:
//! a directory is registered as a root on [`vault::Backend`] under some
//! *vault id*, a `GraphBackend` root is added under the same id, and
//! `VaultCollab::watch_vault(id)` folds every write on it into
//! whichever per-file Loro documents are open. Everything a person
//! means by "these files are collaborative" — the CRDT document, the
//! three-way merge against external writes, the link graph, search, the
//! live `changes` stream, the note editor — is downstream of those
//! three calls.
//!
//! Nothing in that sequence asks where the directory *is*. The proof is
//! already in the tree and predates this module: the `wiki/` tier is
//! not inside the vault, and wiki pages are collaborative anyway.
//! `apps/server/tests/wiki_editor_e2e.rs` says so in as many words —
//! the wiki editor is "`VaultSync` files, per-file CRDT collab" — and
//! `two_collab_sessions_converge_on_a_wiki_page` proves for a wiki page
//! exactly what `vault_collab_e2e` proves for a note in the vault.
//!
//! An earlier draft of ADR 0004 nevertheless made the Assets tier a
//! *subtree of the vault* (`<vault>/Assets/<Kind>/`), and the reason
//! given was that being in the vault is what buys collaboration. It is
//! not. Being registered is what buys collaboration. Filing assets
//! inside the vault to obtain it put a shelf of songs in the middle of
//! somebody's notes, and bought nothing that a third call to the same
//! registration would not have bought.
//!
//! So the roots are siblings — `vault/`, `wiki/`, `assets/<name>/`,
//! `projects/<name>/` — and what they share is a trait rather than a
//! hierarchy. `resources/` is not a fifth root: it is an asset group
//! that happens to be immutable and leaf-only.
//!
//! # Why a trait, and not a fourth branch in the boot sequence
//!
//! Before this module the server reached the same machinery two
//! different ways, and the asymmetry was invisible until a third tier
//! wanted in:
//!
//! ```text
//! vault:  Backend::single("default", root)   + collab.watch_vault("default")
//! wikis:  for (slug, root) in wiki_roots { wiki_vaults.attach(slug, root) }
//! ```
//!
//! Two spellings of one act. Adding assets by writing a third spelling
//! would have made the next tier's author write a fourth, and every one
//! of them is a place a step can be forgotten — the Assets tier that
//! forgets `watch_vault` is a tier whose files are silently not
//! collaborative, and nothing fails; it just quietly does not work.
//!
//! A trait removes the choice. Every shelf yields the same
//! `(name, root)` pair, and one loop registers all of them. If a future
//! reader finds a second registration path, the trait has stopped doing
//! its job and that is the bug.
//!
//! # Why this lives in `org-proto`
//!
//! Because a shelf is a fact about an *organisation's tree*, and this
//! crate already owns that tree: [`crate::OrgRoot`] is where
//! `vault_dir()`, `wikis_dir()`, `resources_dir()` and now
//! [`crate::OrgRoot::assets_dir`] are spelled, and
//! [`crate::OrgRoot::named_wikis`] already returns precisely the
//! `Vec<(String, PathBuf)>` the registration loop consumes. This trait
//! is the generalisation of that method, so it belongs beside it.
//!
//! The alternative placements are each wrong in an instructive way.
//! `vault-proto` would say a shelf is a kind of vault, which is the
//! claim this module exists to retract. `wiki-proto` would say a shelf
//! is a kind of wiki, which makes every future tier either a
//! wiki-shaped compromise or a second copy of the same machinery — ADR
//! 0004 names that failure explicitly. And `apps/server` would put a
//! domain rule in a binary, where the CLI, the seeder and the tests
//! cannot reach it.
//!
//! Layering holds: `org-proto` depends on no feature that depends on
//! it, and `wiki-proto` may name it (it does, for
//! [`Tier`] ↔ `SourceKind`), while `org-proto` names no wiki type.

use std::path::{Path, PathBuf};

use facet::Facet as FacetDerive;
use serde::{Deserialize, Serialize};

/// The `wiki:` vault-id prefix — a wiki's pages, served as a vault.
const WIKI_PREFIX: &str = "wiki:";

/// The `assets:` vault-id prefix — one asset kind's shelf.
const ASSETS_PREFIX: &str = "assets:";

/// The `project:` vault-id prefix — one project's tree.
const PROJECT_PREFIX: &str = "project:";

/// The vault id an org's own vault is registered under.
///
/// One vault per org, so it needs no name of its own; `"default"` is
/// what every client has always sent and changing it would be a wire
/// break for no gain.
pub const VAULT_ID: &str = "default";

/// Which of the four roots a shelf belongs to.
///
/// ADR 0004 decision 1: an organisation's file hierarchy has exactly
/// four roots — `vault/` (exactly one), `wiki/` (many, named),
/// `assets/` (many, named) and `projects/` (many, nested) — and all
/// four are shelves.
///
/// Four members and not five: `resources/` is deliberately **not** a
/// root. A resource is an asset group that happens to be immutable and
/// leaf-only — imports, held whole, never authored here — so it is a
/// property of a shelf rather than a shelf of its own kind. It has no
/// CRDT document to register and nothing to say about editability that
/// [`Shelf::is_editable`] cannot say.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tier {
    /// The org's own vault: `<org>/vault/`. Editable, and never
    /// subscribable — see [`Shelf::is_subscribable`].
    Vault,
    /// One named wiki: `<org>/wiki/Knowledge/` or `<org>/wikis/<slug>/`.
    Wiki,
    /// One asset group: `<org>/assets/<name>/`.
    Assets,
    /// One project's byte tree: `<org>/projects/<slug>/`.
    ///
    /// The fourth root, and the last to move. It was declared here one
    /// change before it existed, as a test of the trait: if a fourth
    /// tier had needed [`Shelf`] reshaped to fit, the trait would not
    /// have been an abstraction — it would have been a description of
    /// the three cases that happened to exist when it was written. It
    /// needed nothing. Every method a project answers, it answers with
    /// the trait's own default, and the only line
    /// [`crate::ProjectShelf`] adds beyond `tier`/`name`/`root` is the
    /// blank one between them.
    ///
    /// # A project is two halves, and only one of them is here
    ///
    /// A project is a **note** — a markdown page in the vault whose
    /// frontmatter declares `type: project`
    /// (`project.identity.declaration`) — and a **tree** of the bytes
    /// the work produced. This tier is the tree. The note stays in the
    /// vault, and [`crate::OrgRoot::project_shelves`] says at length
    /// why that split is the coherent one rather than a compromise.
    ///
    /// # Nested, like submodules
    ///
    /// ADR 0004 calls this tier "many, **nested**", and it means it
    /// literally: `projects/crescendum/track-two/` is a project inside
    /// a project, and the intended model is git's submodule — the child
    /// is a shelf in its own right, and the parent holds a *reference*
    /// to it rather than swallowing it. Taking a copy of the parent is
    /// then a choice between everything and the surface, with the
    /// sub-project visible either way as a named reference.
    ///
    /// [`crate::OrgRoot::project_shelves`] enumerates only the top
    /// level today, and says exactly which mechanism is missing before
    /// it can enumerate the rest: the collaboration layer has no
    /// equivalent of `files::scan::walk_live_tree`'s prune, so two
    /// overlapping shelf roots would put one file in two link graphs
    /// and two CRDT documents.
    ///
    /// The nesting on disk is not the parentage. Parentage is the
    /// child's declared `parentId`, because `project.nesting.explicit`
    /// forbids the inference a directory would invite: *"Parentage is a
    /// declared link, not a consequence of directory containment.
    /// Hardcoded directory names — `Projects/`, `Albums/` — express no
    /// hierarchy and are not consulted."* The directory says where the
    /// bytes are; the frontmatter says what the project belongs to.
    Projects,
}

impl Tier {
    /// The word a person reads for this tier.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vault => "vault",
            Self::Wiki => "wiki",
            Self::Assets => "assets",
            Self::Projects => "projects",
        }
    }

    /// Whether another organisation may subscribe to shelves of this
    /// tier.
    ///
    /// t[impl wiki.boundary.no-subscribe] — the vault answers `false`
    /// here and there is no flag anywhere that can turn it on. A vault
    /// is an org's private tree: sharing a note out of it goes through
    /// a share link, which grants reading one named thing and never
    /// makes it resolvable inside somebody else's writing.
    ///
    /// Assets answer `true`, and that is not a weakening of the rule —
    /// it is the point of moving them out of the vault. An asset shelf
    /// is a *published* shelf; a vault is not. Under the earlier
    /// `<vault>/Assets/` design the two questions had one answer,
    /// which is why cross-org asset reach had to be recorded as a
    /// regression instead of built.
    #[must_use]
    pub const fn is_subscribable(self) -> bool {
        match self {
            Self::Vault => false,
            Self::Wiki | Self::Assets | Self::Projects => true,
        }
    }
}

/// **Which part of a shelf** — the selection a subscription carries,
/// and the same selection a device's local sync will carry.
///
/// # One question asked at two scopes
///
/// ADR 0004 decision 1a. Subscribing says what an *organisation* may
/// see; syncing says what *this machine* bothers to hold. A studio
/// machine takes the multitracks; a phone takes the charts and leaves
/// forty gigabytes of stems on the server. Both are the question
/// *which part of this shelf?*, and the moment they are two types they
/// start meaning subtly different things and a person has to learn
/// both. So there is one type, here, in the crate both lanes can name.
///
/// # Why facet names, and not globs
///
/// Because the primitive already exists and inventing a second one is
/// how a codebase ends up with two selection systems whose interaction
/// nobody can predict. `files.sync.selective` is explicit: *"Devices
/// subscribe to a project type's facets, not to path globs"*, and
/// `files_domain::Facet` is the vocabulary — a newtype over exactly the
/// string held here, resolved from a path by `files_domain::FacetMap`,
/// with `Binding::atomic` carrying the rule that an atomic facet brings
/// its dependencies. A subscription naming `"stems"` and a laptop
/// naming `"stems"` mean one thing.
///
/// A glob would have been easier and is worse in a specific way: a glob
/// is a statement about a *layout*, so reorganising a shelf silently
/// changes what a subscriber receives. A facet is a statement about a
/// *class of content*, which survives the reorganisation — which is the
/// entire argument `files.facet.vocabulary` makes.
///
/// # The device-scope form already exists, and this is its wire shadow
///
/// `files_domain::hydration::Subscription` is `{ facets, pinned,
/// everything }` — what one device holds of one root — and
/// `hydration::decide` is the rule that turns it into resident-or-stub.
/// [`Selection::All`] is its `everything`; [`Selection::Facets`] is its
/// `facets`. What is deliberately *absent* here is `pinned`: a pin is a
/// path on a particular machine ("I am on a plane at six"), which is
/// meaningless as a statement one organisation makes to another. The
/// org scope is the subset of the device scope that survives being said
/// out loud, and the two share a vocabulary rather than a struct
/// precisely because that difference is real.
///
/// # Why the names are `String` and not `files_domain::Facet`
///
/// Dependency direction, and both directions are closed. `files-domain`
/// declares in its own manifest that it "knows nothing of architect,
/// vox or the org", so it may not name this crate; and it pulls
/// `files-store`, which no wire contract with a wasm baseline
/// (`wiki-proto`) may drag in. `Facet` is `pub struct Facet(pub
/// String)`, so the conversion at the one call site that will need both
/// is `Facet::new(name)` — a newtype wrap, not a translation, and there
/// is no second vocabulary to keep in step.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize, FacetDerive)]
#[serde(rename_all = "snake_case", tag = "mode", content = "facets")]
#[repr(u8)]
pub enum Selection {
    /// The whole shelf.
    ///
    /// The degenerate case, and the default — a subscription that says
    /// nothing takes everything, which is what every subscription
    /// written before this type existed meant.
    #[default]
    All,
    /// Only content resolving to one of these facets.
    ///
    /// An empty list is **not** "everything": it is a selection that
    /// matches nothing, and it is expressible on purpose. "I subscribe
    /// to this shelf's structure and none of its bytes" is a real
    /// thing to want — it is what a catalogue-only subscriber wants,
    /// and conflating it with `All` would make that unsayable.
    Facets(Vec<String>),
}

impl Selection {
    /// Whether content resolving to `facet` is inside this selection.
    ///
    /// `None` is content with no facet at all — unmapped, in
    /// `files.facet.*`'s terms. Unmapped content is inside [`Self::All`]
    /// and outside every [`Self::Facets`], which is the conservative
    /// direction: `files.facet.vocabulary` says unmapped content "syncs
    /// with the default" and is *reported* rather than guessed at, and
    /// silently shipping a stranger unclassified bytes because nobody
    /// had labelled them yet is the failure worth ruling out.
    #[must_use]
    pub fn admits(&self, facet: Option<&str>) -> bool {
        match self {
            Self::All => true,
            Self::Facets(names) => facet.is_some_and(|f| names.iter().any(|n| n == f)),
        }
    }

    /// Whether this selection is the whole shelf.
    #[must_use]
    pub const fn is_whole_shelf(&self) -> bool {
        matches!(self, Self::All)
    }

    /// How a person reads it: `everything`, or the facet list.
    #[must_use]
    pub fn describe(&self) -> String {
        match self {
            Self::All => "everything".to_owned(),
            Self::Facets(names) if names.is_empty() => "nothing".to_owned(),
            Self::Facets(names) => names.join(", "),
        }
    }
}

/// **How far down** a subscription reaches: this shelf, or this shelf
/// and the ones nested inside it.
///
/// # Why this is beside [`Selection`] and not inside it
///
/// The obvious move is a third `Selection` variant, and it is wrong for
/// a reason worth stating exactly, because "add a variant" will look
/// tempting again.
///
/// [`Selection::admits`] is a **predicate over paths inside one root**.
/// A nested shelf's files are not paths inside its parent's root —
/// `vault_live::shelf_boundary` prunes them out of the parent's walk,
/// which is the whole mechanism that lets a shelf hold a shelf at all —
/// so no predicate of that shape can reach them, whatever facets it
/// names. `Selection` narrows *within* a shelf; this chooses *which
/// shelves*. They are different questions, and one type answering both
/// is precisely the "second selection system with its own rules" ADR
/// 0004 warns against, arrived at from the other direction.
///
/// The two compose: a subscription carries one of each, and a
/// [`Depth::Deep`] subscription applies its `Selection` to every shelf
/// it reaches, so "the worship material, wherever in this project it
/// is" is one sentence rather than one per sub-project.
///
/// # Why the default is the shallow one
///
/// [`Depth::Surface`] is [`Default`], so a subscription written before
/// this type existed — and one written by somebody who did not think
/// about it — takes the shelf it named and no more. Two reasons, and
/// the first is the one that matters:
///
/// A deep default can pull an unbounded amount of somebody else's disk
/// on the strength of a request that never mentioned it. An album with
/// fifteen promoted songs is fifteen shelves of multitracks, and the
/// person who asked for the album asked for the album. The failure is
/// silent, expensive and remote — the worst combination available.
///
/// The second is that surface-only is not a *loss*. The sub-project is
/// still there: a named, unresolved reference, which ADR 0004 makes the
/// ordinary state of any reference to something not resident locally.
/// A person seeing "Track Two — not materialised" can ask for it. A
/// person whose laptop silently filled cannot un-ask.
///
/// This is the same conservative direction [`Selection::admits`] takes
/// about unmapped content, for the same reason: shipping bytes nobody
/// asked for is worse than reporting an absence.
///
/// # Cycles
///
/// A parent references a sub-project; a sub-project may reference a
/// project that references it back. A cycle is therefore writable by
/// hand, and it must not hang a materialisation or exhaust its memory.
///
/// It is **tolerated rather than refused at write time.** Refusing
/// would mean every write validating a graph the writer may hold only
/// part of, and `project.location.degraded` says a project must still
/// open when a location composing it is unreachable — so the check
/// would have to pass on evidence that is missing, which means it would
/// either refuse valid writes or not really be a check.
///
/// So it is stopped where the graph is actually being walked, by a
/// visited set over shelf names. That is the stance
/// `ProjectService::get` already takes for merge chains, which are the
/// identical hazard one field over: bounded traversal, and a message
/// naming what did not settle rather than a hang.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, FacetDerive)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum Depth {
    /// This shelf only. Shelves nested inside it stay visible as
    /// unresolved references.
    #[default]
    Surface,
    /// This shelf and every shelf nested inside it, transitively.
    Deep,
}

impl Depth {
    /// Whether a shelf nested inside a subscribed one is taken too.
    #[must_use]
    pub const fn reaches_nested(self) -> bool {
        matches!(self, Self::Deep)
    }

    /// How a person reads it.
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            Self::Surface => "this shelf only",
            Self::Deep => "this shelf and everything nested in it",
        }
    }
}

/// Walk a shelf and the shelves nested inside it, stopping at a cycle.
///
/// `nested` answers "which shelves does this one reference", and the
/// visited set is what makes a cycle terminate rather than hang — see
/// [`Depth`] on why a cycle is tolerated at write time and stopped
/// here.
///
/// Returns the shelves reached, in the order they were first seen, with
/// `start` always first. A [`Depth::Surface`] walk is `[start]`, which
/// is the degenerate case rather than a separate path — one code path
/// means a bug in the traversal cannot hide in the shallow case.
#[must_use]
pub fn reachable(start: &str, depth: Depth, nested: &dyn Fn(&str) -> Vec<String>) -> Vec<String> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut queue = std::collections::VecDeque::new();
    queue.push_back(start.to_owned());
    while let Some(name) = queue.pop_front() {
        // The set is checked on the way IN rather than on the way out,
        // so a shelf reached twice by two different parents is walked
        // once and a shelf that reaches itself terminates immediately.
        if !seen.insert(name.clone()) {
            continue;
        }
        out.push(name.clone());
        if depth.reaches_nested() {
            queue.extend(nested(&name));
        }
    }
    out
}

/// The facet a path on a shelf resolves to, for selection purposes.
///
/// The shelf's **top-level directory** is its facet, and nothing
/// deeper. That is not a simplification of `files_domain::FacetMap` so
/// much as the same rule at a different scope: a `FacetMap` resolves a
/// path inside a *project* against that project's capabilities and
/// overrides, because a project's tools created its layout. A shelf has
/// no tools and no capabilities — an asset group named "Live Tracks"
/// holding "Worship Tracks" and "Pop Tracks" is a person's filing, and
/// the top level is where a person filed it.
///
/// A file sitting loose at the shelf root has no facet, so it is in
/// [`Selection::All`] and in no facet selection — see
/// [`Selection::admits`] for why that direction and not the other.
#[must_use]
pub fn facet_of(rel_path: &str) -> Option<&str> {
    let rel = rel_path.trim_start_matches('/');
    let (head, rest) = rel.split_once('/')?;
    (!head.is_empty() && !rest.is_empty()).then_some(head)
}

/// A named, file-backed shelf: one directory of an org's tree that is
/// registered for file sync, the link graph and per-file CRDT
/// collaboration, and — where the tier allows it — published for other
/// organisations to subscribe to.
///
/// Implemented directly by [`VaultShelf`], [`WikiShelf`],
/// [`AssetShelf`] and [`ProjectShelf`]. Four implementations rather
/// than one enum with four arms, because each carries different owned
/// data (a wiki has a slug, the vault has none) and because "Vault,
/// Wiki, Assets and Projects get collaboration the same way" is a claim
/// about *implementing the same trait*, which an enum would let a
/// reader mistake for a claim about being the same thing.
///
/// The surface is small on purpose. It says only what the registration
/// loop and the subscription boundary need to ask, and every method has
/// a caller:
///
/// | method | who asks | why |
/// |---|---|---|
/// | [`Shelf::tier`] | the loop | whether to attach the wiki event bridge |
/// | [`Shelf::name`] | logs, subscriptions | the slug a person and a reference use |
/// | [`Shelf::root`] | the loop | the directory to register |
/// | [`Shelf::vault_id`] | the loop | the key on `vault::Backend` |
/// | [`Shelf::subscriber_key`] | the subscription store | who holds a subscription |
/// | [`Shelf::is_subscribable`] | the upstream resolver | whether a stranger may name it |
/// | [`Shelf::is_editable`] | the editor lanes | whether a local copy may be written |
///
/// Anything a caller wants that is not on that list is a question about
/// one tier in particular, and belongs on that tier's own type.
pub trait Shelf: Send + Sync {
    /// Which tier this shelf belongs to.
    fn tier(&self) -> Tier;

    /// The shelf's name within its tier: a wiki's slug, an asset
    /// kind's slug. The vault has one shelf and answers
    /// [`VAULT_ID`].
    fn name(&self) -> &str;

    /// The directory this shelf's files live in.
    fn root(&self) -> &Path;

    /// The id this shelf is registered under on `vault::Backend`,
    /// `GraphBackend` and `VaultCollab`.
    ///
    /// Namespaced by tier (`wiki:cooking`, `assets:songs`) so one
    /// backend can serve every shelf an org holds without two tiers
    /// ever colliding on a name — a wiki called `songs` and an asset
    /// kind called `songs` are different roots and stay different.
    fn vault_id(&self) -> String {
        match self.tier() {
            Tier::Vault => VAULT_ID.to_owned(),
            Tier::Wiki => format!("{WIKI_PREFIX}{}", self.name()),
            Tier::Assets => format!("{ASSETS_PREFIX}{}", self.name()),
            Tier::Projects => format!("{PROJECT_PREFIX}{}", self.name()),
        }
    }

    /// The key this shelf is stored under when it *holds*
    /// subscriptions, matching `wiki_proto::Subscriber::key`.
    ///
    /// Every shelf may subscribe, including the vault — the asymmetry
    /// that only some may be subscribed *to* is
    /// [`Shelf::is_subscribable`], and keeping the two as separate
    /// questions is what stopped "a vault may subscribe" from ever
    /// being read as "a vault may be subscribed to".
    fn subscriber_key(&self) -> String {
        match self.tier() {
            Tier::Vault => "vault".to_owned(),
            Tier::Wiki => format!("wiki:{}", self.name()),
            Tier::Assets => format!("assets:{}", self.name()),
            Tier::Projects => format!("project:{}", self.name()),
        }
    }

    /// Whether another organisation may subscribe to this shelf.
    /// Defaults to the tier's answer; no implementation overrides it,
    /// and one that did would be overriding a security rule.
    fn is_subscribable(&self) -> bool {
        self.tier().is_subscribable()
    }

    /// Whether a holder may write this shelf's files.
    ///
    /// True for all three tiers. It is on the trait anyway because the
    /// question is asked of a *subscribed copy* as well as of a local
    /// shelf, and there the answer differs: a subscribed Resource is
    /// read-only (`wiki.subscribe.editability`). Modelling editability
    /// as a shelf property rather than a subscription flag is what
    /// makes that a fact about the kind of thing rather than a setting
    /// somebody can get wrong per subscription.
    fn is_editable(&self) -> bool {
        true
    }
}

/// The org's own vault — `<org>/vault/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VaultShelf {
    root: PathBuf,
}

impl VaultShelf {
    #[must_use]
    pub const fn new(root: PathBuf) -> Self {
        Self { root }
    }
}

impl Shelf for VaultShelf {
    fn tier(&self) -> Tier {
        Tier::Vault
    }

    fn name(&self) -> &str {
        VAULT_ID
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

/// One named wiki — `<org>/wiki/Knowledge/` (slug
/// [`crate::DEFAULT_WIKI`]) or `<org>/wikis/<slug>/`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WikiShelf {
    slug: String,
    root: PathBuf,
}

impl WikiShelf {
    #[must_use]
    pub const fn new(slug: String, root: PathBuf) -> Self {
        Self { slug, root }
    }
}

impl Shelf for WikiShelf {
    fn tier(&self) -> Tier {
        Tier::Wiki
    }

    fn name(&self) -> &str {
        &self.slug
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

/// One asset kind's shelf — `<org>/assets/<kind>/`.
///
/// Each *kind* is its own shelf, not each asset and not the tier as a
/// whole. A kind is the unit somebody would publish (`acme.test/songs`
/// is a song library) and the unit somebody would subscribe to; the
/// tier as a whole is not something anyone means, and a single asset is
/// a file, which references already address.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetShelf {
    kind: String,
    root: PathBuf,
}

impl AssetShelf {
    #[must_use]
    pub const fn new(kind: String, root: PathBuf) -> Self {
        Self { kind, root }
    }
}

impl Shelf for AssetShelf {
    fn tier(&self) -> Tier {
        Tier::Assets
    }

    fn name(&self) -> &str {
        &self.kind
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

/// One project's tree — `<org>/projects/<slug>/`.
///
/// Each *project* is its own shelf, at any depth: an album is one, and
/// a song promoted out of that album into a subproject is another,
/// nested inside it the way a git submodule is. See [`Tier::Projects`]
/// for the model and [`crate::OrgRoot::project_shelves`] for the one
/// mechanism the collaboration layer still needs before a nested shelf
/// can be registered safely.
///
/// # The four questions, and why this type answers none of them itself
///
/// [`Shelf`] asks four things a shelf may have an opinion about. A
/// project has an opinion about all four, and in every case the
/// opinion is already the trait's default — which is worth writing
/// down, because "the default happened to fit" and "the default is
/// right here for a reason" look identical in code and are not the
/// same claim.
///
/// **`is_subscribable` — yes**, from [`Tier::is_subscribable`], with
/// no override. A project is *precisely* the unit one organisation
/// hands another: the mix engineer at another studio is given a song,
/// the colourist is given a cut. `project.location.federated` already
/// says so — *"the locations composing a project may sit on different
/// servers, owned by different orgs"* and *"cross-org collaboration is
/// expressed as grants, never by duplicating a project per org"* —
/// and a subscription is what that sentence costs at the storage
/// layer. It is also the argument for a subproject being a shelf: the
/// engineer subscribes to one song and not the other fourteen, which
/// is only sayable if the song is a thing that can be subscribed to.
///
/// **`is_editable` — yes**, from the trait default. A project tree is
/// the one shelf that is written by *tools* rather than by people
/// typing: Pro Tools, Reaper, a render. That is what a File Root is
/// for, and it is why a project tree that could not be written would
/// be a contradiction rather than a restriction. The read-only case is
/// a *subscribed copy* — somebody else's project, refreshed from
/// upstream — and that is a fact about the subscription, which
/// `wiki.subscribe.editability` already owns.
///
/// **`vault_id` — `project:<slug>`**, from the trait's composition.
/// Namespaced by tier for the same reason a wiki and an asset kind
/// are: an org may hold a wiki called `crescendum`, an asset group
/// called `crescendum` and a project called `crescendum`, and they are
/// three roots that must stay three.
///
/// **`subscriber_key` — `project:<slug>`**, likewise. A project both
/// *may be* subscribed to and *may hold* subscriptions — an album
/// citing another org's song library wants those songs to go on being
/// corrected — and keeping those two as separate questions is what
/// stops the first from being read as an answer to the second.
///
/// Four defaults and no override. If a later tier does need one, that
/// is the interesting moment: it means the trait had been describing
/// its implementations rather than abstracting over them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectShelf {
    name: String,
    root: PathBuf,
}

impl ProjectShelf {
    #[must_use]
    pub const fn new(name: String, root: PathBuf) -> Self {
        Self { name, root }
    }
}

impl Shelf for ProjectShelf {
    fn tier(&self) -> Tier {
        Tier::Projects
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn root(&self) -> &Path {
        &self.root
    }
}

/// The asset kinds an org holds by default, in the order a fresh org
/// gets them.
///
/// A *default* set rather than a closed one: [`crate::OrgRoot::assets_dir`]
/// is read from disk like [`crate::OrgRoot::named_wikis`], so an
/// application may put a shelf there that Task has never heard of and
/// it is registered, published and subscribable on the same terms as
/// these two. That is decision 2 of ADR 0004 applied to storage — the
/// primitive does not name its consumers — and it is why this is a
/// `const` list of strings and not an enum.
///
/// These two are here because Task itself writes them: the chart lane
/// and the song lane are server-side code, so their directories have to
/// exist before anybody has used them.
pub const DEFAULT_ASSET_KINDS: [&str; 2] = ["songs", "charts"];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_vault_is_never_subscribable_and_the_type_says_so() {
        assert!(!Tier::Vault.is_subscribable());
        assert!(Tier::Wiki.is_subscribable());
        assert!(
            Tier::Assets.is_subscribable(),
            "an asset shelf is published on its own terms; that is why it left the vault"
        );
        assert!(Tier::Projects.is_subscribable());
        assert!(!VaultShelf::new("/tmp/v".into()).is_subscribable());
    }

    #[test]
    fn vault_ids_are_namespaced_by_tier_so_names_cannot_collide() {
        let wiki = WikiShelf::new("songs".into(), "/tmp/w".into());
        let assets = AssetShelf::new("songs".into(), "/tmp/a".into());
        assert_eq!(wiki.vault_id(), "wiki:songs");
        assert_eq!(assets.vault_id(), "assets:songs");
        assert_ne!(
            wiki.vault_id(),
            assets.vault_id(),
            "a wiki and an asset kind sharing a name must stay different roots"
        );
        assert_eq!(VaultShelf::new("/tmp/v".into()).vault_id(), VAULT_ID);
    }

    #[test]
    fn subscriber_keys_match_the_wire_spelling() {
        assert_eq!(VaultShelf::new("/tmp/v".into()).subscriber_key(), "vault");
        assert_eq!(
            WikiShelf::new("cooking".into(), "/tmp/w".into()).subscriber_key(),
            "wiki:cooking"
        );
        assert_eq!(
            AssetShelf::new("songs".into(), "/tmp/a".into()).subscriber_key(),
            "assets:songs"
        );
        assert_eq!(
            ProjectShelf::new("crescendum".into(), "/tmp/p".into()).subscriber_key(),
            "project:crescendum"
        );
    }

    /// The claim [`Tier::Projects`] makes about the trait: a project
    /// answers all four of the questions a shelf may have an opinion
    /// about, and answers every one of them with the trait's own
    /// default. A day when this test has to change is a day the
    /// abstraction turned out to be a description.
    #[test]
    fn a_project_needed_no_method_of_its_own() {
        let p = ProjectShelf::new("crescendum".into(), "/tmp/p".into());
        assert!(
            p.is_subscribable(),
            "a project is the unit one org hands another"
        );
        assert!(p.is_editable(), "a project tree is written by its tools");
        assert_eq!(p.vault_id(), "project:crescendum");
        assert_eq!(p.subscriber_key(), "project:crescendum");
        assert_eq!(p.tier(), Tier::Projects);
        assert_eq!(p.name(), "crescendum");
    }

    /// A subscription that says nothing about depth takes the shelf it
    /// named and no more — the sub-project is a reference, not a
    /// forty-gigabyte surprise.
    #[test]
    fn depth_defaults_to_the_shelf_that_was_asked_for() {
        assert_eq!(Depth::default(), Depth::Surface);
        assert!(!Depth::default().reaches_nested());
        assert!(Depth::Deep.reaches_nested());
    }

    /// The submodule walk: an album reaches its songs, and a song
    /// reaches nothing further.
    #[test]
    fn a_deep_subscription_reaches_the_shelves_nested_in_it() {
        let nested = |s: &str| match s {
            "example-album" => vec![
                "example-album/track-two".to_owned(),
                "example-album/track-three".to_owned(),
            ],
            _ => Vec::new(),
        };
        assert_eq!(
            reachable("example-album", Depth::Surface, &nested),
            ["example-album"],
            "surface takes what was asked for and nothing else"
        );
        assert_eq!(
            reachable("example-album", Depth::Deep, &nested),
            [
                "example-album",
                "example-album/track-two",
                "example-album/track-three",
            ]
        );
    }

    /// A cycle is writable by hand, so it has to terminate here rather
    /// than be refused at write time — see [`Depth`]. Every shelf is
    /// visited once, the walk returns, and neither the stack nor the
    /// queue runs away.
    #[test]
    fn a_cycle_terminates_and_visits_each_shelf_once() {
        // a → b → c → a, plus b → a for a second way back in.
        let nested = |s: &str| match s {
            "a" => vec!["b".to_owned()],
            "b" => vec!["c".to_owned(), "a".to_owned()],
            "c" => vec!["a".to_owned()],
            _ => Vec::new(),
        };
        let walked = reachable("a", Depth::Deep, &nested);
        assert_eq!(walked, ["a", "b", "c"]);

        // The tightest cycle there is: a shelf that references itself.
        let self_ref = |_: &str| vec!["only".to_owned()];
        assert_eq!(reachable("only", Depth::Deep, &self_ref), ["only"]);
    }

    /// A project, a wiki and an asset group may all be called the same
    /// thing. Three names, three roots, and the ids keep them apart.
    #[test]
    fn a_project_cannot_collide_with_a_wiki_or_an_asset_group() {
        let ids: std::collections::HashSet<String> = [
            WikiShelf::new("crescendum".into(), "/tmp/w".into()).vault_id(),
            AssetShelf::new("crescendum".into(), "/tmp/a".into()).vault_id(),
            ProjectShelf::new("crescendum".into(), "/tmp/p".into()).vault_id(),
        ]
        .into_iter()
        .collect();
        assert_eq!(ids.len(), 3);
    }
}
