//! `task-server` — minimal vox endpoint.
//!
//! Surface after the knowledge + project-CRDT rip:
//! - `/health` — liveness probe.
//! - `/vox` — architect/vox WebSocket endpoint hosting three
//!   services: `AuthService` (architect-auth),
//!   `AttachmentService` (signed upload/download), and
//!   `VaultSyncRpc` (file replication backed by
//!   `vault::Backend`).
//! - `/blobs/*` — signed-URL endpoint for attachment uploads
//!   and downloads, mounted via `attachments::routes`.
//!
//! The previous CRDT machinery (`DocRegistry`, `OpenDoc`,
//! `WorkspaceSyncImpl`, `task-db` / `crdt-seaorm` persistence,
//! `*RepoLoro` dispatchers, capability / share-link / claim
//! services) was ripped along with the `project-proto` /
//! `project-crdt` crates. CRDT now lives only at the per-file
//! editor layer (future); vault is the sole storage path.

pub mod admin_cli;
#[cfg(feature = "plugin-agent")]
pub mod agent_router;
pub mod api_ref;
pub mod attachments;
#[cfg(feature = "plugin-scripture")]
pub mod bible_cli;
pub mod capability;
pub mod central_auth;
#[cfg(feature = "plugin-git")]
pub mod connections;
pub mod debug_profile;
#[cfg(debug_assertions)]
pub mod demo_cli;
pub mod device_sync;
pub mod example_org;
pub mod identity_mgmt;
pub mod iroh_host;
pub mod link_sync;
pub mod mcp;
pub mod media;
pub mod memberships;
pub mod notifier;
pub mod operator;
pub mod org_roots;
pub mod otlp;
pub mod permits;
pub mod presence;
pub mod server_mgmt;
pub mod share;
pub mod share_guest;
pub mod snapshot;
pub mod storage;
pub mod watch_bridge;
pub mod webdav;
#[cfg(feature = "plugin-git")]
pub mod webhooks;
#[cfg(feature = "plugin-wiki")]
pub mod wiki_repo;
#[cfg(feature = "plugin-wiki")]
pub mod wiki_tracker;
#[cfg(feature = "plugin-wiki")]
pub mod wiki_vault;

use std::path::PathBuf;
use std::sync::Arc;

use architect_auth::{
    ArchitectAuth, AuthServiceDispatcher,
    db::{AuthSeaOrmStorage, Migrator as AuthMigrator},
    transport::vox::{AuthServerMiddleware, AuthVoxService},
};
use axum::Router;
use axum::extract::State;
use axum::extract::ws::WebSocketUpgrade;
use axum::response::IntoResponse;
use axum::routing::get;
use sea_orm::Database;
use sea_orm_migration::MigratorTrait;

use crate::capability::ServerKeypair;

/// The home org's identity, which is this server's identity authority.
///
/// A principal is "a user in the home org's `auth.sqlite`, plus the orgs
/// it has membership rows for". Every other org's lane consults this
/// when a token is not one of its own (one account per server).
///
/// `None` on a server whose orgs carry no `is_home`, or before
/// `admin adopt-principal` has created the memberships store: both mean
/// "no cross-org identity", and every lane behaves exactly as it did
/// before this existed.
#[derive(Clone)]
pub struct HomeIdentity {
    pub slug: String,
    pub auth: AuthState,
    pub memberships: Arc<crate::memberships::Memberships>,
}

#[derive(Clone)]
pub struct AuthState {
    pub auth: ArchitectAuth<AuthSeaOrmStorage>,
    /// The underlying pool, kept alongside the storage wrapper so
    /// the snapshot engine can `PRAGMA wal_checkpoint(TRUNCATE)` the
    /// auth db with the rest of the org's sqlites.
    pub db: sea_orm::DatabaseConnection,
}

impl AuthState {
    pub async fn open(db_url: &str, secret: &str) -> eyre::Result<Self> {
        let db = Database::connect(db_url)
            .await
            .map_err(|e| eyre::eyre!("connect auth db `{db_url}`: {e}"))?;
        enable_wal(&db, "auth").await;
        AuthMigrator::up(&db, None)
            .await
            .map_err(|e| eyre::eyre!("auth migrations: {e}"))?;
        let storage = AuthSeaOrmStorage::new(db.clone());
        let auth = ArchitectAuth::builder()
            .secret(secret)
            .storage(storage)
            .build()
            .map_err(|e| eyre::eyre!("build ArchitectAuth: {e}"))?;
        Ok(Self { auth, db })
    }
}

/// Per-org server state. One instance per org dir scanned
/// at boot. Holds every backend the vox dispatcher mounts
/// for that org (auth, attachments, vault, wiki, agent
/// tasks, timer, finance).
///
/// Shared across orgs (the blob signing keypair, the data
/// root) lives on the parent [`AppState`].
#[derive(Clone)]
pub struct OrgAppState {
    /// Org's slug — matches the `<data_root>/orgs/<slug>/`
    /// dir and the URL prefix the vox handler routes from.
    pub slug: String,
    /// The org's directory on this server's data root — the layout
    /// authority the replica lane asks where each root appears
    /// (`org_roots::OrgPlacer`).
    pub org_root: org_proto::OrgRoot,
    /// The org's effective enabled-plugin set, resolved from
    /// `OrgManifest.disabled_plugins` at boot. [`org_layer_router`]
    /// consults it before mounting a plugin's services, the permit gate
    /// installs tables only for what mounts, and `/org/{slug}/api`
    /// reports it. No deny-list = everything on (the pre-plugin
    /// behaviour).
    pub plugins: task_plugin::PluginSet,
    /// Org-scoped architect-auth instance opened against
    /// this org's `auth.sqlite`.
    pub auth: AuthState,
    /// The org lane's permission gate: validated-session identity ×
    /// role engine × per-service permit tables. OBSERVE-ONLY by default
    /// (audits would-be denies, enforces nothing) until
    /// `TASK_ENFORCE_PERMISSIONS=1`.
    pub permissions: Arc<architect::permissions_gate::PermissionsGate>,
    /// Share links (`<org>/shares.json`) — link CRUD on the org lane +
    /// the token-checked `/org/{slug}/share/{token}` landing route.
    pub shares: Arc<share::ShareStore>,
    pub attachments: Arc<attachments::AttachmentServiceImpl>,
    /// File-replication backend rooted at this org's
    /// `vault/` dir.
    pub vault_sync: vault::Backend,
    /// Vault-file ⇄ CRDT reconciliation: the per-file doc registry
    /// (lazily opened, seeded from vault files) + write-behind into
    /// [`Self::vault_sync`] + inbound merge of external writes.
    /// Mounted as the `DocSync` service; per-file presence routes
    /// into it through [`presence::PresenceRouter`]. Docs persist
    /// under `<org>/crdt/` (override: `TASK_SERVER_CRDT_ROOT`).
    pub vault_collab: vault_collab::VaultCollab,
    /// FS watcher over the org's vault root — external disk edits
    /// (vim, Obsidian, `git pull`) broadcast the same `VaultEvent`s
    /// wire writes do, which both `subscribe` clients and the
    /// vault-collab inbound listener consume. Held for its lifetime;
    /// `None` when attaching failed (warned, non-fatal).
    pub vault_watcher: Option<Arc<vault::sync::WatcherHandle>>,
    /// Wiki feature backend rooted at this org's `vault/`.
    #[cfg(feature = "plugin-wiki")]
    pub wiki: wiki_live::WikiBackend,
    /// The wikis' second door: every wiki root registered as the vault
    /// `wiki:<slug>` (sync, graph, collab, watcher, event bridge).
    /// Held for the watchers' lifetime.
    #[cfg(feature = "plugin-wiki")]
    pub wiki_vaults: wiki_vault::WikiVaults,
    /// What this org's vault and wikis subscribe to. Keyed by
    /// subscriber rather than by wiki, because the org vault holds
    /// subscriptions too and is not a wiki.
    #[cfg(feature = "plugin-wiki")]
    pub subscriptions: wiki_live::subscriptions_backend::SubscriptionsBackend,
    /// The Edit lane over this org's wikis (`wiki.edit.*`): requests,
    /// claims, landings. Its tracker is this org's task board, so every
    /// request is an issue here too.
    #[cfg(feature = "plugin-wiki")]
    pub edits: wiki_live::edits_backend::EditsBackend,
    /// Project list / get backend — walks `vault/Projects/*.md`.
    pub projects: project::ProjectBackend,
    /// Goal list / get backend — walks `vault/Goals/**/*.md`.
    pub goals: goal::GoalBackend,
    /// Milestone backend — project-scoped checkpoints, walks
    /// `vault/Projects/<slug>/milestones/*.md`.
    pub milestones: milestone::MilestoneBackend,
    /// Workstream backend — the parent-with-swarm construct,
    /// walks `vault/Projects/<slug>/workstreams/*.md`. Also
    /// hosts the `WorkstreamService` event-stream hub.
    pub workstreams: workstream::WorkstreamBackend,
    /// Files RPC surface v1 backend (issue #259/ADR 0001) — registry +
    /// per-root jj repos live under `<org>/files/`, outside the vault
    /// (a File Root is never vault-replicated; see the glossary).
    pub files: files::FilesBackend,
    /// WebDAV compat bridge over the same roots (issue #274) — mounted
    /// at `/org/{slug}/dav`, current heads only, never the sync path.
    /// Holds this org's per-root WebDAV policy and lock managers, so it
    /// is built once per org rather than per request.
    pub files_webdav: files_webdav::WebdavBridge,
    /// This org's lane onto the Files placement layer (issue #262) — the
    /// Storage Locations it was granted, and where its roots are placed.
    /// The registry underneath is deployment-scoped and shared by every
    /// org (see [`crate::storage`]); this backend is the org-confined
    /// view of it.
    pub storage: files_storage::StorageBackend,
    /// Task backend — walks every `type: task` page in the
    /// vault.
    pub tasks: task::TaskBackend,
    /// Locations backend — `type: location` pages.
    #[cfg(feature = "plugin-home")]
    pub locations: locations::Store,
    /// Inventory backend — `type: item` gear/equipment pages.
    #[cfg(feature = "plugin-home")]
    pub inventory: inventory::Store,
    /// Scripture backend — read-only Bible spine from the resource
    /// library (`<org>/resources/bible/<TX>/`).
    #[cfg(feature = "plugin-scripture")]
    pub scripture: scripture::Store,
    /// Typed-link store — verse↔verse, note↔verse, idea↔wiki links with
    /// confidence + visibility (`<org>/links.jsonl`).
    pub links: links::Store,
    /// Ordered-collection store — song Library / Setlist / Show / Playlist,
    /// one `CollectionService` per org (`<org>/collections.jsonl`; override
    /// `TASK_SERVER_COLLECTIONS_PATH`). JSONL-backed, lexorank-ordered.
    #[cfg(feature = "plugin-fasttrackstudio")]
    pub collections: collection::Store,
    /// Resource Library reader — serves transcript sidecars under
    /// `<org>/resources/` to the watch/reader UI.
    pub resources: resources::ResourcesBackend,
    /// Cookbook (cooklang recipes under `Wiki/Cookbook/`).
    #[cfg(feature = "plugin-mealplan")]
    pub cookbook: cookbook::Store,
    /// Mealplan — scheduled meals + their fulfillment math.
    #[cfg(feature = "plugin-mealplan")]
    pub mealplan: mealplan::Store,
    /// Shopping-list service — generated/curated shopping lists.
    #[cfg(feature = "plugin-mealplan")]
    pub shopping: mealplan::shopping::Store,
    /// Substitution-rule service — ingredient alternatives.
    #[cfg(feature = "plugin-mealplan")]
    pub substitutions: mealplan::substitutions::Store,
    /// Pantry — stocked ingredients + barcode lookup.
    #[cfg(feature = "plugin-mealplan")]
    pub pantry: pantry::Store,
    /// Body metrics — weight / body-fat / measurements log.
    #[cfg(feature = "plugin-fitness")]
    pub body: body::Store,
    /// Exercise library — movement definitions referenced by
    /// routines + sessions.
    #[cfg(feature = "plugin-fitness")]
    pub exercises: exercises::Store,
    /// Workout routines + sessions (planned + completed lifts).
    #[cfg(feature = "plugin-fitness")]
    pub workouts: workouts::Store,
    /// Food intake — per-day calorie + macro log.
    #[cfg(feature = "plugin-fitness")]
    pub intake: intake::Store,
    #[cfg(feature = "plugin-agent")]
    pub agent_tasks: agent_tasks::Store,
    /// The runner registry — who can execute agent work, what they
    /// can do, and whether they are still alive.
    pub agent_runners: agent_runners::Store,
    /// Run records — every attempt at every ticket.
    pub agent_runs: agent_runners::RunStore,
    /// The grill queue — questions agents are waiting on.
    pub agent_questions: agent_runners::QuestionStore,
    /// Codex agent backend — in-process session registry + turn
    /// dispatch. Hosts the `Sessions` + `TurnDispatch` vox services
    /// that back the `/agents` UI. Cheaply clonable (Arc-backed).
    #[cfg(feature = "plugin-agent")]
    pub agent_codex: agent_codex::CodexBackend,
    /// Router over the agent backends (Codex + optional Hermes
    /// gateway) — the surface the agent vox services are served
    /// from. Sessions route to their owning backend.
    #[cfg(feature = "plugin-agent")]
    pub agent_router: agent_router::AgentRouter,
    pub agent_dispatch_vault_root: PathBuf,
    pub timer: timer::Store,
    /// Threads backend — conversations/topics anchored to any entity
    /// (`(entity_type, entity_id)`); SeaORM-backed. Mounted for the
    /// `ThreadsService` RPC surface.
    pub threads: threads::Store,
    /// Per-user preferences — default page, task-board filter
    /// defaults, last "I'm at" location; SeaORM-backed. Mounted for
    /// the `PrefsService` RPC surface.
    pub prefs: prefs::Store,
    /// Identity locker — per-user encrypted session tokens for
    /// linked remote servers. `Some` only for the **home** org
    /// (the identity anchor); `None` for every federated org.
    /// Backed by `<org>/identity.sqlite`. Mounted for the
    /// server-level `IdentityService` RPC.
    pub identity: Option<identity::Store>,
    /// Scheduling backend — day templates / availability under
    /// `vault/Projects/Scheduling/`. Mounted for `DayTemplates` so
    /// the app can overlay the daily plan on the calendar.
    #[cfg(feature = "plugin-scheduling")]
    pub scheduling: scheduling::VaultScheduler,
    /// Inbox backend — captured items under `vault/Records/inbox/`.
    /// Mounted for `Inbox` so the capture UIs + daily review can
    /// round-trip fleeting notes.
    pub inbox: inbox::VaultInbox,
    /// Notifications store — the per-org `notify.sqlite` queue behind
    /// the bell. Mounted for `Notify` (+ its stream); written by the
    /// in-process notifier (`crate::notifier`), which materializes
    /// rows from the org's event hubs.
    pub notify: notify::Store,
    /// Recall backend — spaced-repetition learning cards under
    /// `vault/Records/recall/`. Mounted for `Recall` so the deck UI +
    /// flashcard review round-trip FSRS-scheduled cards.
    #[cfg(feature = "plugin-recall")]
    pub recall: recall::VaultRecall,
    /// Contacts backend — vault-backed people directory under
    /// `vault/Records/contacts/`. Mounted for `Contacts` so the
    /// directory UI + CardDAV sync accounts round-trip.
    #[cfg(feature = "plugin-contacts")]
    pub contacts: contacts::VaultContacts,
    /// Tag registry — name → icon/color decorations at
    /// `vault/Records/tags.json`. Mounted for `TagService` so the
    /// calendar / lists decorate markdown tag names with an icon.
    pub tags: tag::VaultTags,
    #[cfg(feature = "plugin-finance")]
    pub finance_conn: sea_orm::DatabaseConnection,
    /// Invoicing backend — persists invoices in `finance.sqlite` and
    /// links billed sessions in the timer DB. Mounted for `Invoicing`.
    #[cfg(feature = "plugin-finance")]
    pub finance_backend: finance::FinanceBackend,
    /// Ledger backend — double-entry journal over the same
    /// `finance.sqlite`. Mounted for `Ledger` (post / balances /
    /// account history). The invoicing flow posts into it on
    /// mark-sent + payment.
    #[cfg(feature = "plugin-finance")]
    pub ledger_backend: finance::LedgerService,
    /// Email backend — a Maildir-backed `email_proto::EmailSync`
    /// impl rooted at `<org>/vault/Mail/`. Serves whatever
    /// accounts that tree contains (one per top-level mailbox
    /// dir); an org with no mail yet serves an empty account
    /// list, which the `/email` UI renders gracefully. Mounted
    /// for the `EmailSync` RPC surface (accounts / folders /
    /// envelopes).
    #[cfg(feature = "plugin-email")]
    pub email: email_mux::Backend,
    /// Email product layer — the staged-send outbox
    /// (`EmailProduct`: submit / approve / cancel, human-in-the-
    /// loop gate) over the same accounts. Shares the `email`
    /// backend's `EmailChange` hub so outbox events ride the one
    /// stream; its delivery poller sends through `email`'s
    /// `EmailSync::send`.
    #[cfg(feature = "plugin-email")]
    pub email_product: email_product::ProductBackend,

    /// Messages as linkable objects — "every email on this project".
    /// One sqlite table per org, keyed on Message-ID so a link
    /// survives archiving, moving and re-syncing.
    #[cfg(feature = "plugin-email")]
    pub email_links: email_link::LinkBackend,
    /// Forge backend (Forgejo) serving `RepoCatalog` +
    /// `IssueTracker` + `ReviewSurface`. Built from
    /// `TASK_FORGEJO_BASE_URL` + `TASK_FORGEJO_TOKEN`; when either
    /// is absent it's constructed with empty credentials and the
    /// forge calls degrade to auth/forge errors the UI tolerates
    /// (empty list) rather than blocking server startup.
    #[cfg(feature = "plugin-git")]
    pub forge: git_forgejo::Backend,
    /// Forge backend authenticated as the agent/bot identity
    /// (`TASK_FORGEJO_BOT_TOKEN`). The forge-sync path routes
    /// agent-owned tasks through this so their issues are
    /// attributed to the bot account, distinct from human work.
    /// Falls back to [`Self::forge`] when no bot token is set.
    #[cfg(feature = "plugin-git")]
    pub forge_agent: git_forgejo::Backend,
    /// Path to this org's `issue-links.json` (the `git_config`
    /// `FileStore` shared with the CLI). Held so the forge-sync
    /// decorator + poll loop can open it without re-deriving the
    /// org dir from the data root.
    pub issue_links_path: PathBuf,
    /// Org-wide presence channel host — the Discord-style "who's
    /// online" roster. One per org (the fan-out hub + mirror
    /// `EphemeralStore` live inside; per-connection routers share
    /// it through cheap clones). Serves `DocPresence` on the fixed
    /// [`presence::PRESENCE_DOC_ID`]; nothing is persisted —
    /// states expire on their own when a peer goes quiet.
    pub presence: crdt::sync::PresenceHost,
    /// Link-graph read service (`VaultGraph`) over the same vault
    /// root as [`Self::vault_sync`] — backlinks / links / orphans /
    /// unresolved / deadends / tags for the web vault page.
    pub vault_graph: vault::GraphBackend,
    /// Every open sqlite pool of this org (auth, agent-tasks, timer,
    /// threads, finance). The snapshot engine walks these to
    /// `PRAGMA wal_checkpoint(TRUNCATE)` under the write gate, so
    /// the committed `.sqlite` files are complete + consistent.
    pub sqlite_conns: Vec<sea_orm::DatabaseConnection>,
}

/// Top-level server state. Scans `<data_root>/orgs/` at
/// boot and builds one [`OrgAppState`] per discovered org.
/// The vox + blob handlers dispatch by slug (URL path).
///
/// The Ed25519 blob signing keypair is shared across orgs —
/// the server-side identity is one keypair per process.
#[derive(Clone)]
pub struct AppState {
    /// Ed25519 keypair used to sign blob URLs. Loaded from
    /// `<data_root>/server-key.ed25519`, generated on first
    /// boot. Tests use `ServerKeypair::generate_ephemeral()`.
    pub keypair: ServerKeypair,
    /// Slug → per-org state. Built by scanning
    /// `<data_root>/orgs/` at boot and mutated at runtime by
    /// the server-management `create_org` RPC. `RwLock` so
    /// reads on the request hot path stay parallel; writes
    /// happen only when an admin scaffolds a new org.
    ///
    /// **Deliberately `std::sync`, not `tokio::sync`.** Every
    /// accessor below is a *sync* fn that clones what it needs and
    /// drops the guard before returning, so no guard ever spans an
    /// `.await` — the case where a blocking lock is correct (and the
    /// compiler enforces it: `RwLockReadGuard` is `!Send`, so a guard
    /// held across an await in any handler or spawned task would fail
    /// to satisfy axum's / tokio's `Send` bound). A `tokio::RwLock`
    /// here would only make these accessors async and infect the sync
    /// `#[architect::rpc]` backends that call them. Callers must keep
    /// it that way: clone the `OrgAppState` (see [`AppState::org`])
    /// rather than working under the guard.
    pub orgs: Arc<std::sync::RwLock<std::collections::HashMap<String, OrgAppState>>>,
    /// The Files placement layer's coordinator (issue #262): the
    /// deployment's Storage Location registry, its grants, its
    /// placements, and the in-server Storage agent enrolled against
    /// this data root.
    ///
    /// **One per process, owned here.** It is deployment-scoped — one
    /// registry serving every org — so it belongs beside the data root
    /// it was opened against rather than in a process-global that a
    /// second `AppState` with a different data root would silently
    /// inherit (PR #284 review). `build_org_state` and
    /// `server_layer_router` both take it from here.
    pub storage: Arc<files_storage::StorageCore>,
    /// Source data root. Held for `.well-known/task-server.json`
    /// discovery, manifest re-scans, and the keypair path.
    pub data_root: org_proto::DataRoot,
    /// The home org's auth store + memberships table — this server's
    /// identity authority. `None` when no org is marked `is_home`, or
    /// when no memberships store exists yet, in which case every lane
    /// behaves exactly as it did before cross-org identity existed.
    pub home_identity: Option<HomeIdentity>,
    /// Construction scope for every backend resource (DB pools).
    /// Each org's SQLite pools register a finalizer here via
    /// architect's [`Resource::acquire_release`]; [`Scope::close`]
    /// at shutdown tears them down in LIFO order. Shared across all
    /// hosted orgs.
    pub scope: std::sync::Arc<architect::Scope>,
    /// Global write gate for snapshot cycles. Every vox request
    /// (per-org and `/server/vox`) parks at this gate on dispatch
    /// entry ([`snapshot::GatedRouter`]); a snapshot holds it
    /// closed across checkpoint + commit so the on-disk state it
    /// records is quiesced.
    pub write_gate: snapshot::WriteGate,
    /// Serializes snapshot/restore cycles (`try_lock` → `Busy`).
    pub snapshot_cycle: Arc<tokio::sync::Mutex<()>>,
    /// Last (or in-flight) async snapshot's status — polled via
    /// `GET /server/snapshot/status` after a `POST /server/snapshot?wait=0`
    /// kick-off. The synchronous trigger doesn't touch it.
    ///
    /// `std::sync` for the same reason as [`Self::orgs`]: every use
    /// is a read-clone or a field assignment inside its own block,
    /// never across an `.await` (the cycle is awaited *before* the
    /// guard is taken).
    pub snapshot_status: Arc<std::sync::RwLock<snapshot::SnapshotStatus>>,
}

impl AppState {
    /// Look up an org by slug. Convenience for routes that
    /// have extracted the slug from the URL path. Clones the
    /// matched [`OrgAppState`] (`Clone` is cheap — all fields
    /// are `Arc`/`Database` handles).
    #[must_use]
    pub fn org(&self, slug: &str) -> Option<OrgAppState> {
        self.orgs.read().ok()?.get(slug).cloned()
    }

    /// Serve an org's full [`LayerRouter`] over an **in-process**
    /// vox link (no socket, no TCP). Returns a [`LocalServer`] whose
    /// `.establish::<C>()` yields the *same* service client types the
    /// WebSocket transport produces — so a native binary (CLI, desktop)
    /// can drive the backend directly without a running `task-server`.
    /// This is architect's "inject remote vs local, one client".
    ///
    /// The acceptor task lives until `scope` is closed; keep the scope
    /// alive for as long as the clients are used, then `scope.close()`.
    /// `None` if the slug isn't hosted.
    #[must_use]
    pub fn local_server(
        &self,
        slug: &str,
        scope: &std::sync::Arc<architect::Scope>,
    ) -> Option<architect::LocalServer> {
        let org = self.org(slug)?;
        Some(architect::LocalServer::serve(
            org_layer_router(&org),
            std::sync::Arc::clone(scope),
        ))
    }

    /// Serve the **server-management** router (`/server/vox` —
    /// `OrgManagementService` + `SnapshotService`) over an in-process
    /// vox link, the server-level counterpart of [`Self::local_server`].
    /// No per-org slug: this is the transport `task org create/list`
    /// and `task admin *` speak. Embedded restores keep the process
    /// alive (`SnapshotImpl::new_without_exit`) — the CLI is ephemeral
    /// and exits after the verb anyway.
    #[must_use]
    pub fn server_local_server(
        &self,
        scope: &std::sync::Arc<architect::Scope>,
    ) -> architect::LocalServer {
        architect::LocalServer::serve(
            server_layer_router(self, true),
            std::sync::Arc::clone(scope),
        )
    }

    /// Slugs of every hosted org, sorted for deterministic
    /// `.well-known` output.
    #[must_use]
    pub fn org_slugs(&self) -> Vec<String> {
        let guard = match self.orgs.read() {
            Ok(g) => g,
            Err(_) => return Vec::new(),
        };
        let mut slugs: Vec<String> = guard.keys().cloned().collect();
        slugs.sort_unstable();
        slugs
    }

    /// True when the server has no hosted orgs. Used by the
    /// server-management RPC to decide whether to accept an
    /// unauthenticated bootstrap `create_org`.
    #[must_use]
    pub fn is_bootstrap(&self) -> bool {
        self.orgs.read().is_ok_and(|g| g.is_empty())
    }

    /// Slug of the home org, if exactly one is hosted. Used to
    /// gate `create_org` after bootstrap — only home-org users
    /// can mint new federated orgs.
    #[must_use]
    pub fn home_slug(&self) -> Option<String> {
        // Snapshot the slugs and drop the guard before touching the
        // disk — the manifest reads below must not run under the
        // registry lock.
        let slugs = self.org_slugs();
        for slug in slugs {
            // `is_home` lives in the manifest, not the runtime
            // state — re-read from disk.
            if let Ok(manifest) = self.data_root.org(slug.as_str()).manifest() {
                if manifest.is_home {
                    return Some(slug);
                }
            }
        }
        None
    }

    /// Hot-add a freshly scaffolded org to the live dispatcher.
    /// The server-management RPC calls this after writing the
    /// org's dir + initializing its DBs.
    pub fn insert_org(&self, slug: String, state: OrgAppState) -> Result<(), &'static str> {
        self.orgs
            .write()
            .map_err(|_| "orgs lock poisoned")?
            .insert(slug, state);
        Ok(())
    }
}

impl AppState {
    /// Boot path: scan `<data_root>/orgs/` and build one
    /// [`OrgAppState`] per discovered org. Hosts all of them
    /// at `/org/<slug>/...`. If `slug_filter` is `Some`,
    /// only that one org is hosted (matches the
    /// single-org-process pattern earlier PRs used).
    ///
    /// When no orgs are present the server boots empty — the
    /// `/server/vox` `OrgManagementService` accepts an
    /// unauthenticated `create_org` in that state so the CLI
    /// can bootstrap the first org without touching the
    /// server's filesystem.
    pub async fn new(slug_filter: Option<&str>) -> eyre::Result<Self> {
        let data_root =
            org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
        data_root
            .ensure()
            .map_err(|e| eyre::eyre!("ensure data root: {e}"))?;
        let keypair = ServerKeypair::load_or_generate(&data_root.server_keypair_path())
            .map_err(|e| eyre::eyre!("load server keypair: {e}"))?;

        let scope = architect::Scope::new();
        // The deployment's storage coordinator, opened once against the
        // data root we just ensured. Fatal on failure: a registry that
        // cannot be opened means placement is broken for every org, and
        // half-mounting it was the policy split the review flagged.
        let storage = crate::storage::open(data_root.path())?;
        let org_roots = pick_server_orgs(&data_root, slug_filter)?;

        // The home org's identity, opened BEFORE the org loop so every
        // lane can be built with it. A second connection to the same
        // sqlite file (WAL, like the admin verbs) rather than plumbing
        // the loop's own AuthState out of it — the home org is not
        // guaranteed to be built first, and ordering the loop by
        // is_home to arrange that would be a subtle trap for whoever
        // next touches it.
        let home_identity = build_home_identity(&org_roots).await;
        if let Some(home) = &home_identity {
            tracing::info!(
                home.slug = home.slug,
                "cross-org identity: home org is this server's identity authority"
            );
        }

        let mut orgs = std::collections::HashMap::new();
        for org_root in org_roots {
            let slug = org_root.slug().to_owned();
            let auth_db_url = format!("sqlite://{}?mode=rwc", org_root.auth_db().display());
            let auth = AuthState::open(&auth_db_url, &auth_secret()).await?;
            let org_state = build_org_state(
                auth,
                &keypair,
                org_root,
                &scope,
                &storage,
                home_identity.as_ref(),
            )
            .await?;
            orgs.insert(slug, org_state);
        }

        let state = Self {
            keypair,
            orgs: Arc::new(std::sync::RwLock::new(orgs)),
            storage,
            data_root,
            home_identity,
            scope,
            write_gate: snapshot::WriteGate::new(),
            snapshot_cycle: Arc::new(tokio::sync::Mutex::new(())),
            snapshot_status: Arc::new(std::sync::RwLock::new(snapshot::SnapshotStatus::default())),
        };
        // Per-org notifier: subscribes to the org's event hubs
        // in-process and materializes notifications by rule
        // (see `notifier`'s module docs for the catalog).
        notifier::spawn(&state);
        Ok(state)
    }

    /// Test helper. Build a one-org `AppState` from an
    /// explicit auth + keypair (e.g. in-memory `AuthState`
    /// plus ephemeral keypair). Picks the org root the same
    /// way [`Self::new`] does.
    pub async fn new_with_auth(auth: AuthState, keypair: ServerKeypair) -> eyre::Result<Self> {
        let data_root =
            org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
        data_root
            .ensure()
            .map_err(|e| eyre::eyre!("ensure data root: {e}"))?;
        let scope = architect::Scope::new();
        let storage = crate::storage::open(data_root.path())?;
        let mut org_roots = pick_server_orgs(&data_root, None)?;
        let org_root = org_roots
            .pop()
            .ok_or_else(|| eyre::eyre!("no org to host"))?;
        let slug = org_root.slug().to_owned();
        let org_state = build_org_state(auth, &keypair, org_root, &scope, &storage, None).await?;
        let mut orgs = std::collections::HashMap::new();
        orgs.insert(slug, org_state);
        Ok(Self {
            keypair,
            orgs: Arc::new(std::sync::RwLock::new(orgs)),
            storage,
            data_root,
            // Test helpers host a single org: no cross-org identity.
            home_identity: None,
            scope,
            write_gate: snapshot::WriteGate::new(),
            snapshot_cycle: Arc::new(tokio::sync::Mutex::new(())),
            snapshot_status: Arc::new(std::sync::RwLock::new(snapshot::SnapshotStatus::default())),
        })
    }

    /// Test helper: same as `new_with_auth` but takes an
    /// explicit [`OrgRoot`] (tempdir-backed in tests) instead
    /// of scanning the data root.
    pub async fn new_with_auth_and_org(
        auth: AuthState,
        keypair: ServerKeypair,
        org_root: org_proto::OrgRoot,
    ) -> eyre::Result<Self> {
        let data_root =
            org_proto::DataRoot::from_env().map_err(|e| eyre::eyre!("data root: {e}"))?;
        let scope = architect::Scope::new();
        let storage = crate::storage::open(data_root.path())?;
        let slug = org_root.slug().to_owned();
        let org_state = build_org_state(auth, &keypair, org_root, &scope, &storage, None).await?;
        let mut orgs = std::collections::HashMap::new();
        orgs.insert(slug, org_state);
        Ok(Self {
            keypair,
            orgs: Arc::new(std::sync::RwLock::new(orgs)),
            storage,
            data_root,
            // Test helpers host a single org: no cross-org identity.
            home_identity: None,
            scope,
            write_gate: snapshot::WriteGate::new(),
            snapshot_cycle: Arc::new(tokio::sync::Mutex::new(())),
            snapshot_status: Arc::new(std::sync::RwLock::new(snapshot::SnapshotStatus::default())),
        })
    }
}

/// Put a sqlite pool into WAL journal mode. WAL is what makes the
/// server-native snapshot story work: writers append to the `-wal`
/// sidecar (excluded from snapshots) while the main `.sqlite` file
/// stays stable + consistent on disk, so `PRAGMA wal_checkpoint
/// (TRUNCATE)` followed by `git add` captures a complete database.
/// The mode is persistent (recorded in the db file), so script-era
/// DELETE-mode databases are upgraded on first boot. Best-effort:
/// sqlx leaves the journal mode untouched by default, and a failure
/// here only degrades snapshot consistency back to the old
/// best-effort behavior.
async fn enable_wal(db: &sea_orm::DatabaseConnection, label: &str) {
    use sea_orm::ConnectionTrait as _;
    if let Err(e) = db.execute_unprepared("PRAGMA journal_mode=WAL;").await {
        tracing::warn!(db = label, error = %e, "could not enable WAL journal mode");
    }
}

/// Open a migrated SQLite pool as an architect [`Resource`] tied to
/// `scope`: connect, run `migrate`, and register a finalizer that
/// closes the pool. On [`Scope::close`] (graceful shutdown) every pool
/// opened this way is torn down in LIFO order instead of relying on
/// `Drop`. `migrate` receives the fresh connection and returns it after
/// running its migrator.
async fn open_sqlite_pool<F>(
    scope: &std::sync::Arc<architect::Scope>,
    url: String,
    label: &'static str,
    migrate: F,
) -> eyre::Result<sea_orm::DatabaseConnection>
where
    F: FnOnce(
            sea_orm::DatabaseConnection,
        ) -> std::pin::Pin<
            Box<
                dyn std::future::Future<
                        Output = Result<sea_orm::DatabaseConnection, sea_orm::DbErr>,
                    > + Send,
            >,
        > + Send
        + 'static,
{
    architect::Resource::acquire_release(
        architect::Resource::from_fn(move |_| async move {
            let db = Database::connect(&url)
                .await
                .map_err(|e| eyre::eyre!("connect {label} db `{url}`: {e}"))?;
            enable_wal(&db, label).await;
            let db = migrate(db)
                .await
                .map_err(|e| eyre::eyre!("{label} migrations: {e}"))?;
            Ok(db)
        }),
        |db: sea_orm::DatabaseConnection| async move {
            if let Err(e) = db.close().await {
                tracing::warn!(error = %e, "closing sqlite pool");
            }
        },
    )
    .build(scope)
    .await
}

/// Build one [`OrgAppState`] for a single org's
/// [`OrgRoot`]. Opens every backend the vox dispatcher
/// will mount.
/// Open the home org's auth store + memberships table, if this server
/// has both.
///
/// Every failure here degrades to `None` — no `is_home` org, no
/// memberships file yet, or a store that will not open — because the
/// fallback it powers is an ADDITION to per-org auth, never a
/// replacement. A server that cannot answer "which orgs does this
/// principal belong to" must still serve every org exactly as it did
/// before, rather than refuse to boot.
async fn build_home_identity(org_roots: &[org_proto::OrgRoot]) -> Option<HomeIdentity> {
    let home = org_roots
        .iter()
        .find(|r| r.manifest().is_ok_and(|m| m.is_home))?;
    let db = home.memberships_db();
    if !db.exists() {
        tracing::debug!(
            path = %db.display(),
            "no memberships store — cross-org identity off (run `admin adopt-principal`)"
        );
        return None;
    }
    let memberships = match crate::memberships::Memberships::open(&db).await {
        Ok(m) => Arc::new(m),
        Err(e) => {
            tracing::warn!(error = %e, "memberships store failed to open — cross-org identity off");
            return None;
        }
    };
    let url = format!("sqlite://{}?mode=rwc", home.auth_db().display());
    let auth = match AuthState::open(&url, &auth_secret()).await {
        Ok(a) => a,
        Err(e) => {
            tracing::warn!(error = %e, "home auth store failed to open — cross-org identity off");
            return None;
        }
    };
    Some(HomeIdentity {
        slug: home.slug().to_owned(),
        auth,
        memberships,
    })
}

pub(crate) async fn build_org_state(
    auth: AuthState,
    keypair: &ServerKeypair,
    org_root: org_proto::OrgRoot,
    scope: &std::sync::Arc<architect::Scope>,
    storage: &Arc<files_storage::StorageCore>,
    home_identity: Option<&HomeIdentity>,
) -> eyre::Result<OrgAppState> {
    {
        // The org's effective plugin set, resolved once from the
        // manifest's deny-list. A missing/unreadable manifest resolves
        // to "everything on" — the safe default and the pre-plugin
        // behaviour (`PluginSet::resolve(None)`); unknown ids in the
        // deny-list are warned about and ignored by `resolve`.
        let plugins = task_plugin::PluginSet::resolve(
            org_root
                .manifest()
                .ok()
                .map(|m| task_plugin::PluginChoice::Disabled(m.disabled_plugins.0.clone()))
                .as_ref(),
        );

        // Attachments — local blob store under the standard XDG
        // path; the keypair signs upload/download URLs.
        let blob_root =
            attachments::default_blob_root().map_err(|e| eyre::eyre!("blob root: {e}"))?;
        let object_store: Arc<dyn attachments::ObjectStore> =
            Arc::new(attachments::LocalFsStore::new(blob_root));
        let public_base_url = std::env::var("TASK_SERVER_PUBLIC_URL").unwrap_or_default();
        let attachment_service = Arc::new(attachments::AttachmentServiceImpl::new(
            keypair.clone(),
            object_store,
            public_base_url,
        ));

        // Vault file-replication. Org-scoped: each org's
        // vault lives under `<data_root>/orgs/<slug>/vault/`.
        // `TASK_SERVER_VAULT_ROOT` still wins as a hard
        // override (for tests / containers that want a
        // flat parent dir).
        let vault_root = std::env::var("TASK_SERVER_VAULT_ROOT")
            .map_or_else(|_| org_root.vault_dir(), PathBuf::from);
        // Every wiki this org holds, not one. `LLM/` scratch stays a
        // sibling subtree the wiki backend doesn't touch — agents
        // read/write it through plain filesystem ops.
        //
        // t[impl wiki.many.addressable] — one mount serves the whole
        // set, so every wiki is reachable by its own slug across the
        // service surface, and there is no method that works only on a
        // default wiki. t[impl wiki.many.isolation] — the backend
        // resolves each call's `wiki_id` to its own root, so an
        // operation on one wiki cannot read or write another's state.
        //
        // `TASK_SERVER_WIKI_ROOT` still pins a single root when it is
        // set: it exists so a test can point the wiki somewhere
        // disposable, and it names that root `knowledge` so the
        // override is a member of the set rather than a fourth shape.
        //
        // Computed before the vault backend because the same map feeds
        // it: each wiki root is also a vault root (`wiki:<slug>`), which
        // is what lets the vault editor open a wiki page.
        #[cfg(any(feature = "plugin-wiki", feature = "plugin-mealplan"))]
        let wiki_root = std::env::var("TASK_SERVER_WIKI_ROOT")
            .map_or_else(|_| org_root.wiki_knowledge_dir(), PathBuf::from);
        #[cfg(feature = "plugin-wiki")]
        let wiki_roots: std::collections::HashMap<String, PathBuf> = {
            let mut roots = std::collections::HashMap::new();
            if std::env::var_os("TASK_SERVER_WIKI_ROOT").is_some() {
                roots.insert(org_proto::DEFAULT_WIKI.to_string(), wiki_root.clone());
            } else {
                roots.extend(org_root.named_wikis());
                // A fresh org has no wiki directory yet. Keep the
                // default in the map anyway, so bootstrapping one is a
                // write rather than a `WikiNotFound`.
                roots
                    .entry(org_proto::DEFAULT_WIKI.to_string())
                    .or_insert_with(|| org_root.wiki_knowledge_dir());
            }
            roots
        };
        // `"default"` → the org's vault root *directly* — one vault per
        // org. Earlier we used `under_parent`, which routed writes
        // into `vault_root/default/…` — and every `ProjectBackend` /
        // `GoalBackend` scan then saw each file twice (once at the
        // real path, once under the ghost `default/` subdir). Beside
        // it, `wiki:<slug>` → that wiki's root for every wiki the org
        // holds (`wiki_vault`), registered below once the wiki backend
        // exists — the same explicit layout, so an unknown id is still
        // `NotFound` and a page path is still guarded the same way.
        std::fs::create_dir_all(&vault_root).map_err(|e| eyre::eyre!("vault backend: {e}"))?;
        let vault_sync_state = vault::Backend::single("default", vault_root.clone())
            .map_err(|e| eyre::eyre!("vault backend: {e}"))?;
        // Link-graph reader over the same roots the sync backend
        // serves — read-only, so no dir creation. Wiki roots join it
        // in the same registration as the sync backend.
        let vault_graph = vault::GraphBackend::single("default", vault_root.clone());
        // Recipes are `.cook` files under the wiki root, outside the
        // vault this backend serves — so a `.base` filtering
        // `type: recipe` matches nothing unless the backend is told
        // where the cookbook lives. Read-only, `base_views` only; the
        // `cookbook` service still owns every read and write.
        #[cfg(feature = "plugin-mealplan")]
        let vault_sync_state = vault_sync_state.with_recipe_roots(
            [(
                "default".to_string(),
                std::env::var("TASK_SERVER_WIKI_ROOT")
                    .map_or_else(|_| org_root.wiki_knowledge_dir(), PathBuf::from),
            )]
            .into_iter()
            .collect(),
        );
        // Per-file CRDT collaboration over the same backend. Doc
        // persistence (snapshot + update log, one dir per doc id)
        // lives at `<org>/crdt/` — file-per-doc fits the plain-text
        // ethos; `crdt-seaorm` is the drop-in alternative if the org
        // dirs ever move into a database. The inbound listener folds
        // every `VaultEvent::Put` (non-CRDT `put_file` callers AND
        // the watcher below) into whichever per-file docs are open.
        let crdt_root = std::env::var("TASK_SERVER_CRDT_ROOT")
            .map_or_else(|_| org_root.path().join("crdt"), PathBuf::from);
        let vault_collab = vault_collab::VaultCollab::new(vault_sync_state.clone(), crdt_root);
        vault_collab.watch_vault("default");
        // External disk edits (vim, Obsidian, git) → VaultEvents.
        // Best-effort: a vault on a filesystem without notify support
        // still serves wire traffic, just without live disk pickup.
        let vault_watcher = match vault_sync_state.start_watcher("default").await {
            Ok(handle) => Some(Arc::new(handle)),
            Err(e) => {
                tracing::warn!(org = %org_root.slug(), "vault watcher not attached: {e}");
                None
            }
        };
        #[cfg(feature = "plugin-wiki")]
        let (wiki, wiki_vaults) = {
            let mut roots = wiki_roots.clone();
            // Compatibility alias. Every client predating multi-wiki
            // asks for `"default"`, and renaming the tier to
            // `knowledge` turned those into `WikiNotFound`. The alias
            // points at the same directory and is excluded from
            // `list_wikis`, so it resolves without appearing as a
            // second wiki.
            if let Some(knowledge) = roots.get(org_proto::DEFAULT_WIKI).cloned() {
                roots.insert(wiki_live::backend::COMPAT_WIKI_ID.to_string(), knowledge);
            }
            tracing::info!(
                org = %org_root.slug(),
                wikis = roots.len(),
                "wiki backend serving {}",
                {
                    let mut names: Vec<&str> = roots.keys().map(String::as_str).collect();
                    names.sort_unstable();
                    names.join(", ")
                }
            );
            // Created into `<org>/wikis/` at runtime (`wiki.many.set`),
            // so the map the backend holds is the set at boot plus
            // whatever `create_wiki` adds while the server runs.
            let backend =
                wiki_live::WikiBackend::with_roots_under(roots.clone(), org_root.wikis_dir())
                    // A repo-sourced wiki's working copy is pushed as
                    // the Editor's own forge identity and becomes a
                    // pull request (`wiki.source.editable`); the
                    // server holds the forge clients.
                    .with_lander(std::sync::Arc::new(crate::wiki_repo::ForgeLander));
            // Each wiki root is a vault root too (`wiki:<slug>`): the
            // editor's sync / collab / graph / live-changes path over
            // the same files the `Pages` service writes. Registered
            // now for every wiki at boot, and again by `create_wiki`
            // for one made while the server runs. The `default` alias
            // is skipped — it is the `knowledge` tier under another
            // name, and one vault id per directory is enough.
            let wiki_vaults = wiki_vault::WikiVaults::new(
                org_root.slug(),
                vault_sync_state.clone(),
                vault_graph.clone(),
                vault_collab.clone(),
                backend.clone(),
            );
            let backend = backend
                .with_on_created(wiki_vaults.created_hook(tokio::runtime::Handle::current()));
            for (slug, root) in &wiki_roots {
                wiki_vaults.attach(slug, root).await;
            }
            // Hand this org's vault and each of its wikis the core
            // set. Doing it at boot rather than at org creation is
            // what makes `wiki.core.retroactive` true: an org planted
            // before a source became core picks it up on next start,
            // and one that declined keeps its decline.
            let subs = wiki_live::subscriptions::SubscriptionStore::open(org_root.path());
            let core = core_subscriptions();
            let mut subscribers = vec![wiki_proto::Subscriber::Vault];
            subscribers.extend(
                roots
                    .keys()
                    .map(|slug| wiki_proto::Subscriber::Wiki(slug.clone())),
            );
            for subscriber in subscribers {
                match subs.ensure_core(&subscriber, &core) {
                    Ok(added) if !added.is_empty() => tracing::info!(
                        org = %org_root.slug(),
                        subscriber = ?subscriber,
                        "core subscriptions added: {}",
                        added.join(", ")
                    ),
                    Ok(_) => {}
                    // Never fatal: an org that cannot hold its
                    // subscriptions should still serve its own wikis.
                    Err(e) => tracing::warn!(
                        org = %org_root.slug(),
                        "core subscriptions not applied: {e}"
                    ),
                }
            }
            (backend, wiki_vaults)
        };

        // The subscription service, over the same store the boot sweep
        // just topped up. `LocalOrgs` resolves a source published by
        // another org on this data root; a peer's is the same
        // materialize call against a vox client, which is why the
        // resolver is a trait rather than a match.
        #[cfg(feature = "plugin-wiki")]
        let subscriptions = {
            let orgs_dir = org_root
                .path()
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_else(|| org_root.path().to_path_buf());
            let domains = wiki_domains(&orgs_dir, std::env::var("TASK_WIKI_DOMAINS").ok());
            let upstream = std::sync::Arc::new(wiki_live::subscriptions_backend::LocalOrgs::new(
                orgs_dir.parent().unwrap_or(org_root.path()).to_path_buf(),
                domains,
            ));
            wiki_live::subscriptions_backend::SubscriptionsBackend::new(
                org_root.path().to_path_buf(),
                core_subscriptions(),
                upstream,
            )
        };

        // Agent-task queue. SQLite under the org root
        // (override via `TASK_SERVER_AGENT_TASKS_URL`).
        // `OrgRoot` doesn't yet have an `agent_tasks_db()`
        // helper — we co-locate it alongside the other org
        // dbs by hand for now. PR 4 promotes this to a
        // first-class resolver.
        #[cfg(feature = "plugin-agent")]
        let agent_tasks_url = std::env::var("TASK_SERVER_AGENT_TASKS_URL").unwrap_or_else(|_| {
            format!(
                "sqlite://{}?mode=rwc",
                org_root.path().join("agent-tasks.sqlite").display()
            )
        });
        #[cfg(feature = "plugin-agent")]
        let agent_tasks_conn = open_sqlite_pool(scope, agent_tasks_url, "agent-tasks", |db| {
            Box::pin(async move { agent_tasks::Migrator::up(&db, None).await.map(|()| db) })
        })
        .await?;
        #[cfg(feature = "plugin-agent")]
        let agent_tasks = agent_tasks::Store::new(agent_tasks_conn);

        // Runner registry. Its OWN sqlite file, like every other
        // slice — two SeaORM migrators sharing one database share
        // one `seaql_migrations` table, and the second one silently
        // applies nothing. There is a regression test for that in
        // `agent-runners`; do not co-locate this with agent-tasks.
        #[cfg(feature = "plugin-agent")]
        let agent_runners_url =
            std::env::var("TASK_SERVER_AGENT_RUNNERS_URL").unwrap_or_else(|_| {
                format!(
                    "sqlite://{}?mode=rwc",
                    org_root.path().join("agent-runners.sqlite").display()
                )
            });
        #[cfg(feature = "plugin-agent")]
        let agent_runners_conn =
            open_sqlite_pool(scope, agent_runners_url, "agent-runners", |db| {
                Box::pin(async move { agent_runners::Migrator::up(&db, None).await.map(|()| db) })
            })
            .await?;
        #[cfg(feature = "plugin-agent")]
        let agent_runs = agent_runners::RunStore::new(agent_runners_conn.clone());
        #[cfg(feature = "plugin-agent")]
        let agent_questions = agent_runners::QuestionStore::new(agent_runners_conn.clone());
        #[cfg(feature = "plugin-agent")]
        let agent_runners = agent_runners::Store::new(agent_runners_conn);

        // Codex agent backend. In-process, in-memory session
        // registry + turn dispatch — hosts the `Sessions` +
        // `TurnDispatch` vox services behind the `/agents` UI.
        // One event hub across both agent backends: `Subscriptions`
        // is a single `#[subscribe]` stream served from one
        // `PubSub`, so Codex and Hermes publish into the same hub
        // and a client's one subscription covers sessions on either.
        #[cfg(feature = "plugin-agent")]
        let agent_events = architect::PubSub::sliding(512);
        #[cfg(feature = "plugin-agent")]
        let agent_codex = agent_codex::CodexBackend::with_events(agent_events.clone());
        // Hermes gateway backend — enabled when TASK_HERMES_URL is
        // set (see agent_hermes::HermesConfig::from_env). When
        // present it becomes the DEFAULT chat backend: sessions
        // created without an explicit backend_id land on Hermes.
        #[cfg(feature = "plugin-agent")]
        let agent_hermes = agent_hermes::HermesBackend::from_env_with_events(agent_events.clone());
        #[cfg(feature = "plugin-agent")]
        if let Some(h) = &agent_hermes {
            tracing::info!(url = %h.config().base_url, model = %h.config().model, "hermes agent gateway configured");
        }
        #[cfg(feature = "plugin-agent")]
        let agent_router =
            agent_router::AgentRouter::new(agent_codex.clone(), agent_hermes, agent_events);

        // Timer store. SQLite at
        // `<data_root>/orgs/<slug>/timer.sqlite`
        // (override via `TASK_SERVER_TIMER_URL`). Project
        // defaults are resolved off the same vault root the
        // rest of the server uses — the rate cascade calls
        // `VaultProjectDefaults::lookup` to read each
        // session's project markdown on close.
        let timer_url = std::env::var("TASK_SERVER_TIMER_URL")
            .unwrap_or_else(|_| format!("sqlite://{}?mode=rwc", org_root.timer_db().display()));
        let timer_conn = open_sqlite_pool(scope, timer_url, "timer", |db| {
            Box::pin(async move { timer::Migrator::up(&db, None).await.map(|()| db) })
        })
        .await?;
        let timer_defaults = std::sync::Arc::new(timer::store::VaultProjectDefaults {
            vault_root: vault_root.clone(),
        });
        let timer = timer::Store::new(timer_conn, timer_defaults);

        // Threads — conversations/topics anchored to tasks/projects.
        // SeaORM-backed (DB swappable); migrations run on open. Override
        // via `TASK_SERVER_THREADS_URL`.
        let threads_url = std::env::var("TASK_SERVER_THREADS_URL")
            .unwrap_or_else(|_| format!("sqlite://{}?mode=rwc", org_root.threads_db().display()));
        let threads_conn = open_sqlite_pool(scope, threads_url, "threads", |db| {
            Box::pin(async move { threads::Migrator::up(&db, None).await.map(|()| db) })
        })
        .await?;
        let threads = threads::Store::new(threads_conn);

        // Notifications store. SQLite at
        // `<data_root>/orgs/<slug>/notify.sqlite` (override via
        // `TASK_SERVER_NOTIFY_URL`); migrations run on open. Fed by
        // the notifier (`crate::notifier`), served as `Notify`.
        let notify_url = std::env::var("TASK_SERVER_NOTIFY_URL").unwrap_or_else(|_| {
            format!(
                "sqlite://{}?mode=rwc",
                org_root.path().join("notify.sqlite").display()
            )
        });
        let notify_conn = open_sqlite_pool(scope, notify_url, "notify", |db| {
            Box::pin(async move { notify::Migrator::up(&db, None).await.map(|()| db) })
        })
        .await?;
        let notify_store = notify::Store::new(notify_conn);

        // Per-user preferences. SQLite at
        // `<data_root>/orgs/<slug>/prefs.sqlite` (override via
        // `TASK_SERVER_PREFS_URL`); migrations run on open.
        let prefs_url = std::env::var("TASK_SERVER_PREFS_URL")
            .unwrap_or_else(|_| format!("sqlite://{}?mode=rwc", org_root.prefs_db().display()));
        let prefs_conn = open_sqlite_pool(scope, prefs_url, "prefs", |db| {
            Box::pin(async move { prefs::Migrator::up(&db, None).await.map(|()| db) })
        })
        .await?;
        let prefs = prefs::Store::new(prefs_conn);

        // Identity locker — only the **home** org anchors it. Opened
        // at `<org>/identity.sqlite` (per `OrgRoot::identity_db`);
        // `is_home` comes from the on-disk manifest (same source
        // `AppState::home_slug` reads). Tokens are (de)crypted with the
        // shared AEAD secret. Federated orgs get `None`.
        let is_home = org_root.manifest().map(|m| m.is_home).unwrap_or(false);
        let identity = if is_home {
            let identity_url = format!("sqlite://{}?mode=rwc", org_root.identity_db().display());
            let identity_conn = open_sqlite_pool(scope, identity_url, "identity", |db| {
                Box::pin(async move { identity::Migrator::up(&db, None).await.map(|()| db) })
            })
            .await?;
            Some(identity::Store::new(identity_conn, auth_secret()))
        } else {
            None
        };

        // Scheduling backend rooted at the same vault. Day templates
        // live under `Projects/Scheduling/templates/`, bookings under
        // `Records/bookings/`, and the booking audit trail under
        // `Records/audit/` — every byte it owns is on disk, so a
        // restart loses nothing.
        #[cfg(feature = "plugin-scheduling")]
        let scheduling = scheduling::VaultScheduler::new(vault_root.clone())
            .map_err(|e| eyre::eyre!("scheduling backend: {e}"))?;

        // Inbox backend rooted at the same vault — captured items
        // live under `Records/inbox/`.
        let inbox = inbox::VaultInbox::new(vault_root.clone())
            .map_err(|e| eyre::eyre!("inbox backend: {e}"))?;

        // Recall backend rooted at the same vault — learning cards
        // live under `Records/recall/`.
        #[cfg(feature = "plugin-recall")]
        let recall = recall::VaultRecall::new(vault_root.clone())
            .map_err(|e| eyre::eyre!("recall backend: {e}"))?;

        // Contacts backend rooted at the same vault — people live
        // under `Records/contacts/`.
        #[cfg(feature = "plugin-contacts")]
        let contacts = contacts::VaultContacts::new(vault_root.clone())
            .map_err(|e| eyre::eyre!("contacts backend: {e}"))?;

        // Tag registry rooted at the same vault — `Records/tags.json`.
        let tags =
            tag::VaultTags::new(vault_root.clone()).map_err(|e| eyre::eyre!("tag backend: {e}"))?;

        // Finance store. SQLite at
        // `<data_root>/orgs/<slug>/finance.sqlite`
        // (override via `TASK_SERVER_FINANCE_URL`). Services
        // (Invoicing / Ledger) are not mounted yet — only
        // the migrated DB connection is exposed; the
        // task-cli `finance invoice` flow writes against it
        // when that feature lands.
        #[cfg(feature = "plugin-finance")]
        let finance_url = std::env::var("TASK_SERVER_FINANCE_URL")
            .unwrap_or_else(|_| format!("sqlite://{}?mode=rwc", org_root.finance_db().display()));
        #[cfg(feature = "plugin-finance")]
        let finance_conn = open_sqlite_pool(scope, finance_url, "finance", |db| {
            Box::pin(async move { finance_db::Migrator::up(&db, None).await.map(|()| db) })
        })
        .await?;

        // Ledger service — double-entry journal over the same
        // finance.sqlite connection. Shared with the invoicing
        // backend so mark-sent / payment post into it.
        #[cfg(feature = "plugin-finance")]
        let ledger_backend = finance::LedgerService::new(finance_conn.clone())
            .map_err(|e| eyre::eyre!("ledger backend: {e}"))?;

        // Invoicing service — persists invoices in finance.sqlite and
        // marks billed sessions in the timer DB, so it needs both.
        // It also posts double-entry journal entries to the ledger on
        // mark-sent + payment, so it gets a clone of the ledger.
        #[cfg(feature = "plugin-finance")]
        let finance_backend = finance::FinanceBackend::new(
            finance_conn.clone(),
            timer.conn().clone(),
            org_root
                .manifest()
                .map_or_else(|_| "Business".into(), |m| m.display_name),
            ledger_backend.clone(),
        )
        .map_err(|e| eyre::eyre!("finance backend: {e}"))?;

        // Email backend — Maildir-backed `EmailSync`. The mail
        // root lives at `<org>/vault/Mail/` (override via
        // `TASK_SERVER_MAIL_ROOT`); each top-level subdir there
        // is one account (its dir name is the account id). An
        // account dir may carry an `account.json`
        // (`email_config::AccountConfig`) supplying the real
        // address, folder aliases, and — for `Maildir` backends —
        // an optional SMTP `submit` endpoint that makes
        // `EmailSync::send` work end to end. An org with no
        // `Mail/` tree just serves an empty account list — the
        // `/email` UI tolerates that.
        #[cfg(feature = "plugin-email")]
        let mail_root = std::env::var("TASK_SERVER_MAIL_ROOT")
            .map_or_else(|_| vault_root.join("Mail"), PathBuf::from);
        #[cfg(feature = "plugin-email")]
        let (mail_accounts, mail_configs) = discover_mail_accounts(&mail_root);
        // Every account gets a product store, remote ones included.
        //
        // The store is a sqlite file in the account's directory — which
        // exists for an IMAP account too, since that is where its
        // `account.json` lives. It backs the triage pass, the
        // derivation cache, and the alert-once notification state, so
        // excluding remote accounts here would mean a Gmail mailbox
        // silently never raises a new-mail notification.
        #[cfg(feature = "plugin-email")]
        let product_accounts: Vec<email_product::ProductAccount> = mail_accounts
            .iter()
            .map(|e| email_product::ProductAccount {
                id: e.account.id.0.clone(),
                root: e.root.clone(),
                address: e.account.address.clone(),
            })
            .chain(mail_configs.iter().filter_map(|c| {
                matches!(c.backend, email_config::BackendKind::Maildir { .. })
                    .then(|| None)
                    .unwrap_or_else(|| {
                        Some(email_product::ProductAccount {
                            id: c.id.0.clone(),
                            root: mail_root.join(&c.id.0),
                            address: c.address.clone(),
                        })
                    })
            }))
            .collect();
        // One `EmailSync` service, several backends behind it: local
        // Maildir accounts plus any remote IMAP accounts (Gmail &c)
        // declared by an `account.json`. The mux routes by account id
        // and gives both sub-backends one shared change hub, so
        // subscribers still see a single stream.
        // Every account's on-disk directory, for the mux's sqlite
        // index. Local accounts keep it beside their maildir; remote
        // ones beside their `account.json`.
        #[cfg(feature = "plugin-email")]
        let account_dirs: std::collections::HashMap<String, PathBuf> = mail_accounts
            .iter()
            .map(|e| (e.account.id.0.clone(), e.root.clone()))
            .chain(
                mail_configs
                    .iter()
                    .map(|c| (c.id.0.clone(), mail_root.join(&c.id.0))),
            )
            .collect();
        #[cfg(feature = "plugin-email")]
        let email = email_mux::Backend::build(mail_accounts, mail_configs, account_dirs);

        // Push delivery for remote accounts: one IDLE loop per IMAP
        // account's INBOX, publishing into the shared hub. Failures
        // inside the loop are its own problem (it reconnects with
        // backoff) — startup does not depend on the server being
        // reachable, so a wrong password degrades to "that account
        // lists nothing" rather than blocking boot.
        #[cfg(feature = "plugin-email")]
        if let Some(imap) = email.imap() {
            for account in email.imap_account_ids() {
                let imap = imap.clone();
                let acct = account.clone();
                tokio::spawn(async move {
                    if let Err(err) = imap.start_idle(&acct, "INBOX").await {
                        tracing::warn!(account = %acct, ?err, "imap: IDLE watcher not started");
                    }
                });
            }
        }

        // Known-sender scoring rides the org's contacts when that
        // plugin is compiled in; without it every sender is scored
        // "unknown" (the trait's empty set).
        #[cfg(all(feature = "plugin-email", feature = "plugin-contacts"))]
        let email_contacts: std::sync::Arc<dyn email_product::ContactLookup> =
            std::sync::Arc::new(VaultContactLookup(contacts.clone()));
        #[cfg(all(feature = "plugin-email", not(feature = "plugin-contacts")))]
        let email_contacts: std::sync::Arc<dyn email_product::ContactLookup> =
            std::sync::Arc::new(NoContactLookup);

        // Email product layer — outbox with human-in-the-loop
        // approval + the bounded triage pass. Shares the maildir
        // backend's `EmailChange` hub (one stream for mailbox +
        // outbox + derivation events), delivers approved entries
        // through `EmailSync::send` on a 30s poller (approval
        // wakes it immediately), and scores senders against the
        // org's contacts.
        #[cfg(feature = "plugin-email")]
        let email_product = email_product::ProductBackend::new(
            product_accounts,
            std::sync::Arc::new(email.clone()),
            email_proto::EmailSyncStreamSource::changes_hub(&email).clone(),
            email_contacts,
        )
        .map_err(|e| eyre::eyre!("email product stores: {e}"))?;
        #[cfg(feature = "plugin-email")]
        let _email_poller = email_product.spawn_poller(std::time::Duration::from_secs(30));

        // Link store lives beside the org's other databases, not under
        // a mailbox: links are org-scoped and outlive any one account.
        #[cfg(feature = "plugin-email")]
        let email_links = email_link::LinkBackend::open(org_root.path())
            .map_err(|e| eyre::eyre!("email link store: {e}"))?;

        // Forge backend — Forgejo, the org's primary forge. Base
        // URL + token come from the same env vars the CLI's forge
        // sync uses (`TASK_FORGEJO_BASE_URL` / `TASK_FORGEJO_TOKEN`,
        // falling back to `FORGEJO_TOKEN`). Both are optional: when
        // unset we build with empty strings so startup never fails on
        // a missing credential — the forge methods then return an
        // auth/forge `GitError` the /repos UI renders as an empty
        // list. `from_token` only errors when called outside a tokio
        // runtime, which `build_org_state` always is.
        #[cfg(feature = "plugin-git")]
        let forgejo_base = std::env::var("TASK_FORGEJO_BASE_URL").unwrap_or_default();
        #[cfg(feature = "plugin-git")]
        let forgejo_token = std::env::var("TASK_FORGEJO_TOKEN")
            .ok()
            .filter(|t| !t.is_empty())
            .or_else(|| std::env::var("FORGEJO_TOKEN").ok())
            .unwrap_or_default();
        #[cfg(feature = "plugin-git")]
        let forge = git_forgejo::Backend::from_token(forgejo_base.clone(), forgejo_token)
            .map_err(|e| eyre::eyre!("forge backend: {e}"))?;
        // Agent/bot identity for forge-sync attribution. Token from
        // `TASK_FORGEJO_BOT_TOKEN`, or `FTS_CODEBERG_ACCESS_TOKEN`
        // (the var name the sops-rendered `fts-codeberg.env` carries,
        // so a service `EnvironmentFile=` works without remapping).
        // When neither is set we reuse the human backend, so
        // agent-owned tasks still sync — just under the human
        // identity until the bot token is configured.
        #[cfg(feature = "plugin-git")]
        let forge_agent = match std::env::var("TASK_FORGEJO_BOT_TOKEN")
            .ok()
            .or_else(|| std::env::var("FTS_CODEBERG_ACCESS_TOKEN").ok())
            .filter(|t| !t.is_empty())
        {
            Some(bot_token) => git_forgejo::Backend::from_token(forgejo_base, bot_token)
                .map_err(|e| eyre::eyre!("forge agent backend: {e}"))?,
            None => forge.clone(),
        };

        // Auto-retry any wiki ingest tasks the previous
        // backend left stuck mid-flight. Best-effort —
        // failures here shouldn't block startup.
        #[cfg(feature = "plugin-wiki")]
        if let Ok(entries) = std::fs::read_dir(&vault_root) {
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                let wiki_handle = wiki_live::WikiLive::open(entry.path());
                if !wiki_handle.is_bootstrapped() {
                    continue;
                }
                if let Ok((retried, failed)) = wiki_handle.auto_retry_stuck_tasks(3) {
                    if !retried.is_empty() || !failed.is_empty() {
                        tracing::info!(
                            vault = %entry.path().display(),
                            retried = retried.len(),
                            failed = failed.len(),
                            "wiki auto-retry: revived stuck tasks"
                        );
                    }
                }
            }
        }

        // Project + Goal readers. Both walk
        // `<org>/vault/` on each call; cheap-clone PathBuf
        // wrappers, no shared mutable state.
        let projects = project::ProjectBackend::new(vault_root.clone());
        let goals = goal::GoalBackend::new(vault_root.clone());
        let milestones = milestone::MilestoneBackend::new(vault_root.clone());
        let workstreams = workstream::WorkstreamBackend::new(vault_root.clone());
        // Root content lives outside the vault (`<org>/files/`); the
        // Named / Project Version entities that reference it are
        // ordinary vault pages, so the backend gets both paths.
        // Storage Locations widen where this org may hold live trees
        // (issue #262): without this a File Root can only live under
        // `<org>/files`, which is on the server's own disk — so media
        // that was never going to fit there, on a NAS or an external
        // volume, could not be registered at all.
        let files = files::FilesBackend::new(org_root.path().join("files"), vault_root.clone())
            .map_err(|e| eyre::eyre!("files backend: {e}"))?
            .with_location_boundaries(Arc::new(crate::storage::GrantedBoundaries::new(
                Arc::clone(storage),
                org_root.slug().to_owned(),
            )));
        // t[impl project.vault.write-path] — the org vault is a File
        // Root from boot, and every vault page the server writes from
        // here on goes through the Files API rather than `std::fs`.
        // Before `enable_watching`, so the vault is watched like any
        // other root. A vault that cannot be adopted still serves — the
        // pages fall back to the filesystem write they always had — but
        // that is a degraded server and is logged as one.
        // On the blocking pool, like every other sync entry into the
        // Files backend: adoption opens the root's store, which is
        // `pollster`-driven jj/iroh-blobs work that must not park a
        // runtime worker.
        let adopting = files.clone();
        match tokio::task::spawn_blocking(move || adopting.adopt_vault()).await {
            Ok(Ok(_)) => {}
            Ok(Err(err)) => tracing::error!(
                org = %org_root.slug(),
                %err,
                "files: the org vault could not be adopted as a root; vault writes bypass Files"
            ),
            Err(err) => tracing::error!(
                org = %org_root.slug(),
                %err,
                "files: vault adoption panicked; vault writes bypass Files"
            ),
        }
        // And every other knowledge directory — the wiki tier, each named
        // wiki, the resource library, each subscribed copy — so a machine
        // syncing this org can show them all, not the vault alone. Read
        // from disk here and again on every device-sync sweep, which is
        // what lets a wiki created later appear without a restart.
        match crate::org_roots::adopt_knowledge_roots(&files, &org_root).await {
            0 => {}
            n => {
                tracing::info!(org = %org_root.slug(), adopted = n, "files: knowledge trees adopted as roots")
            }
        }
        let files_webdav = files_webdav::WebdavBridge::new(files.clone());
        // Placement lane. The coordinator is the deployment's, owned by
        // `AppState` and passed in — this is just this org's view of it.
        let storage = files_storage::StorageBackend::new(storage.clone(), org_root.slug());
        // The Files cadence engine (issue #260): one driver task ticks
        // the engine, and every root gets an inotify watch feeding it
        // activity hints. The interval only bounds how promptly a due
        // capture happens — the cadence itself (10-minute auto-
        // snapshots, 30-minute quiescence) is the engine's.
        files.enable_watching().await;
        files.spawn_cadence_driver(std::time::Duration::from_secs(30));
        // Derived media (issue #269): wire the real ffmpeg driver when
        // the toolchain is on PATH. Without it the rendition RPC (and
        // the Review page's player, issue #270) reports "no transcoder
        // configured" — everything else works, so absence is a warn,
        // not an error.
        match tokio::process::Command::new("ffprobe")
            .arg("-version")
            .output()
            .await
        {
            Ok(out) if out.status.success() => {
                files.set_transcoder(std::sync::Arc::new(
                    files_transcode::transcoder::ffmpeg::FfmpegTranscoder,
                ));
                tracing::info!(org = org_root.slug(), "files: ffmpeg transcoder wired");
            }
            _ => {
                tracing::warn!(
                    org = org_root.slug(),
                    "files: ffmpeg/ffprobe not on PATH — media renditions disabled"
                );
            }
        }
        let tasks = task::TaskBackend::new(vault_root.clone());
        // The Edit lane, over the wikis above and the tasks just built:
        // an Edit Request is an issue on this org's board
        // (`wiki.edit.tracked`). A claim stands an hour unless the
        // deployment says otherwise (`TASK_WIKI_CLAIM_TTL_SECS`) — a
        // test shrinks it to watch one expire.
        #[cfg(feature = "plugin-wiki")]
        let edits = {
            let mut edits = wiki_live::edits_backend::EditsBackend::new(
                wiki.clone(),
                std::sync::Arc::new(crate::wiki_tracker::TaskTracker::new(tasks.clone())),
            );
            if let Some(secs) = std::env::var("TASK_WIKI_CLAIM_TTL_SECS")
                .ok()
                .and_then(|v| v.trim().parse::<u64>().ok())
            {
                edits = edits.with_claim_ttl(std::time::Duration::from_secs(secs));
            }
            // t[impl wiki.source.sync] — every wiki with a repository
            // behind it is fetched on a schedule, first pass at boot
            // (spawned, so a slow remote never delays serving). What
            // it finds is written to the wiki's config, which is what a
            // client shows as "reflects commit …" or "stale since …";
            // and each sync settles the Edit lane's landings against it.
            crate::wiki_repo::spawn_sync_loop(org_root.slug().to_string(), edits.clone());
            edits
        };
        // Locations + mealplan / pantry each hold their own
        // `vault::Vault` snapshot behind an `Arc<Mutex<…>>`.
        // We open the vault once per store — they're independent
        // mutable views; cross-coordination happens at the
        // service level. `Vault::open` is cheap (no parsing
        // beyond directory walk).
        #[cfg(feature = "plugin-home")]
        let locations_vault = vault::Vault::open(&vault_root)
            .map_err(|e| eyre::eyre!("open locations vault: {e}"))?;
        #[cfg(feature = "plugin-home")]
        let locations = locations::Store::new(locations_vault);
        // Inventory — `type: item` gear/equipment pages. Its own
        // `vault::Vault` snapshot behind an `Arc<Mutex<…>>`, like
        // locations.
        #[cfg(feature = "plugin-home")]
        let inventory_vault = vault::Vault::open(&vault_root)
            .map_err(|e| eyre::eyre!("open inventory vault: {e}"))?;
        #[cfg(feature = "plugin-home")]
        let inventory = inventory::Store::new(inventory_vault);
        // Typed-link store (user-asserted verse/note/wiki links, plus
        // the derived links the vault and sermon syncs mint). Opened
        // before scripture: the reader's backlinks read it.
        let links = links::Store::open(org_root.path().join("links.jsonl"));
        // Scripture — read-only Bible spine loaded from the resource
        // library (`<org>/resources/bible/<TX>/`). A missing root yields
        // an empty store, so orgs without an installed corpus just show
        // no translations.
        // Copyright-restricted editions, fetched live with the user's key
        // (never bundled). ESV needs a Crossway key; NIV rides API.Bible
        // and additionally needs that edition's `bible_id` (NIV is tightly
        // licensed — only works if the key has NIV access).
        #[cfg(feature = "plugin-scripture")]
        let mut scripture_api = Vec::new();
        #[cfg(feature = "plugin-scripture")]
        if let Ok(key) = std::env::var("TASK_ESV_API_KEY").or_else(|_| std::env::var("ESV_API_KEY"))
        {
            if !key.is_empty() {
                scripture_api.push(scripture::ApiTranslation::esv(key));
            }
        }
        #[cfg(feature = "plugin-scripture")]
        if let (Ok(key), Ok(bible_id)) = (
            std::env::var("TASK_API_BIBLE_KEY").or_else(|_| std::env::var("API_BIBLE_KEY")),
            std::env::var("TASK_API_BIBLE_NIV_ID"),
        ) {
            if !key.is_empty() && !bible_id.is_empty() {
                scripture_api.push(scripture::ApiTranslation::api_bible(
                    "NIV",
                    "New International Version",
                    bible_id,
                    key,
                ));
            }
        }
        // Strong's lexicon for word study (`<org>/resources/lexicon/strongs/`);
        // empty if not installed.
        #[cfg(feature = "plugin-scripture")]
        let scripture_lexicon =
            scripture::Lexicon::load_dir(&org_root.resources_dir().join("lexicon").join("strongs"))
                .map_err(|e| eyre::eyre!("load lexicon: {e}"))?;
        #[cfg(feature = "plugin-scripture")]
        let scripture =
            scripture::Store::load_resource_root(&org_root.resources_dir().join("bible"))
                .map_err(|e| eyre::eyre!("load scripture: {e}"))?
                // The vault powers per-verse backlinks: notes that link
                // `[[John 3:16]]` surface in the reader.
                .with_vault(vault_root.clone())
                // So do media sources: a sermon whose captions name a
                // verse (`sermon:<slug>#t:<secs> → verse:<osis>`, minted
                // by the sermon sync) is listed at the moment it said it.
                .with_media_links(links.clone(), org_root.resources_dir())
                .with_api(scripture_api)
                .with_lexicon(scripture_lexicon)
                // Original-language editions (TAGNT/TAHOT/SBLGNT/OSHB),
                // loaded lazily per edition on first interlinear request.
                .with_originals_root(org_root.resources_dir().join("original"))
                // Versification mappings reconcile Hebrew vs English
                // verse numbering for the interlinear.
                .with_versification(
                    scripture::Versification::load_dir(
                        &org_root.resources_dir().join("versification"),
                    )
                    .map_err(|e| eyre::eyre!("load versification: {e}"))?,
                )
                // OpenBible cross-references + topical tags (CC BY,
                // vote-weighted), lazy-loaded on first query.
                .with_crossref(
                    org_root
                        .resources_dir()
                        .join("crossref")
                        .join("cross_references.txt"),
                )
                .with_topics(
                    org_root
                        .resources_dir()
                        .join("topics")
                        .join("topic-votes.txt"),
                );
        // Ordered-collection store — Library / Setlist / Show / Playlist.
        // JSONL at `<org>/collections.jsonl` (override via
        // `TASK_SERVER_COLLECTIONS_PATH`, mirroring the vault-root override
        // so tests can isolate it). A missing file is an empty store.
        #[cfg(feature = "plugin-fasttrackstudio")]
        let collections_path = std::env::var("TASK_SERVER_COLLECTIONS_PATH")
            .map_or_else(|_| org_root.path().join("collections.jsonl"), PathBuf::from);
        #[cfg(feature = "plugin-fasttrackstudio")]
        let collections = collection::Store::open(collections_path);
        // Keep `note → verse` + `note → note` links live as notes are
        // saved: a background task syncs each changed note's
        // `[[wikilinks]]` into the store.
        crate::link_sync::spawn(
            links.clone(),
            vault_root.clone(),
            vault_graph.clone(),
            vault_sync_state.channel("default").await.subscribe(),
        );
        // Resource Library: transcript sidecars under resources/ for the
        // watch view, and the sermon sync's write path — which mints the
        // sermon's `→ verse` links into the same typed-link store.
        let resources = resources::ResourcesBackend::new(org_root.resources_dir())
            .with_wikis(org_root.wikis_dir())
            .with_links(links.clone());
        // Cookbook lives at `<wiki>/Cookbook/*.cook`, NOT the vault
        // root. Which wiki is now a real question: with named wikis an
        // org can hold a Cooking wiki, and recipes belong there rather
        // than in the org's default knowledge tier — a wiki is
        // subscribable, so recipes kept in one can be shared and
        // referenced instead of being a per-org plugin store nobody
        // else can reach.
        //
        // Prefer a `cooking` wiki when the org has one; fall back to
        // the default wiki, which is where every existing org's
        // recipes already are. Backwards compatible by construction:
        // an org with no Cooking wiki sees exactly what it saw before.
        #[cfg(feature = "plugin-mealplan")]
        let cookbook = {
            let cooking = org_root.named_wiki_dir("cooking");
            let root = if cooking.is_dir() {
                cooking
            } else {
                wiki_root.clone()
            };
            cookbook::Store::new(root)
        };
        #[cfg(feature = "plugin-mealplan")]
        let mealplan_vault =
            vault::Vault::open(&vault_root).map_err(|e| eyre::eyre!("open mealplan vault: {e}"))?;
        // `with_cookbook`: meals live in the vault, but their
        // recipe paths resolve against the wiki-rooted
        // cookbook above — without it `can_cook` / cook
        // deductions look for `.cook` files under the vault
        // root and never find them.
        #[cfg(feature = "plugin-mealplan")]
        let mealplan = mealplan::Store::new(mealplan_vault).with_cookbook(cookbook.clone());
        // Shopping list + substitution-rule services — sibling
        // mealplan stores, each its own vault snapshot.
        #[cfg(feature = "plugin-mealplan")]
        let shopping_vault =
            vault::Vault::open(&vault_root).map_err(|e| eyre::eyre!("open shopping vault: {e}"))?;
        #[cfg(feature = "plugin-mealplan")]
        let shopping =
            mealplan::shopping::Store::new(shopping_vault).with_cookbook(cookbook.clone());
        #[cfg(feature = "plugin-mealplan")]
        let substitutions_vault = vault::Vault::open(&vault_root)
            .map_err(|e| eyre::eyre!("open substitutions vault: {e}"))?;
        #[cfg(feature = "plugin-mealplan")]
        let substitutions = mealplan::substitutions::Store::new(substitutions_vault);
        #[cfg(feature = "plugin-mealplan")]
        let pantry_vault =
            vault::Vault::open(&vault_root).map_err(|e| eyre::eyre!("open pantry vault: {e}"))?;
        #[cfg(feature = "plugin-mealplan")]
        let pantry = pantry::Store::new(pantry_vault);
        // Fitness suite. Each takes its own vault snapshot.
        #[cfg(feature = "plugin-fitness")]
        let body_vault =
            vault::Vault::open(&vault_root).map_err(|e| eyre::eyre!("open body vault: {e}"))?;
        #[cfg(feature = "plugin-fitness")]
        let body = body::Store::new(body_vault);
        #[cfg(feature = "plugin-fitness")]
        let exercises_vault = vault::Vault::open(&vault_root)
            .map_err(|e| eyre::eyre!("open exercises vault: {e}"))?;
        #[cfg(feature = "plugin-fitness")]
        let exercises = exercises::Store::new(exercises_vault);
        #[cfg(feature = "plugin-fitness")]
        let workouts_vault =
            vault::Vault::open(&vault_root).map_err(|e| eyre::eyre!("open workouts vault: {e}"))?;
        #[cfg(feature = "plugin-fitness")]
        let workouts = workouts::Store::new(workouts_vault);
        #[cfg(feature = "plugin-fitness")]
        let intake_vault =
            vault::Vault::open(&vault_root).map_err(|e| eyre::eyre!("open intake vault: {e}"))?;
        #[cfg(feature = "plugin-fitness")]
        let intake = intake::Store::new(intake_vault);

        // Every open sqlite pool, for the snapshot engine's
        // wal_checkpoint pass. Keep in lockstep with the pools
        // opened above — a missing entry only costs checkpoint
        // coverage for that db, never correctness of live serving.
        let mut sqlite_conns = vec![
            auth.db.clone(),
            timer.conn().clone(),
            threads.conn().clone(),
            notify_store.conn().clone(),
            prefs.conn().clone(),
        ];
        #[cfg(feature = "plugin-agent")]
        sqlite_conns.push(agent_tasks.conn().clone());
        #[cfg(feature = "plugin-finance")]
        sqlite_conns.push(finance_conn.clone());
        // Identity locker only exists on the home org; include its pool
        // in the snapshot checkpoint set when it was opened.
        if let Some(store) = &identity {
            sqlite_conns.push(store.conn().clone());
        }

        let permissions = Arc::new(build_org_permissions_gate(
            &auth,
            &plugins,
            org_root.slug(),
            home_identity,
            &files,
        ));
        // Coverage + dry-run, once per org at boot: how many mounted
        // services carry a permit table, which do not, and what a
        // signed-in member would be denied if enforcement were on. The
        // gap used to be silent — that is how it sat at 2/71.
        permits::log_coverage(org_root.slug(), &permissions, enforce_permissions());
        let shares = Arc::new(share::ShareStore::open(org_root.path()));

        Ok(OrgAppState {
            slug: org_root.slug().to_owned(),
            org_root: org_root.clone(),
            plugins,
            auth,
            permissions,
            shares,
            attachments: attachment_service,
            vault_sync: vault_sync_state,
            vault_collab,
            vault_watcher,
            #[cfg(feature = "plugin-scripture")]
            scripture,
            links,
            #[cfg(feature = "plugin-fasttrackstudio")]
            collections,
            resources,
            #[cfg(feature = "plugin-wiki")]
            wiki,
            #[cfg(feature = "plugin-wiki")]
            wiki_vaults,
            #[cfg(feature = "plugin-wiki")]
            subscriptions,
            #[cfg(feature = "plugin-wiki")]
            edits,
            projects,
            goals,
            milestones,
            workstreams,
            files,
            files_webdav,
            storage,
            tasks,
            #[cfg(feature = "plugin-home")]
            locations,
            #[cfg(feature = "plugin-home")]
            inventory,
            #[cfg(feature = "plugin-mealplan")]
            cookbook,
            #[cfg(feature = "plugin-mealplan")]
            mealplan,
            #[cfg(feature = "plugin-mealplan")]
            shopping,
            #[cfg(feature = "plugin-mealplan")]
            substitutions,
            #[cfg(feature = "plugin-mealplan")]
            pantry,
            #[cfg(feature = "plugin-fitness")]
            body,
            #[cfg(feature = "plugin-fitness")]
            exercises,
            #[cfg(feature = "plugin-fitness")]
            workouts,
            #[cfg(feature = "plugin-fitness")]
            intake,
            #[cfg(feature = "plugin-agent")]
            agent_tasks,
            #[cfg(feature = "plugin-agent")]
            agent_runners,
            #[cfg(feature = "plugin-agent")]
            agent_runs,
            #[cfg(feature = "plugin-agent")]
            agent_questions,
            #[cfg(feature = "plugin-agent")]
            agent_codex,
            #[cfg(feature = "plugin-agent")]
            agent_router,
            agent_dispatch_vault_root: vault_root,
            timer,
            threads,
            prefs,
            identity,
            #[cfg(feature = "plugin-scheduling")]
            scheduling,
            inbox,
            notify: notify_store,
            #[cfg(feature = "plugin-recall")]
            recall,
            #[cfg(feature = "plugin-contacts")]
            contacts,
            tags,
            #[cfg(feature = "plugin-finance")]
            finance_conn,
            #[cfg(feature = "plugin-finance")]
            finance_backend,
            #[cfg(feature = "plugin-finance")]
            ledger_backend,
            #[cfg(feature = "plugin-email")]
            email,
            #[cfg(feature = "plugin-email")]
            email_product,
            #[cfg(feature = "plugin-email")]
            email_links,
            #[cfg(feature = "plugin-git")]
            forge,
            #[cfg(feature = "plugin-git")]
            forge_agent,
            issue_links_path: org_root.path().join("issue-links.json"),
            presence: crdt::sync::PresenceHost::new(
                presence::PRESENCE_DOC_ID,
                presence::PRESENCE_TIMEOUT_MS,
            ),
            vault_graph,
            sqlite_conns,
        })
    }
}

/// Discover Maildir accounts under `mail_root`. Each immediate
/// subdirectory is one account: its dir name is the account id
/// (and the default display name / address). When the dir
/// carries an `account.json` (`email_config::AccountConfig`,
/// JSON), the config supplies the real address / display name /
/// folder aliases — and, for a `Maildir` backend kind with a
/// `submit` block, an SMTP submitter so `EmailSync::send` works
/// for that account. A broken `account.json` is logged and the
/// account falls back to the bare-directory defaults (read-only).
/// An absent or empty `mail_root` yields an empty vec — the
/// backend then serves no accounts, which is a valid
/// "operational but unconfigured" state.
/// `email-product` known-sender lookup over the org's vault
/// contacts. One `list_contacts` walk per triage pass (the pass
/// snapshots the result), lower-cased addresses.
#[cfg(all(feature = "plugin-email", feature = "plugin-contacts"))]
struct VaultContactLookup(contacts::VaultContacts);

#[cfg(all(feature = "plugin-email", feature = "plugin-contacts"))]
impl email_product::ContactLookup for VaultContactLookup {
    fn known_addresses(&self) -> std::collections::BTreeSet<String> {
        use contacts_proto::Contacts as _;
        match self.0.list_contacts() {
            Ok(list) => list
                .iter()
                .flat_map(contacts_proto::Contact::email_list)
                .map(str::to_ascii_lowercase)
                .collect(),
            Err(err) => {
                tracing::debug!(%err, "contact lookup failed; treating senders as unknown");
                std::collections::BTreeSet::new()
            }
        }
    }
}

/// The contacts plugin is compiled out: every sender scores
/// "unknown" — triage still runs, it just never grants the
/// known-sender boost.
#[cfg(all(feature = "plugin-email", not(feature = "plugin-contacts")))]
struct NoContactLookup;

#[cfg(all(feature = "plugin-email", not(feature = "plugin-contacts")))]
impl email_product::ContactLookup for NoContactLookup {
    fn known_addresses(&self) -> std::collections::BTreeSet<String> {
        std::collections::BTreeSet::new()
    }
}

#[cfg(feature = "plugin-email")]
/// Scan the org's mail root: one directory per account, each
/// optionally carrying an `account.json` ([`email_config::AccountConfig`]).
///
/// Returns both views the multiplexer needs — the maildir entries for
/// local accounts, and every parsed config, which is what decides
/// whether an account is local or remote IMAP. An account directory
/// with no config is a plain Maildir, which keeps the zero-config
/// fixture mailbox working.
fn discover_mail_accounts(
    mail_root: &std::path::Path,
) -> (
    Vec<email_maildir::AccountEntry>,
    Vec<email_config::AccountConfig>,
) {
    let Ok(entries) = std::fs::read_dir(mail_root) else {
        return (Vec::new(), Vec::new());
    };
    let mut accounts = Vec::new();
    let mut configs = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
            continue;
        };

        let mut account = email_proto::Account {
            id: email_proto::AccountId(name.to_owned()),
            name: name.to_owned(),
            address: name.to_owned(),
            display_name: None,
        };
        let mut aliases = email_config::FolderAliases::new();
        let mut submit: Option<std::sync::Arc<dyn email_maildir::Submit>> = None;

        match email_config::AccountConfig::load_json(&path.join("account.json")) {
            Ok(Some(cfg)) => {
                let is_remote = !matches!(cfg.backend, email_config::BackendKind::Maildir { .. });
                configs.push(cfg.clone());
                // Keep the directory name as the account id (the
                // maildir root is keyed by it); take identity +
                // aliases + submit from the config.
                account.name = cfg.name.clone();
                account.address = cfg.address.clone();
                account.display_name = cfg.display_name.clone();
                aliases = cfg.folder_aliases.clone();
                if let email_config::BackendKind::Maildir {
                    submit: Some(smtp), ..
                } = &cfg.backend
                {
                    submit = Some(std::sync::Arc::new(email_smtp::SmtpSender::new(
                        smtp.clone(),
                    )));
                }
                // A remote account has no local maildir to serve
                // from — routing sends its calls to IMAP instead.
                if is_remote {
                    continue;
                }
            }
            Ok(None) => {}
            Err(err) => {
                tracing::warn!(account = name, %err, "invalid account.json; using defaults");
            }
        }

        accounts.push(email_maildir::AccountEntry {
            account,
            root: path,
            aliases,
            submit,
        });
    }
    (accounts, configs)
}

/// Dev default — replace via config in a later phase. Length-checked
/// at build time so this fails loudly if shortened.
const DEFAULT_AUTH_SECRET: &str = "task-server-auth-dev-secret-32+!";

/// The secret that signs every org's session tokens. A real deployment
/// MUST set `TASK_AUTH_SECRET` (a high-entropy 32+ char value) — the
/// hardcoded [`DEFAULT_AUTH_SECRET`] is a dev convenience and makes
/// tokens forgeable. Falls back to it (with a warning) when unset.
pub(crate) fn auth_secret() -> String {
    match std::env::var("TASK_AUTH_SECRET") {
        Ok(s) if !s.is_empty() => s,
        _ => {
            tracing::warn!(
                "TASK_AUTH_SECRET unset — using the dev auth secret (tokens are forgeable)"
            );
            DEFAULT_AUTH_SECRET.to_owned()
        }
    }
}

/// Pick the org root this server should serve.
///
/// Order: explicit `slug` arg → `TASK_SERVER_ORG` env →
/// scan-and-disambiguate. If `<data_root>/orgs/` has one
/// loadable org, use it. If none, auto-bootstrap `default`
/// so a fresh install boots. If many, refuse — operator
/// must pick one (PR 4 lifts this and serves all of them).
/// Pick the [`OrgRoot`]s this server should host.
///
/// - `slug_filter = Some` → host exactly that one org
///   (rejects on missing dir).
/// - `slug_filter = None` + `$TASK_SERVER_ORG` set → host
///   just that env-selected org (legacy single-org boot).
/// - `slug_filter = None`, env unset → host every loadable
///   org under `<data_root>/orgs/`. Returns an empty vec
///   when none exist; the server-management RPC handles
///   first-org bootstrap from there.
fn pick_server_orgs(
    data_root: &org_proto::DataRoot,
    slug_filter: Option<&str>,
) -> eyre::Result<Vec<org_proto::OrgRoot>> {
    let explicit = slug_filter
        .map(str::to_owned)
        .or_else(|| std::env::var("TASK_SERVER_ORG").ok())
        .filter(|s| !s.is_empty());
    if let Some(slug) = explicit {
        let (org_root, _) = data_root
            .load_org(&slug)
            .map_err(|e| eyre::eyre!("load org `{slug}`: {e}"))?;
        return Ok(vec![org_root]);
    }
    let scanned = data_root
        .scan_orgs()
        .map_err(|e| eyre::eyre!("scan orgs: {e}"))?;
    // Empty data root is no longer auto-bootstrapped. The
    // `/server/vox` `OrgManagementService` accepts an
    // unauthenticated `create_org` while in this state, so
    // the CLI flow `task org create … --home` mints the
    // first org without anyone touching the server's
    // filesystem directly.
    Ok(scanned.into_iter().map(|(org, _)| org).collect())
}

/// Parse a single HTTP byte-range request (`bytes=start-end`, `bytes=start-`,
/// or the suffix form `bytes=-N`) against a known total size, returning the
/// inclusive `[start, end]` it resolves to. Multi-range and unsatisfiable
/// requests return `None` (the caller then serves the full body).
pub(crate) fn parse_byte_range(value: &str, total: u64) -> Option<(u64, u64)> {
    if total == 0 {
        return None;
    }
    let spec = value.trim().strip_prefix("bytes=")?;
    // Only a single range is supported; a comma means multi-range → fall back.
    if spec.contains(',') {
        return None;
    }
    let (a, b) = spec.split_once('-')?;
    let last = total - 1;
    let (start, end) = if a.is_empty() {
        // Suffix range: the final N bytes.
        let n: u64 = b.trim().parse().ok()?;
        if n == 0 {
            return None;
        }
        (total.saturating_sub(n), last)
    } else {
        let start: u64 = a.trim().parse().ok()?;
        let end = if b.trim().is_empty() {
            last
        } else {
            b.trim().parse::<u64>().ok()?.min(last)
        };
        (start, end)
    };
    if start > end || start > last {
        return None;
    }
    Some((start, end))
}

/// JSON listing of a media directory's immediate entries, sorted by name.
/// Each entry is `{ "name": String, "dir": bool, "size": u64 }` (`size` is 0
/// for directories). Lets a manifest-less song enumerate its stem files over
/// HTTP (e.g. `GET /org/{slug}/media/songs/{slug}/stems/`).
async fn media_dir_listing(dir: &std::path::Path) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    let mut rd = match tokio::fs::read_dir(dir).await {
        Ok(rd) => rd,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    let mut entries: Vec<serde_json::Value> = Vec::new();
    while let Ok(Some(e)) = rd.next_entry().await {
        let name = e.file_name().to_string_lossy().into_owned();
        // Skip hidden/dotfiles.
        if name.starts_with('.') {
            continue;
        }
        let (dir, size) = match e.metadata().await {
            Ok(m) => (m.is_dir(), if m.is_dir() { 0 } else { m.len() }),
            Err(_) => continue,
        };
        entries.push(serde_json::json!({ "name": name, "dir": dir, "size": size }));
    }
    entries.sort_by(|a, b| {
        a["name"]
            .as_str()
            .unwrap_or_default()
            .cmp(b["name"].as_str().unwrap_or_default())
    });
    (
        [(header::CONTENT_TYPE, "application/json")],
        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".into()),
    )
        .into_response()
}

/// Query string of [`per_org_media_handler`] — the signed media token.
#[derive(serde::Deserialize, Default)]
pub struct MediaQuery {
    /// [`BlobToken::media`], base64url. Absent for an anonymous read.
    #[serde(default)]
    pub token: Option<String>,
}

/// Is the media route enforcing? **Off unless `TASK_ENFORCE_MEDIA_TOKEN`
/// is exactly `1`** — the deliberate operator action, matching
/// [`enforce_permissions`].
#[must_use]
pub fn enforce_media_token() -> bool {
    std::env::var("TASK_ENFORCE_MEDIA_TOKEN").is_ok_and(|v| v == "1")
}

/// Decide whether a media read is allowed. `None` = proceed;
/// `Some(response)` = refuse with it.
///
/// Always evaluates and always records, so the observe phase answers
/// "would enforcing break anyone?" from the traces. Only the final
/// refusal is conditional on [`enforce_media_token`].
async fn authorize_media(
    state: &AppState,
    slug: &str,
    rel: &str,
    token: Option<&str>,
    headers: &axum::http::HeaderMap,
) -> Option<axum::response::Response> {
    use architect_telemetry::wide;

    wide::set("org.slug", slug.to_owned());
    wide::set("media.path", rel.to_owned());

    // 1. Signed prefix token — the browser channel (an <audio> tag can
    //    set no headers, so the grant has to live in the URL).
    if let Some(token) = token.filter(|t| !t.is_empty()) {
        let now = chrono::Utc::now().timestamp();
        match crate::attachments::signed_url::BlobToken::verify(token, &state.keypair, now) {
            Ok(tok) if tok.allows_media(slug, rel) => {
                wide::set("media.authorized", true);
                wide::set("media.auth_via", "signed-token");
                return None;
            }
            Ok(_) => {
                wide::set("media.authorized", false);
                wide::set("media.auth_via", "token-wrong-scope");
            }
            Err(e) => {
                wide::set("media.authorized", false);
                wide::set("media.auth_via", "token-invalid");
                wide::set_display("media.token_error", &format!("{e:?}"));
            }
        }
    } else if let Some(bearer) = crate::watch_bridge::bearer(headers) {
        // 2. Session bearer — native clients own their requests and
        //    shouldn't need a mint round trip for every file.
        let ok = match state.org(slug) {
            Some(org) => org
                .auth
                .auth
                .current_session(architect_auth::CurrentSession { token: bearer })
                .await
                .is_ok(),
            None => false,
        };
        wide::set("media.authorized", ok);
        wide::set(
            "media.auth_via",
            if ok { "bearer" } else { "bearer-invalid" },
        );
        if ok {
            return None;
        }
    } else {
        wide::set("media.authorized", false);
        wide::set("media.auth_via", "absent");
    }

    if enforce_media_token() {
        wide::set("media.mode", "enforcing");
        Some(axum::response::IntoResponse::into_response((
            axum::http::StatusCode::UNAUTHORIZED,
            "media requires a signed token or session bearer",
        )))
    } else {
        // Observe-only: the refusal that did not happen.
        wide::set("media.mode", "observe-only");
        tracing::warn!(
            target: "task_server::media",
            org = slug,
            path = rel,
            "media: WOULD DENY (served anyway — TASK_ENFORCE_MEDIA_TOKEN is off)",
        );
        None
    }
}

/// Filesystem-first song media: `GET /org/{slug}/media/<path>` serves
/// `<data_root>/orgs/{slug}/resources/<path>` straight off disk — no
/// ingest, no content-addressing, drop-a-file-and-it-serves (fits network
/// storage). `/org/*` is already edge-routed to task-server, so this is
/// reachable where a bare `/media` (behind the web SPA) is not. Traversal
/// guarded; content-type by extension.
///
/// Serves HTTP Range requests (`Accept-Ranges: bytes`, `206 Partial
/// Content`): browser media elements need this to determine seekability —
/// without it Chrome's `<audio>` loader stalls at `readyState 0` on Ogg
/// (which needs a seek to the tail to read its duration), so stems never
/// actually play. The reference stem player streams these directly.
///
/// ## Authorization
///
/// This route served the org's whole `resources/` tree — files AND
/// directory listings — to anyone, and it is NOT a vox path, so
/// `TASK_ENFORCE_PERMISSIONS` does not reach it (verified open on
/// production 2026-08-07). A caller now presents either a signed
/// `?token=` ([`BlobToken::media`], minted over vox by a caller the
/// permission gate already allowed) or an `Authorization: Bearer`
/// session token — the latter for native clients, which can set headers
/// and shouldn't need a mint round trip.
///
/// Gated by `TASK_ENFORCE_MEDIA_TOKEN` and OFF by default, deliberately
/// mirroring `TASK_ENFORCE_PERMISSIONS`: shipping the check hot would
/// black out every `<audio>`/`<img>` on the deployed bundle the moment
/// it rolled. The wide fields (`media.authorized`, `media.auth_via`)
/// make the observe phase queryable, so the flip is evidence-driven
/// rather than hopeful — the same ordering the vox work followed.
async fn per_org_media_handler(
    State(state): State<AppState>,
    axum::extract::Path((slug, rel)): axum::extract::Path<(String, String)>,
    axum::extract::Query(q): axum::extract::Query<MediaQuery>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use tokio::io::{AsyncReadExt, AsyncSeekExt};
    if rel.split('/').any(|s| s == ".." || s.is_empty()) {
        return StatusCode::NOT_FOUND.into_response();
    }
    if let Some(refusal) = authorize_media(&state, &slug, &rel, q.token.as_deref(), &headers).await
    {
        return refusal;
    }
    let file = state
        .data_root
        .org(slug.as_str())
        .path()
        .join("resources")
        .join(&rel);
    let meta = match tokio::fs::metadata(&file).await {
        Ok(m) => m,
        Err(_) => return StatusCode::NOT_FOUND.into_response(),
    };
    // A directory → JSON listing of its immediate entries. This is how a
    // manifest-less colocated song enumerates its stems (`…/stems/`,
    // `…/media/proxy/`) — the client lists the dir and derives the stem set,
    // deriving structure separately from `arrangements/original.kf`.
    if meta.is_dir() {
        return media_dir_listing(&file).await;
    }
    if !meta.is_file() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let total = meta.len();
    let ct = match file.extension().and_then(|x| x.to_str()) {
        Some("json") => "application/json",
        Some("ogg") => "audio/ogg",
        Some("wav") => "audio/wav",
        Some("mp3") => "audio/mpeg",
        Some("webm") => "audio/webm",
        Some("kf") | Some("txt") | Some("md") => "text/plain; charset=utf-8",
        _ => "application/octet-stream",
    };

    // Honour a single byte-range request → 206 Partial Content.
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_byte_range(s, total));
    if let Some((start, end)) = range {
        let len = end - start + 1;
        let mut f = match tokio::fs::File::open(&file).await {
            Ok(f) => f,
            Err(_) => return StatusCode::NOT_FOUND.into_response(),
        };
        if f.seek(std::io::SeekFrom::Start(start)).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        let mut buf = vec![0u8; len as usize];
        if f.read_exact(&mut buf).await.is_err() {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        return (
            StatusCode::PARTIAL_CONTENT,
            [
                (header::CONTENT_TYPE, ct.to_string()),
                (header::ACCEPT_RANGES, "bytes".to_string()),
                (
                    header::CONTENT_RANGE,
                    format!("bytes {start}-{end}/{total}"),
                ),
                (header::CONTENT_LENGTH, len.to_string()),
            ],
            buf,
        )
            .into_response();
    }

    // No (or unsatisfiable) range → full body, but still advertise range support.
    match tokio::fs::read(&file).await {
        Ok(bytes) => (
            [
                (header::CONTENT_TYPE, ct.to_string()),
                (header::ACCEPT_RANGES, "bytes".to_string()),
                (header::CONTENT_LENGTH, total.to_string()),
            ],
            bytes,
        )
            .into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}

/// Rendition streaming (issue #270): `GET
/// /org/{slug}/files/renditions/{root_id}/{kind}/{file_id}` serves a
/// derived rendition (issue #269 — proxy, filmstrip, peaks) out of the
/// root's *private* rendition CAS. Originals are never reachable here:
/// `file_id` must be a rendition content id in this root's rendition
/// store, and the source-content read paths can't see that store.
///
/// Honours a single HTTP byte range with `206 Partial Content`, reading
/// only the chunks overlapping the window — the `<video>` proxy seek
/// path, so a scrub never pulls the whole file.
///
/// Authorization matches `/org/{slug}/media` exactly (same channels,
/// same `TASK_ENFORCE_MEDIA_TOKEN` flag): a browser media element can't
/// set headers, so the grant is a signed `?token=` minted over vox —
/// prefix `files/renditions/{root_id}` covers a review's whole
/// rendition ladder — or an `Authorization: Bearer` session token for
/// native clients.
///
/// Content-Type comes from the `{kind}` path segment (the stable
/// rendition tag, e.g. `proxy-720`), never from sniffing the bytes.
async fn files_rendition_handler(
    State(state): State<AppState>,
    axum::extract::Path((slug, root_id, kind, file_id)): axum::extract::Path<(
        String,
        uuid::Uuid,
        String,
        String,
    )>,
    axum::extract::Query(q): axum::extract::Query<MediaQuery>,
    headers: axum::http::HeaderMap,
) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    let rel = format!("files/renditions/{root_id}/{kind}/{file_id}");
    if let Some(refusal) = authorize_media(&state, &slug, &rel, q.token.as_deref(), &headers).await
    {
        return refusal;
    }
    let Some(kind) = files::TranscodeRenditionKind::from_tag(&kind) else {
        return (StatusCode::NOT_FOUND, "unknown rendition kind").into_response();
    };
    let Some(org) = state.org(&slug) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let total = match org.files.rendition_len(root_id, &file_id).await {
        Ok(n) => n,
        Err(files::FilesError::NotFound(_)) => {
            return (StatusCode::NOT_FOUND, "no such rendition").into_response();
        }
        Err(files::FilesError::BadRequest(m)) => {
            return (StatusCode::BAD_REQUEST, m).into_response();
        }
        Err(e) => {
            tracing::error!(%root_id, file_id, ?e, "rendition: stat failed");
            return (StatusCode::INTERNAL_SERVER_ERROR, "rendition store error").into_response();
        }
    };

    // Single byte range → 206; absent/malformed/unsatisfiable → full
    // body 200, still advertising range support (same contract as the
    // `/media` route — browser media elements need `Accept-Ranges` to
    // consider the source seekable at all).
    let range = headers
        .get(header::RANGE)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| parse_byte_range(s, total));
    rendition_stream_response(&org, root_id, &file_id, kind.mime(), total, range)
}

/// Stream a rendition's bytes (whole, or one byte range → 206) as a
/// response — shared by the org rendition route above and the share
/// serving routes (issue #271).
///
/// Streams, never buffers: a rendition can be a full-length proxy, and
/// `read_rendition_range` takes an `AsyncWrite` precisely so memory
/// stays bounded to one chunk. The status is already on the wire when
/// the read runs, so a mid-stream failure (e.g. the source-tied GC
/// sweeping this rendition between the caller's stat and here) can only
/// truncate the body — the client sees a short read against the
/// advertised Content-Length, not a 500.
pub(crate) fn rendition_stream_response(
    org: &OrgAppState,
    root_id: uuid::Uuid,
    file_id: &str,
    mime: &str,
    total: u64,
    range: Option<(u64, u64)>,
) -> axum::response::Response {
    use axum::http::{StatusCode, header};
    use axum::response::IntoResponse;
    let (start, len) = match range {
        Some((start, end)) => (start, end - start + 1),
        None => (0, total),
    };
    let (mut writer, reader) = tokio::io::duplex(64 * 1024);
    let files = org.files.clone();
    let read_file_id = file_id.to_string();
    tokio::spawn(async move {
        if let Err(e) = files
            .read_rendition_range(root_id, &read_file_id, start, len, &mut writer)
            .await
        {
            match e {
                files::FilesError::NotFound(_) => {
                    tracing::debug!(%root_id, file_id = read_file_id, "rendition: swept mid-stream");
                }
                other => {
                    tracing::warn!(%root_id, file_id = read_file_id, ?other, "rendition: ranged read failed");
                }
            }
        }
    });
    let body = axum::body::Body::from_stream(tokio_util::io::ReaderStream::new(reader));
    let base_headers = [
        (header::CONTENT_TYPE, mime.to_string()),
        (header::ACCEPT_RANGES, "bytes".to_string()),
        (header::CONTENT_LENGTH, len.to_string()),
    ];
    match range {
        Some((start, end)) => (
            StatusCode::PARTIAL_CONTENT,
            base_headers,
            [(
                header::CONTENT_RANGE,
                format!("bytes {start}-{end}/{total}"),
            )],
            body,
        )
            .into_response(),
        None => (StatusCode::OK, base_headers, body).into_response(),
    }
}

pub fn router(state: AppState) -> Router {
    use attachments::routes::AttachmentRouteState;
    use axum::routing::any;

    // Mount the /blobs/* HTTP routes against the FIRST org's
    // attachment service. Multi-org blob routing
    // (`/org/<slug>/blobs/...`) is a follow-up — the existing
    // path stays for single-org back-compat. When no org is
    // hosted (test boot path), fall back to a synthetic
    // empty router so axum doesn't choke.
    let blob_router = state
        .orgs
        .read()
        .ok()
        .and_then(|guard| guard.values().next().cloned())
        .map(|org| {
            let blob_state = AttachmentRouteState {
                service: org.attachments.clone(),
            };
            attachments::attachment_router().with_state(blob_state)
        })
        .unwrap_or_default();

    // Per-org vox at `/org/{slug}/vox`. Also keep `/vox`
    // and `/health` at the top level for back-compat —
    // `/vox` dispatches into the first hosted org so
    // single-org clients keep working without a URL change.
    let well_known = Router::new()
        .route("/.well-known/task-server.json", get(well_known_handler))
        .with_state(state.clone());
    // Forge webhook receiver — only a forge-carrying build has the
    // handler; without the plugin the route 404s like any unknown path.
    #[cfg(feature = "plugin-git")]
    let webhook_routes = Router::new()
        .route(
            "/org/{slug}/webhooks/forge",
            axum::routing::post(webhooks::forge_webhook_handler),
        )
        .with_state(state.clone());
    let per_org = Router::new()
        .route("/org/{slug}/health", get(per_org_health_handler))
        .route("/org/{slug}/api", get(per_org_api_handler))
        .route("/org/{slug}/vox", any(per_org_vox_handler))
        .route(
            "/org/{slug}/share/{token}",
            get(share::share_landing_handler),
        )
        // Files share serving (issue #271): scoped browse, view-only
        // renditions, capability-gated downloads with receipts.
        .route(
            "/org/{slug}/share/{token}/b/{*rel}",
            get(share::share_browse_handler),
        )
        .route(
            "/org/{slug}/share/{token}/rendition/{kind}/{*rel}",
            get(share::share_rendition_handler),
        )
        .route(
            "/org/{slug}/share/{token}/download/{*rel}",
            get(share::share_download_handler),
        )
        // The guest lane (issue #272): the real RPC surface over an
        // anonymous WebSocket, scoped to the link's Review.
        .route(
            "/org/{slug}/share/{token}/vox",
            any(share::share_guest_vox_handler),
        )
        // The file-request inbox (issue #272): uploads land in the
        // link's incoming area, never the tree. Media runs far past
        // axum's 2 MB default body cap.
        .route(
            "/org/{slug}/share/{token}/upload/{*name}",
            axum::routing::post(share::share_upload_handler)
                .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024 * 1024)),
        )
        // MCP — Task as a tool surface for agents (Hermes gateway,
        // Claude Code, any MCP client). See `mcp`.
        .route("/org/{slug}/mcp", axum::routing::post(mcp::mcp_handler))
        // Account-scoped MCP: one endpoint for every org the caller
        // can reach, instead of one registration per org.
        .route("/mcp", axum::routing::post(mcp::mcp_account_handler))
        .route("/org/{slug}/media/{*path}", get(per_org_media_handler))
        // Derived-rendition streaming for the Review page (issue #270).
        .route(
            "/org/{slug}/files/renditions/{root_id}/{kind}/{file_id}",
            get(files_rendition_handler),
        )
        .with_state(state.clone());

    // Server-management vox: `OrgManagementService` +
    // `SnapshotService` mounted on a top-level endpoint (not
    // per-org). Lets a CLI connect once and ask the server to
    // scaffold new orgs / run data snapshots without touching the
    // data root locally. `POST /server/snapshot` is the HTTP
    // trigger for the chart's backup CronJob (Bearer
    // `TASK_BACKUP_GIT_TOKEN`).
    let server_mgmt = Router::new()
        .route("/server/vox", any(server_vox_handler))
        .route(
            "/server/snapshot",
            axum::routing::post(snapshot::http_snapshot_handler),
        )
        .route(
            "/server/snapshot/status",
            get(snapshot::http_snapshot_status_handler),
        )
        // The permissions dry-run: coverage + "what would break if I
        // enforced right now" + the observed would-deny tally. Same
        // bearer as the snapshot routes (`TASK_BACKUP_GIT_TOKEN`), and
        // like them it is 503 when that token is unset — no new secret,
        // and nothing new is exposed on a default dev boot.
        .route("/server/permissions", get(permissions_report_handler))
        // The server profiling itself: a CPU flamegraph / pprof profile
        // and a per-thread CPU table. Gated by the operator rule the
        // MCP telemetry tools use (`operator::is_operator`), not the
        // backup bearer — these read the process, not the data root.
        .route("/server/debug/profile", get(debug_profile::profile_handler))
        .route("/server/debug/threads", get(debug_profile::threads_handler))
        .with_state(state.clone());

    let router = Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/vox", get(legacy_vox_handler))
        .merge(well_known)
        .merge(per_org)
        .merge(server_mgmt)
        .merge(watch_bridge::watch_router())
        .merge(blob_router);
    // Authenticated OTLP ingest, only when an upstream collector is
    // configured — see `otlp`. Clients export through here rather than to
    // a public collector endpoint.
    let router = match otlp::otlp_router() {
        Some(r) => router.merge(r),
        None => router,
    };
    #[cfg(feature = "plugin-git")]
    let router = router.merge(webhook_routes);

    // Files WebDAV bridge (issue #274) — mount an org's File Roots from
    // Finder/Explorer. `any` because WebDAV's verbs (PROPFIND, MKCOL,
    // MOVE, LOCK, …) are not in axum's method router, and all three
    // path shapes because a collection is addressed with a trailing
    // slash (`/dav/` is what a client PROPFINDs at mount time), which
    // axum's wildcard does not match — it needs at least one character
    // — while `/dav` without one is what a user types into the dialog.
    //
    // Merged AFTER `cors_layer()` so these routes sit outside it.
    // tower-http's `Cors` short-circuits *any* request whose method is
    // `OPTIONS` — it does not require an `Origin` or
    // `Access-Control-Request-Method` header — returning a bare 200 and
    // never calling the inner service. Finder/Explorer/gvfs begin a
    // mount with exactly that: `OPTIONS /org/{slug}/dav/` with no CORS
    // headers, and they read `DAV:` and `Allow:` off the response to
    // decide whether this is a WebDAV server at all. Under the layer
    // they got a bare 200 with neither header and refused to mount, and
    // `webdav_handler` was never reached — so nothing authenticated
    // either. WebDAV clients are not browsers; CORS has nothing to say
    // about them (PR #287 review).
    let dav = Router::new()
        .route("/org/{slug}/dav", any(webdav::webdav_handler))
        .route("/org/{slug}/dav/", any(webdav::webdav_handler))
        .route("/org/{slug}/dav/{*path}", any(webdav::webdav_handler))
        .with_state(state.clone());

    router.layer(cors_layer()).merge(dav).with_state(state)
}

/// CORS policy. **Default is unchanged**: with `TASK_CORS_ALLOWED_ORIGINS`
/// unset (or `*`) this is the same `CorsLayer::permissive()` the server has
/// always used — any origin, any method, any header — plus a startup
/// warning, because "any origin" on an internet-reachable server that
/// accepts bearer tokens is a policy nobody chose on purpose.
///
/// Set it to a comma-separated origin list
/// (`https://task.example,https://app.example`) to restrict the `Origin`
/// allowlist; methods and headers stay permissive so nothing else about
/// the surface changes. Credentials are never allowed automatically —
/// Task authenticates with bearer tokens in vox metadata, not cookies, so
/// `Access-Control-Allow-Credentials` is not needed for the app to work.
fn cors_layer() -> tower_http::cors::CorsLayer {
    use axum::http::HeaderValue;
    use tower_http::cors::{AllowOrigin, CorsLayer};

    let raw = std::env::var("TASK_CORS_ALLOWED_ORIGINS").unwrap_or_default();
    let raw = raw.trim();
    let permissive = |why: &str| {
        tracing::warn!(
            var = "TASK_CORS_ALLOWED_ORIGINS",
            reason = why,
            "CORS is PERMISSIVE — every origin may call this server. Set \
             TASK_CORS_ALLOWED_ORIGINS to a comma-separated origin list on \
             any internet-reachable deployment.",
        );
        CorsLayer::permissive()
    };
    if raw.is_empty() {
        return permissive("unset");
    }
    if raw == "*" {
        return permissive("set to `*`");
    }
    let origins: Vec<HeaderValue> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| match HeaderValue::from_str(s) {
            Ok(v) => Some(v),
            Err(_) => {
                tracing::warn!(origin = s, "ignoring unparseable CORS origin");
                None
            }
        })
        .collect();
    if origins.is_empty() {
        // Better a loud fall-back to today's behaviour than a server that
        // silently refuses every browser after a typo in the var.
        return permissive("no parseable origins in the list");
    }
    tracing::info!(
        origins = %raw,
        count = origins.len(),
        "CORS restricted to the configured origin allowlist",
    );
    CorsLayer::permissive().allow_origin(AllowOrigin::list(origins))
}

/// `GET /server/permissions` — the enforcement dry-run.
///
/// Answers "if I set `TASK_ENFORCE_PERMISSIONS=1` right now, what would
/// break?" from three angles: permit-table coverage, a static replay of
/// every permit through the live engine, and the tally of denials the
/// running server has actually observed. Auth + the 503-when-unconfigured
/// behaviour are the snapshot routes' (`TASK_BACKUP_GIT_TOKEN`).
async fn permissions_report_handler(headers: axum::http::HeaderMap) -> axum::response::Response {
    if let Err(resp) = snapshot::check_backup_auth(&headers) {
        return *resp;
    }
    let engine = org_permission_engine();
    axum::Json(permits::report_json(
        enforce_permissions(),
        engine.as_ref(),
        &permission_deny_ledger(),
    ))
    .into_response()
}

/// Does the bearer belong to `slug` by way of the home org?
///
/// Validates the token against the home org's auth store (this server's
/// identity authority) and then requires a membership row. False on
/// every failure — no home identity, an unreadable table, a token the
/// home org does not know — because discovery must never claim
/// membership the org lane would then refuse.
async fn home_membership(state: &AppState, token: &str, slug: &str) -> bool {
    let Some(home) = &state.home_identity else {
        return false;
    };
    // Who is asking: a home-org session, or — when the server delegates
    // identity — an account the issuer vouches for. `home_principal` is
    // the one place that answers this for every home-org lane.
    let Some(user_id) = central_auth::home_principal(state, token).await else {
        return false;
    };
    home.memberships
        .role_for(user_id, slug)
        .await
        .is_ok_and(|m| m.is_some())
}

/// `.well-known/task-server.json` — federation discovery.
/// Lists every org this server hosts plus its routing URL
/// suffix. Public, no auth required.
///
/// Peers fetch this to learn what slugs are available on a federation host
/// before opening a vox connection.
async fn well_known_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
) -> axum::Json<serde_json::Value> {
    // Membership, when the caller says who they are. Each org has its OWN
    // auth database (`AppState::new` opens one `AuthState` per
    // `org_root.auth_db()`), so "orgs I belong to" is exactly "orgs where
    // my token validates" — no cross-org membership table to consult, and
    // a token from one org is simply unknown to another.
    //
    // The list itself is NOT filtered here. A client must be able to see
    // an org before it can sign into it, and discovery runs before any
    // session exists; filtering server-side would make sign-in
    // unreachable. Instead each entry is tagged, and the client uses the
    // tag to decide what "All organizations" means (issue #109 criterion
    // 6). Enforcement of actual data access is the permission gate's job
    // on the org lane, not this endpoint's.
    let bearer = crate::watch_bridge::bearer(&headers);
    // Who the bearer is, learned once while tagging membership. Handed
    // back as `principal` so a client holding a cached token can restore
    // its session from this one response instead of validating the token
    // again against the org lane and then the issuer — the chain that
    // made a warm reload take five seconds.
    let mut principal: Option<serde_json::Value> = None;
    let mut orgs: Vec<serde_json::Value> = Vec::new();
    for slug in state.org_slugs() {
        let Ok(manifest) = state.data_root.org(slug.as_str()).manifest() else {
            continue;
        };
        let member = match &bearer {
            None => None,
            Some(token) => {
                // The org's own store first: a session it minted is a
                // member by definition, and the bundle names the account.
                let own = match state.org(&slug) {
                    Some(org) => org
                        .auth
                        .auth
                        .current_session(architect_auth::CurrentSession {
                            token: token.clone(),
                        })
                        .await
                        .ok(),
                    None => None,
                };
                match own {
                    Some(bundle) => {
                        principal.get_or_insert_with(|| {
                            serde_json::json!({
                                "id": bundle.user.id,
                                "email": bundle.user.email,
                                "name": bundle.user.name,
                                "via": "org",
                            })
                        });
                        Some(true)
                    }
                    None => Some(home_membership(&state, token, &slug).await),
                }
            }
        };
        orgs.push(serde_json::json!({
            "slug": slug,
            "id": manifest.id,
            "display_name": manifest.display_name,
            "is_home": manifest.is_home,
            "federation_url": manifest.federation_url,
            // Org-level config, not secret (the doc already carries
            // schema stamps): the client shell hides a disabled
            // plugin's nav/widgets/routes from this list.
            "disabled_plugins": manifest.disabled_plugins.0,
            "member": member,
            "vox": format!("/org/{slug}/vox"),
            "health": format!("/org/{slug}/health"),
            // The org's iroh endpoint id — the whole of its non-HTTP
            // address, written by `iroh_host` as the endpoint binds and
            // stable across restarts (the key beside it is persisted).
            // Discovery is how a native client learns to dial the org
            // over iroh instead of this WebSocket; `null` until the
            // first bind, or when iroh is disabled.
            "iroh": std::fs::read_to_string(
                state.data_root.org(slug.as_str()).path().join("iroh-endpoint-id"),
            )
            .ok()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty()),
        }));
    }
    // A token no org here minted may still be the issuer's — the same
    // resolution the org lane does, answered from its cache after the
    // first ask.
    if principal.is_none()
        && let Some(token) = &bearer
        && let Some(central) = central_auth::configured()
        && let Some(profile) = central.profile_for(token).await
    {
        principal = Some(serde_json::json!({
            "id": profile.user_id,
            "email": profile.email,
            "name": profile.name,
            "via": "issuer",
        }));
    }
    // Schema stamps — the proto/server skew guard. Clients
    // (`task doctor`) compare these against their own build;
    // see `schema_stamps`.
    let stamps: serde_json::Map<String, serde_json::Value> = schema_stamps()
        .into_iter()
        .map(|(name, stamp)| (name.to_owned(), serde_json::Value::String(stamp)))
        .collect();
    axum::Json(serde_json::json!({
        "version": 1,
        // Git rev this binary was built from (baked into the container
        // image env by the flake; "unknown" outside that path). CI's
        // verify-live step polls this until it matches the pushed sha —
        // a green run means the deployment is actually serving it.
        "build": std::env::var("TASK_BUILD_REV").unwrap_or_else(|_| "unknown".to_owned()),
        "orgs": orgs,
        // The account the bearer resolved to (`null` without one, or when
        // nothing here or at the issuer recognised it). `via` says which.
        "principal": principal,
        "schema_stamps": stamps,
        // Where accounts come from, when they do not come from here.
        //
        // Absent (`null`) on a self-hosted server, which is the default
        // and means what it always meant: sign in against the home org.
        // Present, and a client signs in *there* instead — one account
        // across every FastTrackStudio app rather than one per org.
        //
        // It belongs in discovery because the client has to know before
        // it has a session, and discovery is the only thing it fetches
        // before one exists. Public: an issuer URL is not a secret, and
        // a client that cannot learn it cannot sign in at all.
        "central_auth": crate::central_auth::configured().map(|c| c.issuer().to_owned()),
    }))
}

/// `/org/<slug>/health` — per-org liveness probe. `200 ok`
/// when the slug is hosted, `404` otherwise.
async fn per_org_health_handler(
    State(state): State<AppState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> axum::response::Response {
    if state.org(&slug).is_some() {
        axum::response::IntoResponse::into_response("ok")
    } else {
        axum::response::IntoResponse::into_response((
            axum::http::StatusCode::NOT_FOUND,
            format!("org `{slug}` not hosted"),
        ))
    }
}

/// `/org/<slug>/api` — the self-describing API reference:
/// [`permits::mounts()`] serialized (every service, its methods + arg
/// names, the permit action/resource per method, stream-ness, and the
/// per-service schema stamp). See [`api_ref`].
///
/// **Auth posture**: public, exactly like `/org/{slug}/health` and
/// `/.well-known/task-server.json` — the precedent. The well-known
/// document already publishes the per-service `schema_stamps` without
/// auth (federation discovery), and this endpoint adds only more
/// build-static metadata of the same kind (names + permit templates —
/// no org data, no tokens, nothing runtime). `404` when the slug is
/// not hosted, mirroring the health probe.
async fn per_org_api_handler(
    State(state): State<AppState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
) -> axum::response::Response {
    let Some(org) = state.org(&slug) else {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::NOT_FOUND,
            format!("org `{slug}` not hosted"),
        ));
    };
    // The org's plugin set decides the per-service `mounted` flag and
    // the top-level `plugins` state; the catalog itself stays complete
    // (a disabled service is listed with `"mounted": false`).
    let mut body = api_ref::reference_json_for(&org.plugins);
    if let Some(obj) = body.as_object_mut() {
        obj.insert("org".to_owned(), serde_json::Value::String(slug));
    }
    axum::Json(body).into_response()
}

/// `/org/<slug>/vox` — per-org vox WebSocket. Looks up the
/// slug in the AppState's org map; rejects with 404 if the
/// org isn't hosted.
async fn per_org_vox_handler(
    State(state): State<AppState>,
    axum::extract::Path(slug): axum::extract::Path<String>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    let Some(org) = state.org(&slug) else {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::NOT_FOUND,
            format!("org `{slug}` not hosted"),
        ));
    };
    serve_org_vox(org, state.write_gate.clone(), ws, &headers)
}

/// `/server/vox` — server-management WebSocket. Hosts the
/// `OrgManagementService`. Unauthenticated requests are
/// allowed in bootstrap mode (no orgs hosted yet); after that
/// the service itself rejects requests whose `session_token`
/// doesn't validate against the home org.
async fn server_vox_handler(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    let gate = state.write_gate.clone();
    let router = server_layer_router(&state, false);
    // Gated like the per-org endpoints: `create_org` writes to the
    // data root, so it must quiesce during a snapshot too. The
    // snapshot verbs themselves pass the entry gate before closing
    // it — no self-deadlock.
    let router = crate::snapshot::GatedRouter::new(router, gate);
    // The `/server/vox` services take the session token as an explicit
    // ARGUMENT (the identity locker validates it itself), so there is no
    // gate to feed here — but the subprotocol must still be echoed, or a
    // browser that offered one gets no connection at all.
    ws.protocols([VOX_SUBPROTOCOL])
        .on_upgrade(move |socket| architect::axum_ws::serve_router(socket, router))
        .into_response()
}

/// Build the server-management [`LayerRouter`] (`OrgManagementService`
/// and `SnapshotService`) — the `/server/vox` service set. Shared by the
/// WebSocket handler above (which additionally wraps it in the
/// snapshot [`GatedRouter`](crate::snapshot::GatedRouter)) and the
/// in-process transport ([`AppState::server_local_server`]).
///
/// `local_trusted`: false for the network-facing WebSocket (session
/// auth enforced, restore exits so the supervisor restarts on the
/// restored data); true for the in-process transport (the caller
/// already owns the data root — no session check, no exit-on-restore
/// since the embedded CLI process is ephemeral).
#[must_use]
pub fn server_layer_router(state: &AppState, local_trusted: bool) -> architect::LayerRouter {
    let (mgmt, snap, identity) = if local_trusted {
        (
            crate::server_mgmt::OrgManagementImpl::new_local_trusted(state.clone()),
            crate::snapshot::SnapshotImpl::new_local_trusted(state.clone()),
            crate::identity_mgmt::IdentityServiceImpl::new_local_trusted(state.clone()),
        )
    } else {
        (
            crate::server_mgmt::OrgManagementImpl::new(state.clone()),
            crate::snapshot::SnapshotImpl::new(state.clone()),
            crate::identity_mgmt::IdentityServiceImpl::new(state.clone()),
        )
    };
    // The Files placement layer's operator + agent lanes (issue #262).
    // Both are deployment-scoped, so they belong here rather than on any
    // org router: the operator registers locations and admits orgs onto
    // them, and Storage agents enroll, heartbeat, take directives and
    // report outcomes.
    //
    // Neither is unauthenticated. `/server/vox` has no permission gate
    // in front of it, so — exactly like the three services below — the
    // operator lane validates a session token itself (against the home
    // org, or trusting the in-process transport), and the agent lane
    // requires the per-agent enrollment secret on every call after
    // enrollment (PR #284 review).
    let storage_admin = if local_trusted {
        files_storage::StorageAdminBackend::new_local_trusted(state.storage.clone())
    } else {
        files_storage::StorageAdminBackend::new(
            state.storage.clone(),
            std::sync::Arc::new(crate::storage::HomeOrgOperator::new(state.clone())),
        )
    };

    architect::LayerRouter::new()
        .with(
            org_proto::org_management_descriptor(),
            org_proto::serve_org_management(mgmt),
        )
        .with(
            org_proto::snapshot_descriptor(),
            org_proto::serve_snapshot(snap),
        )
        .with(
            identity_proto::identity_descriptor(),
            identity_proto::serve_identity(identity),
        )
        .with(
            files_storage::storage_admin_descriptor(),
            files_storage::serve_storage_admin(storage_admin),
        )
        .with(
            files_storage::storage_agent_descriptor(),
            files_storage::serve_storage_agent(files_storage::StorageAgentBackend::new(
                state.storage.clone(),
            )),
        )
        .merge(files_storage::storage_agent_stream_layer(
            files_storage::StorageAgentBackend::new(state.storage.clone()),
        ))
}

/// `/vox` — legacy single-org alias. Dispatches into the
/// first hosted org so clients written against the
/// pre-multi-org URL keep working without a redirect.
/// Returns 503 when no org is hosted (which shouldn't
/// happen post-boot but is a sane fallback).
async fn legacy_vox_handler(
    State(state): State<AppState>,
    headers: axum::http::HeaderMap,
    ws: WebSocketUpgrade,
) -> axum::response::Response {
    let Some(org) = state
        .orgs
        .read()
        .ok()
        .and_then(|guard| guard.values().next().cloned())
    else {
        return axum::response::IntoResponse::into_response((
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "no org hosted on this server",
        ));
    };
    serve_org_vox(org, state.write_gate.clone(), ws, &headers)
}

/// Build the per-org [`LayerRouter`]: every service this org hosts,
/// mounted by its descriptor onto one router. One connection then
/// multiplexes all of them — the client's establish handshake names
/// the service and the router dispatches by method id.
///
/// This replaces the old per-connection `match req.service()` acceptor
/// with architect's composable layer system; the same router is reused
/// for the WebSocket transport here and the in-process `LocalServer`
/// transport (see [`org_local_server`]).
/// The role every validated user gets on the org lane until per-row
/// membership sync lands.
pub const DEFAULT_ORG_ROLE: &str = "member";

/// Is the gate enforcing? **Off unless `TASK_ENFORCE_PERMISSIONS` is
/// exactly `1`** — the deliberate operator action. Everything else
/// (unset, empty, `true`, `yes`) leaves the gate in observe-only mode,
/// exactly as before permit tables existed.
#[must_use]
pub fn enforce_permissions() -> bool {
    std::env::var("TASK_ENFORCE_PERMISSIONS").is_ok_and(|v| v == "1")
}

/// Server-wide tally of every (would-be) denial the gate produced. Read
/// by `GET /server/permissions`; written by [`permits::GateAudit`].
#[must_use]
pub fn permission_deny_ledger() -> Arc<permits::DenyLedger> {
    static LEDGER: std::sync::LazyLock<Arc<permits::DenyLedger>> =
        std::sync::LazyLock::new(Arc::default);
    LEDGER.clone()
}

/// The org lane's permission engine: the role engine (every validated
/// user is a [`DEFAULT_ORG_ROLE`]) plus a scope engine granting
/// [`permits::PUBLIC_GLOB`] to EVERY principal — including anonymous
/// ones, which is how the sign-in path stays reachable once enforcement
/// is on (see `permits`' module docs). First-allow-wins, so the public
/// rules only ever widen.
#[must_use]
pub fn org_permission_engine() -> Arc<dyn architect_permissions::PermissionEngine> {
    let roles = architect_permissions::RoleEngine::new().with_default_user_role(DEFAULT_ORG_ROLE);
    let public = architect_permissions::ScopeEngine::new(vec![architect_permissions::Rule::new(
        permits::PUBLIC_GLOB,
        &["*"],
    )]);
    Arc::new(
        architect_permissions::CompositeEngine::new()
            .push(Arc::new(roles))
            .push(Arc::new(public))
            // An admitted host, holding the replica lane and nothing
            // else. Last because first-allow-wins and this only ever
            // widens — and it widens for a principal the other two
            // refuse outright.
            .push(Arc::new(permits::HostEngine)),
    )
}

/// Build the org lane's permission gate: session-validating identity over
/// THIS org's auth store, the [`org_permission_engine`], and a permit
/// table for EVERY service [`org_layer_router`] mounts for this org's
/// plugin set (see [`permits`] — disabled plugins' services are neither
/// mounted nor tabled).
///
/// **Runtime defaults are unchanged**: `UnlistedPolicy::Allow` and
/// `observe_only(!enforce)` with `enforce` false unless
/// `TASK_ENFORCE_PERMISSIONS=1`. Installing tables changes what is
/// *evaluated and logged*, never what is refused, while the gate is in
/// observe-only mode — [`architect::permissions_gate::PermissionedRouter`]
/// dispatches to the inner router on every outcome when `observe_only` is
/// set.
fn build_org_permissions_gate(
    auth: &AuthState,
    plugins: &task_plugin::PluginSet,
    slug: &str,
    home_identity: Option<&HomeIdentity>,
    files: &files::FilesBackend,
) -> architect::permissions_gate::PermissionsGate {
    use architect::permissions_gate::{PermissionsGate, UnlistedPolicy};
    let own = architect_auth::identity::SessionIdentityResolver::new(auth.auth.clone());
    // Cross-org identity: a token this org does not know may still be a
    // home-org token belonging to a principal with a membership row for
    // this org. Not installed on the home org itself (its own resolver
    // already IS the home resolver) or when there is no home identity.
    let resolver: Arc<dyn architect_permissions::IdentityResolver + Send + Sync> =
        match home_identity {
            Some(home) if home.slug != slug => Arc::new(permits::AuditedIdentityResolver::new(
                permits::HomeFallbackResolver::new(
                    own,
                    architect_auth::identity::SessionIdentityResolver::new(home.auth.auth.clone()),
                    Arc::clone(&home.memberships),
                    slug,
                ),
                slug,
            )),
            // Wrapped so every RPC's wide event carries WHY the principal came out
            // the way it did — "no token presented" vs "token rejected" are the
            // same `Principal::Anonymous` without it (see `AuditedIdentityResolver`).
            _ => Arc::new(permits::AuditedIdentityResolver::new(own, slug)),
        };
    // Central identity: a token no local store knows may still be a real
    // FastTrackStudio account. Only when an issuer is configured and this
    // server has a membership table to check against — see
    // `central_auth`, which is a no-op on a self-hosted server that has
    // not opted in.
    let resolver = match (central_auth::configured(), home_identity) {
        (Some(central), Some(home)) => Arc::new(central_auth::CentralFallbackResolver::new(
            resolver,
            Arc::clone(central),
            Arc::clone(&home.memberships),
            slug,
        ))
            as Arc<dyn architect_permissions::IdentityResolver + Send + Sync>,
        // Configured but no home identity means no membership table, and
        // the table is the whole fence — admitting on identity alone
        // would let one account reach every org here. Say so once rather
        // than failing quietly for every sign-in.
        (Some(_), None) => {
            tracing::warn!(
                org.slug = slug,
                "central auth is configured but this server has no home identity — \
                 central accounts cannot be admitted without a membership table \
                 (`admin adopt-principal`)"
            );
            resolver
        }
        (None, _) => resolver,
    };
    // Outermost: a `host:<endpoint-id>` bearer resolves against this
    // org's admitted set instead of its auth store. Non-host bearers
    // fall straight through, so nothing about a person's sign-in
    // changes.
    let identity: Arc<dyn architect_permissions::IdentityResolver + Send + Sync> =
        Arc::new(permits::HostResolver::new(resolver, files.clone(), slug));
    let enforce = enforce_permissions();
    let audit = permits::GateAudit::new(enforce, permission_deny_ledger(), DEFAULT_ORG_ROLE);
    let gate = PermissionsGate::new(org_permission_engine(), Arc::new(identity))
        .with_audit(Arc::new(audit))
        .unlisted(UnlistedPolicy::Allow)
        .observe_only(!enforce);
    permits::install_for(gate, plugins)
}

/// Schema stamps for every vox service [`org_layer_router`]
/// mounts — the dev guard against proto/server skew (served in
/// `/.well-known/task-server.json` as `schema_stamps`). A vox
/// method id hashes the method's name + payload shapes, so a
/// stamp diff between a client's build and the *running* server
/// binary means one of them predates a `*-proto` change — the
/// "structural mismatch / InvalidPayload out of nowhere" failure
/// mode. `task doctor` (which links this very function through
/// the task-server crate, so the two lists can't drift) compares
/// against this map and says "rebuild task-server" instead of
/// letting the skew surface as decode errors.
///
/// The descriptor list is [`permits::mounted_descriptors`] — the
/// SAME list the permit gate folds, so stamps and permits can no
/// longer drift from each other (they used to be two hand-kept
/// copies). `permits_cover_router` asserts it matches what
/// [`org_layer_router`] actually mounts.
#[must_use]
pub fn schema_stamps() -> Vec<(&'static str, String)> {
    org_proto::schema_stamp::stamp_services(permits::mounted_descriptors())
}

pub fn org_layer_router(org: &OrgAppState) -> architect::LayerRouter {
    use architect::LayerRouter;

    // Per-org plugin gate: a disabled plugin's services are simply not
    // mounted, so a wire call fails at dispatch with unknown-service —
    // the same failure an old client gets from a server that never had
    // the feature. Grouping mirrors `permits::mounts()` (the `plugin`
    // field there is the single source of truth; `permits_cover_router`
    // asserts the two views agree for any set). No deny-list = every
    // branch taken = exactly the pre-plugin router.
    // Unused only in a core-only build (every plugin compiled out).
    #[allow(unused_variables)]
    let on = |id: &str| org.plugins.contains(id);

    let mut router = LayerRouter::new()
        // Auth — wrapped with the server middleware that validates
        // session tokens before the inner service sees the request.
        .with(
            architect_auth::auth_service_service_descriptor(),
            AuthServiceDispatcher::new(AuthVoxService::new(org.auth.auth.clone()))
                .with_middleware(AuthServerMiddleware),
        )
        // Attachments — signed blob upload/download.
        .merge(attachments_proto::layer((*org.attachments).clone()))
        // Attachment blobs streamed over vox (`Tx<MediaChunk>`), no
        // HTTP side-channel — the session player's stems and large
        // media, addressed by content hash.
        //
        // Distinct from `files_proto`'s byte lane below, which reads
        // *file roots* by root and path. Both traits were called
        // `MediaService` until now, and both `#[vox::service]` and
        // `#[architect::rpc]` derive their wire identity from the trait
        // name, so the two could not be mounted together. Only one ever
        // was, which is why the clash stayed invisible until the files
        // byte lane was wired up.
        .merge(media_proto::layer(
            crate::media::AttachmentMediaServiceImpl::new(
                org.attachments.clone(),
                org.slug.clone(),
                // Same process-wide keypair the media route verifies
                // with (`AppState::keypair`, threaded through
                // `build_org_state`) — sign and verify cannot drift.
                org.attachments.keypair.clone(),
            ),
        ))
        // Replica sync: the commit graph and the chunks under it, which
        // is how a second host of this org converges its structure
        // (`files.peering.replication`). Mounted here rather than left
        // to whoever happened to be serving — a peer dials the org's
        // endpoint and expects the org's router.
        // The org places each root it offers (`<org>/Wiki/<slug>`,
        // read-only `Subscribed/` and `Resources/`), so a device's mount
        // composes from what the layout says rather than from places
        // typed per machine.
        .merge(files_sync::layer(
            files_sync::SyncHost::new(org.files.clone())
                .placing(crate::org_roots::OrgPlacer::new(org.org_root.clone())),
        ))
        // Vault file replication (manifest / get / put / delete).
        .with(
            vault_proto::descriptor(),
            vault_proto::serve(org.vault_sync.clone()),
        )
        // Live vault changes — `VaultSync`'s `#[subscribe]` stream
        // sibling. The hub lives on the `vault::Backend` above, so
        // every path publishes into it: wire PUT/DELETE/set_folder,
        // in-process writers holding a backend clone, and the
        // filesystem watcher (external edits from vim / Obsidian /
        // `git pull`).
        .merge(vault_proto::stream_layer(org.vault_sync.clone()))
        // Permissions oracle — the caller's capability manifest, answered
        // by the SAME engine + identity the org lane's gate enforces with.
        .with(
            architect_permissions_proto::permissions_service_service_descriptor(),
            architect_permissions_proto::PermissionsServiceDispatcher::new(
                architect_permissions_proto::Permissions::new(
                    org.permissions.engine(),
                    org.permissions.identity_resolver(),
                ),
            ),
        )
        // Share links — CRUD for the note Share panel + Links registry.
        .with(
            share_proto::share_service_service_descriptor(),
            share_proto::ShareServiceDispatcher::new(share::ShareServiceImpl::new(
                org.shares.clone(),
                org.slug.clone(),
                share_public_base(),
                Some(org.files.clone()),
            )),
        );

    // ── Agent plugin ─────────────────────────────────────────────
    #[cfg(feature = "plugin-agent")]
    if on("agent") {
        router = router
            // Agent-task queue — slim domain trait (claim / complete / set-status).
            .with(
                agent_proto::service::tasks::agent_task_queue_rpc_service_descriptor(),
                agent_proto::service::tasks::serve(org.agent_tasks.clone()),
            )
            // Agent sessions — conversation lifecycle (list / read /
            // create / rename / pin / archive). Backs the `/agents`
            // sidebar listing. Served by the in-process Codex backend.
            .with(
                agent_proto::service::sessions::sessions_rpc_service_descriptor(),
                agent_proto::service::sessions::serve(org.agent_router.clone()),
            )
            // Agent turn dispatch — kick off / cancel / resume a turn
            // on a session. Served by the same Codex backend.
            .with(
                agent_proto::service::turn_dispatch::turn_dispatch_rpc_service_descriptor(),
                agent_proto::service::turn_dispatch::serve(org.agent_router.clone()),
            )
            // Agent threads — conversation threading within a session.
            // Served by the same Codex backend (impls Threads).
            .with(
                agent_proto::service::threads::threads_rpc_service_descriptor(),
                agent_proto::service::threads::serve(org.agent_router.clone()),
            )
            // Live agent events — the `#[subscribe]` stream over the hub
            // both agent backends publish into. One subscription per
            // client; the envelope's `session_id` is the filter.
            .merge(agent_proto::service::subscriptions::stream_layer(
                org.agent_router.clone(),
            ))
            // Agent discovery — live model/skill/capability lists for the
            // chat UI's pickers and inspector panel.
            .with(
                agent_proto::service::discovery::discovery_rpc_service_descriptor(),
                agent_proto::service::discovery::serve(org.agent_router.clone()),
            )
            // Agent routines — the gateway's scheduled runs, surfaced as
            // a first-class Task feature.
            .with(
                agent_proto::service::routines::routines_rpc_service_descriptor(),
                agent_proto::service::routines::serve(org.agent_router.clone()),
            )
            // Runner registry — who can execute agent work, what they
            // can do, and whether they have heartbeated recently
            // enough to be offered any.
            .with(
                agent_proto::service::backends::backends_rpc_service_descriptor(),
                agent_proto::service::backends::serve(org.agent_runners.clone()),
            )
            // Run records — every attempt at every ticket, so retry
            // history and leftover worktrees are both answerable.
            .with(
                agent_proto::service::runs::runs_rpc_service_descriptor(),
                agent_proto::service::runs::serve(org.agent_runs.clone()),
            )
            // The grill queue — questions agents are blocked on, and
            // the answers that unblock them.
            .with(
                agent_proto::service::questions::questions_rpc_service_descriptor(),
                agent_proto::service::questions::serve(org.agent_questions.clone()),
            )
            // Live run state — the snapshot half…
            .with(
                agent_proto::service::run_stream::run_stream_rpc_service_descriptor(),
                agent_proto::service::run_stream::serve(org.agent_runs.clone()),
            )
            // …and the stream half, off the hub the run store owns.
            .merge(agent_proto::service::run_stream::stream_layer(
                org.agent_runs.clone(),
            ));
    }

    router = router
        // Timer — billable time tracking.
        .with(
            timer_proto::service::timer_service_rpc_service_descriptor(),
            timer_proto::service::serve(org.timer.clone()),
        )
        // Live session changes — `TimerService`'s `#[subscribe]`
        // stream sibling, served from the hub on the timer `Store`
        // above. Sessions only; rate edits don't stream.
        .merge(timer_proto::timer_service_stream_layer(org.timer.clone()))
        .with(
            threads::service::threads_service_rpc_service_descriptor(),
            threads::service::serve(org.threads.clone()),
        )
        // Per-user preferences — get-with-defaults / upsert set.
        .with(
            prefs_proto::service::prefs_service_rpc_service_descriptor(),
            prefs_proto::service::serve(org.prefs.clone()),
        );

    // ── Scheduling plugin ────────────────────────────────────────
    #[cfg(feature = "plugin-scheduling")]
    if on("scheduling") {
        router = router
            // Scheduling — day templates (drives the calendar overlay)
            // + per-date day plans (the day-by-day editor).
            .with(
                scheduling_proto::service::day_templates::day_templates_rpc_service_descriptor(),
                scheduling_proto::service::day_templates::serve(org.scheduling.clone()),
            )
            .with(
                scheduling_proto::service::day_plans::day_plans_rpc_service_descriptor(),
                scheduling_proto::service::day_plans::serve(org.scheduling.clone()),
            )
            .with(
                scheduling_proto::service::calendar_events::calendar_events_rpc_service_descriptor(
                ),
                scheduling_proto::service::calendar_events::serve(org.scheduling.clone()),
            )
            // Scheduling — booking half (Cal.com-style): event types,
            // availability schedules, open-slot listing, and bookings.
            // All four are served by the same `VaultScheduler`.
            .with(
                scheduling_proto::service::event_types::event_types_rpc_service_descriptor(),
                scheduling_proto::service::event_types::serve(org.scheduling.clone()),
            )
            .with(
                scheduling_proto::service::schedules::schedules_rpc_service_descriptor(),
                scheduling_proto::service::schedules::serve(org.scheduling.clone()),
            )
            .with(
                scheduling_proto::service::slots::slots_rpc_service_descriptor(),
                scheduling_proto::service::slots::serve(org.scheduling.clone()),
            )
            .with(
                scheduling_proto::service::bookings::bookings_rpc_service_descriptor(),
                scheduling_proto::service::bookings::serve(org.scheduling.clone()),
            )
            // Live scheduling changes — the slice's ONE `#[subscribe]`
            // stream (day templates / day plans / calendar events /
            // event types / schedules / bookings), served from the hub
            // on the `VaultScheduler` above. The event names which
            // sub-resource changed; subscribers filter client-side.
            .merge(scheduling_proto::scheduling_events_stream_layer(
                org.scheduling.clone(),
            ));
    }

    router = router
        .with(
            inbox_proto::service::inbox::inbox_rpc_service_descriptor(),
            inbox_proto::service::inbox::serve(org.inbox.clone()),
        )
        // Live inbox changes — `Inbox`'s `#[subscribe]` stream
        // sibling, served from the hub on the `VaultInbox` above.
        .merge(inbox_proto::inbox_stream_layer(org.inbox.clone()));

    router = router
        // Notifications — the bell's queue (list / mark_read /
        // mark_all_read / delete). Rows are written by the in-process
        // notifier (`crate::notifier`), not by clients.
        .with(
            notify_proto::notify_rpc_service_descriptor(),
            notify_proto::notify_serve(org.notify.clone()),
        )
        // Live notification changes — `Notify`'s `#[subscribe]`
        // stream sibling, served from the hub on the notify `Store`
        // above. The bell folds these (fetch-once-then-fold).
        .merge(notify_proto::notify_stream_layer(org.notify.clone()));

    // ── Recall plugin ────────────────────────────────────────────
    #[cfg(feature = "plugin-recall")]
    if on("recall") {
        router = router
            .with(
                recall_proto::service::recall::recall_rpc_service_descriptor(),
                recall_proto::service::recall::serve(org.recall.clone()),
            )
            // Live deck changes — `Recall`'s `#[subscribe]` stream
            // sibling, served from the hub on the `VaultRecall` above.
            .merge(recall_proto::recall_stream_layer(org.recall.clone()));
    }

    // ── Contacts plugin ──────────────────────────────────────────
    #[cfg(feature = "plugin-contacts")]
    if on("contacts") {
        router = router
            .with(
                contacts_proto::service::contacts::contacts_rpc_service_descriptor(),
                contacts_proto::service::contacts::serve(org.contacts.clone()),
            )
            // Live directory changes — `Contacts`' `#[subscribe]` stream
            // sibling, served from the hub on the `VaultContacts` above
            // (contacts only; account edits don't stream).
            .merge(contacts_proto::contacts_stream_layer(org.contacts.clone()));
    }

    router = router.with(
        tag_proto::service::tags::tag_service_rpc_service_descriptor(),
        tag_proto::service::tags::serve(org.tags.clone()),
    );

    // ── Finance plugin ───────────────────────────────────────────
    #[cfg(feature = "plugin-finance")]
    if on("finance") {
        router = router
            .with(
                finance_proto::service::invoicing::invoicing_rpc_service_descriptor(),
                finance_proto::service::invoicing::serve(org.finance_backend.clone()),
            )
            .with(
                finance_proto::service::ledger::ledger_rpc_service_descriptor(),
                finance_proto::service::ledger::serve(org.ledger_backend.clone()),
            );
    }

    // ── Wiki plugin — 11 per-capability traits, one descriptor each.
    #[cfg(feature = "plugin-wiki")]
    if on("wiki") {
        let wiki = org.wiki.clone();
        router = router
            .with(
                wiki_proto::service::schema::schema_rpc_service_descriptor(),
                wiki_proto::service::schema::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::catalog::catalog_rpc_service_descriptor(),
                wiki_proto::service::catalog::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::raw_layer::raw_layer_rpc_service_descriptor(),
                wiki_proto::service::raw_layer::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::graph::graph_rpc_service_descriptor(),
                wiki_proto::service::graph::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::pages::pages_rpc_service_descriptor(),
                wiki_proto::service::pages::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::subscriptions::subscriptions_rpc_service_descriptor(),
                wiki_proto::service::subscriptions::serve(org.subscriptions.clone()),
            )
            .with(
                wiki_proto::service::registry::registry_rpc_service_descriptor(),
                wiki_proto::service::registry::serve(wiki.clone()),
            )
            // t[impl wiki.source.same-surface] — one mount serves
            // every wiki the org holds, repo-sourced or not: pages,
            // search, graph, subscriptions and the Edit lane all take a
            // slug, and nothing outside the landing path asks whether a
            // repository stands behind it.
            .with(
                wiki_proto::service::edits::edits_rpc_service_descriptor(),
                wiki_proto::service::edits::serve(org.edits.clone()),
            )
            .with(
                wiki_proto::service::ingest::ingest_rpc_service_descriptor(),
                wiki_proto::service::ingest::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::lint::lint_rpc_service_descriptor(),
                wiki_proto::service::lint::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::search::search_rpc_service_descriptor(),
                wiki_proto::service::search::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::watcher::watcher_rpc_service_descriptor(),
                wiki_proto::service::watcher::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::multimodal::multimodal_rpc_service_descriptor(),
                wiki_proto::service::multimodal::serve(wiki.clone()),
            )
            .with(
                wiki_proto::service::review::review_rpc_service_descriptor(),
                wiki_proto::service::review::serve(wiki.clone()),
            )
            // Live wiki changes — the `Events` `#[subscribe]` stream.
            // The hub lives on the `WikiBackend`, so every committed
            // page write / ingest enqueue / review enqueue publishes
            // into it.
            .merge(wiki_proto::service::events::stream_layer(wiki.clone()));
    }

    // Project / Goal / Milestone / Task readers (vault-backed).
    router = router
        .with(
            project::project_service_descriptor(),
            project::serve_project_service(org.projects.clone()),
        )
        // Live project changes — `ProjectService`'s `#[subscribe]`
        // stream sibling. The hub lives on the `ProjectBackend`
        // above; every CRUD path publishes into it.
        .merge(project::project_service_stream_layer(org.projects.clone()))
        .with(
            goal::goal_service_descriptor(),
            goal::serve_goal_service(org.goals.clone()),
        )
        // Live goal changes — `GoalService`'s `#[subscribe]` stream
        // sibling, served from the hub on the `GoalBackend` above.
        .merge(goal::goal_service_stream_layer(org.goals.clone()))
        .with(
            milestone::milestone_service_descriptor(),
            milestone::serve_milestone_service(org.milestones.clone()),
        )
        // Live milestone changes — `MilestoneService`'s
        // `#[subscribe]` stream sibling, served from the hub on the
        // `MilestoneBackend` above.
        .merge(milestone::milestone_service_stream_layer(
            org.milestones.clone(),
        ))
        .with(
            workstream::workstream_service_descriptor(),
            workstream::serve_workstream_service(org.workstreams.clone()),
        )
        .with(
            files::files_service_descriptor(),
            files::serve_files_service(org.files.clone()),
        )
        // Live root-creation / checkpoint events — `FilesService`'s
        // `#[subscribe]` stream sibling, served from the hub on the
        // `FilesBackend` above.
        .merge(files::files_service_stream_layer(org.files.clone()))
        // The v2 lanes (`files_proto::service`), one mount per trait.
        //
        // Every one is served by the SAME `FilesBackend` as v1 above —
        // they are `impl XService for FilesBackend`, not separate
        // objects — so a root adopted through `RootsService` is the root
        // `TreeService` browses, with no state to keep in step.
        //
        // v1 stays mounted beside them until its last caller moves. The
        // two surfaces disagree about nothing because they are the same
        // backend; where a method exists on both (`browse`, `chain`,
        // `hydrate`), the v2 one adds typed ids, typed faults and path
        // confinement and delegates to the same inner method.
        //
        // Each lane's permits live in `permits.rs` — mounting without
        // granting fails every method closed, which is the failure this
        // ordering exists to prevent.
        .with(
            files_proto::roots_descriptor(),
            files_proto::serve_roots(org.files.clone()),
        )
        .with(
            files_proto::tree_descriptor(),
            files_proto::serve_tree(org.files.clone()),
        )
        .with(
            files_proto::write_descriptor(),
            files_proto::serve_write(org.files.clone()),
        )
        .with(
            files_proto::upload_descriptor(),
            files_proto::serve_upload(org.files.clone()),
        )
        .with(
            files_proto::version_descriptor(),
            files_proto::serve_version(org.files.clone()),
        )
        .with(
            files_proto::curation_descriptor(),
            files_proto::serve_curation(org.files.clone()),
        )
        .with(
            files_proto::sync_descriptor(),
            files_proto::serve_sync(org.files.clone()),
        )
        .with(
            files_proto::access_descriptor(),
            files_proto::serve_access(org.files.clone()),
        )
        .with(
            files_proto::organise_descriptor(),
            files_proto::serve_organise(org.files.clone()),
        )
        // Federation. Mounted like any other lane: an offered subtree is
        // reached by a peer dialling this server's iroh endpoint, and
        // `browse_offered` is the only method here whose caller is not a
        // member of this org.
        .with(
            files_proto::federation_descriptor(),
            files_proto::serve_federation(org.files.clone()),
        )
        // The byte lane, both halves. `media` mints tickets and lists
        // renditions; `media_stream` is the subscription those tickets
        // are redeemed on. Neither was mounted, so every v2 path to a
        // file's *content* — reads, previews, editor handoff — was
        // unreachable on the server while passing its own tests.
        .with(
            files_proto::media_descriptor(),
            files_proto::serve_media(org.files.clone()),
        )
        .merge(files_proto::media_stream_layer(org.files.clone()))
        .with(
            files_proto::search_descriptor(),
            files_proto::serve_search(org.files.clone()),
        )
        .with(
            files_proto::review_descriptor(),
            files_proto::serve_review(org.files.clone()),
        )
        // Placement — Storage Locations this org was granted, where its
        // roots live, and blob replicas (issue #262). The operator and
        // agent lanes of the same layer sit on the SERVER router, not
        // here: the registry is deployment-scoped.
        .with(
            files_storage::storage_service_descriptor(),
            files_storage::serve_storage_service(org.storage.clone()),
        )
        .merge(files_storage::storage_service_stream_layer(
            org.storage.clone(),
        ))
        .with(
            task::task_service_descriptor(),
            // The raw backend serves directly. There used to be a
            // forge-sync decorator here mirroring task writes into
            // Forgejo issues; it went with `forge_sync` (2026-08-06).
            task::serve_task_service(org.tasks.clone()),
        )
        // Live task changes — the `#[subscribe]` stream sibling of
        // `TaskService`. The hub lives on the raw `TaskBackend`, so
        // every write path publishes into it: vox calls through the
        // forge-sync decorator above (it delegates to `org.tasks`),
        // CLI/agent mutations over this same router, and the forge
        // poll loop (it writes via `org.tasks.update`).
        .merge(task::task_service_stream_layer(org.tasks.clone()))
        // Live workstream changes — `WorkstreamService`'s
        // `#[subscribe]` stream sibling. The hub lives on the
        // `WorkstreamBackend` above; every CRUD path publishes
        // into it.
        .merge(workstream::workstream_service_stream_layer(
            org.workstreams.clone(),
        ));

    // ── Home-ops plugin — locations + physical inventory.
    #[cfg(feature = "plugin-home")]
    if on("home") {
        router = router
            .with(
                locations::locations_service_descriptor(),
                locations::serve_locations_service(org.locations.clone()),
            )
            .with(
                inventory::inventory_service_descriptor(),
                inventory::serve_inventory_service(org.inventory.clone()),
            );
    }

    // ── Scripture plugin ─────────────────────────────────────────
    #[cfg(feature = "plugin-scripture")]
    if on("scripture") {
        router = router.with(
            scripture::scripture_service_descriptor(),
            scripture::serve_scripture_service(org.scripture.clone()),
        );
    }

    router = router
        .with(
            links::links_service_descriptor(),
            links::serve_links_service(org.links.clone()),
        )
        .with(
            resources_proto::resources_service_rpc_service_descriptor(),
            resources_proto::serve(org.resources.clone()),
        );

    // ── FastTrackStudio plugin — ordered collections (Library /
    // Setlist / Show / Playlist) backing the song/setlist surfaces.
    #[cfg(feature = "plugin-fasttrackstudio")]
    if on("fasttrackstudio") {
        router = router.with(
            collection::collection_service_descriptor(),
            collection::serve_collection_service(org.collections.clone()),
        );
    }

    // ── Mealplan plugin — cookbook / plan / pantry / shopping /
    // substitutions.
    #[cfg(feature = "plugin-mealplan")]
    if on("mealplan") {
        router = router
            .with(
                cookbook::cookbook_service_descriptor(),
                cookbook::serve_cookbook_service(org.cookbook.clone()),
            )
            .with(
                mealplan::mealplan_service_descriptor(),
                mealplan::serve_mealplan_service(org.mealplan.clone()),
            )
            .with(
                pantry::pantry_service_descriptor(),
                pantry::serve_pantry_service(org.pantry.clone()),
            )
            .with(
                mealplan::shopping::shopping_service_rpc_service_descriptor(),
                mealplan::shopping::serve(org.shopping.clone()),
            )
            .with(
                mealplan::substitutions::substitution_service_rpc_service_descriptor(),
                mealplan::substitutions::serve(org.substitutions.clone()),
            );
    }

    // ── Fitness plugin — body / exercises / workouts / intake.
    #[cfg(feature = "plugin-fitness")]
    if on("fitness") {
        router = router
            .with(
                body::body_service_descriptor(),
                body::serve_body_service(org.body.clone()),
            )
            .with(
                exercises::exercises_service_descriptor(),
                exercises::serve_exercises_service(org.exercises.clone()),
            )
            .with(
                workouts::workouts_service_descriptor(),
                workouts::serve_workouts_service(org.workouts.clone()),
            )
            .with(
                intake::intake_service_descriptor(),
                intake::serve_intake_service(org.intake.clone()),
            );
    }

    // ── Email plugin — `EmailSync` (accounts / folders / envelopes /
    // fetch / send / flag / subscribe), served by the per-org
    // Maildir backend.
    #[cfg(feature = "plugin-email")]
    if on("email") {
        router = router
            .with(
                email_proto::descriptor(),
                email_proto::serve(org.email.clone()),
            )
            // Product layer — the staged-send outbox
            // (`EmailProduct`: list / submit / approve / cancel).
            // Its events ride the `EmailSync` changes stream
            // below (shared hub), so there's no second stream.
            .with(
                email_proto::product_descriptor(),
                email_proto::product_serve(org.email_product.clone()),
            )
            // Message↔entity links — "every email on this project".
            .with(
                email_proto::links_descriptor(),
                email_proto::links_serve(org.email_links.clone()),
            )
            // Live mailbox changes — `EmailSync`'s `#[subscribe]`
            // stream sibling, served from the backend's hub.
            .merge(email_proto::stream_layer(org.email.clone()));
    }

    // ── Forge plugin — RepoCatalog + IssueTracker + ReviewSurface, all
    // served by the org's single Forgejo `Backend`. The /repos UI
    // binds RepoCatalog (list repos) + IssueTracker (list issues
    // per repo); ReviewSurface rounds out the surface so PR views
    // can bind without another mount pass.
    #[cfg(feature = "plugin-git")]
    if on("git") {
        router = router
            .with(
                git_proto::repo::repo_catalog_rpc_service_descriptor(),
                git_proto::repo::serve(org.forge.clone()),
            )
            .with(
                git_proto::issues::issue_tracker_rpc_service_descriptor(),
                git_proto::issues::serve(org.forge.clone()),
            )
            .with(
                git_proto::reviews::review_surface_rpc_service_descriptor(),
                git_proto::reviews::serve(org.forge.clone()),
            )
            // Live forge changes — the `#[subscribe]` stream siblings of
            // `IssueTracker` / `ReviewSurface`. The hubs live on the
            // forge backend, so every issue / PR write this server
            // commits publishes into them.
            .merge(git_proto::issues::stream_layer(org.forge.clone()))
            .merge(git_proto::reviews::stream_layer(org.forge.clone()))
            .with(
                git_proto::connections::repo_connections_rpc_service_descriptor(),
                git_proto::connections::serve(connections::ConnectionsBackend::new(
                    org.issue_links_path.clone(),
                )),
            );
    }

    router
        // Per-file collaborative editing — the `DocSync` service over
        // the vault-collab `DocRegistry`: one mounted dispatcher
        // serves every vault-file doc (admission: ids registered via
        // `VaultSync::open_collab`), with the write-behind keeping
        // the plain files on disk authoritative for everyone else.
        .with(
            crdt::sync::doc_sync_service_descriptor(),
            crdt::sync::DocSyncDispatcher::new(org.vault_collab.registry().clone()),
        )
        // Presence — ONE mounted `DocPresence` service, routed by doc
        // id: the fixed `presence::PRESENCE_DOC_ID` reaches the
        // org-wide "who's online" host; any other id reaches the
        // vault-collab registry (per-file cursor channels). States
        // ride Loro's `EphemeralStore` and expire when a peer goes
        // quiet; nothing is persisted.
        .with(
            crdt::sync::doc_presence_service_descriptor(),
            crdt::sync::DocPresenceDispatcher::new(presence::PresenceRouter::new(
                org.presence.clone(),
                org.vault_collab.registry().clone(),
            )),
        )
        // Vault link-graph (backlinks / links / orphans / unresolved /
        // deadends / tags) — the read-only sibling of the vault sync
        // service mounted above, over the same per-org `"default"`
        // vault. Backs the vault page's backlinks panel + the
        // editor's tag-autocomplete candidates.
        .with(
            vault_proto::vault_graph_rpc_service_descriptor(),
            vault_proto::vault_graph_serve(org.vault_graph.clone()),
        )
}

/// The public base only when explicitly configured — `None` on the
/// bind-address fallback. A guest's `server=` hint must never carry
/// the bind address (a remote browser would dial 127.0.0.1); absent,
/// the guest app falls back to same-origin, which is right in prod.
pub(crate) fn share_public_base_explicit() -> Option<String> {
    std::env::var("TASK_SHARE_PUBLIC_BASE")
        .or_else(|_| std::env::var("TASK_SERVER_PUBLIC_URL"))
        .ok()
        .filter(|s| !s.is_empty())
}

/// Public base URL share links are composed against.
pub(crate) fn share_public_base() -> String {
    std::env::var("TASK_SHARE_PUBLIC_BASE")
        .or_else(|_| std::env::var("TASK_SERVER_PUBLIC_URL"))
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            let bind =
                std::env::var("TASK_SERVER_BIND").unwrap_or_else(|_| "127.0.0.1:3456".into());
            format!("http://{bind}")
        })
}

// The share landing / browse / rendition / download handlers live in
// `share` (issue #271) — token-gated, re-checked on every hit.

/// The WebSocket subprotocol every Task client offers, and the one the
/// server selects. A client that offers subprotocols gets no connection at
/// all unless the server echoes one back, so this is what makes the
/// bearer subprotocol below safe to add.
pub const VOX_SUBPROTOCOL: &str = "vox.v1";

/// Prefix of the subprotocol carrying the caller's session token:
/// `vox.bearer.<token>`.
///
/// Session tokens are base64url-no-pad (`generate_token`), whose alphabet
/// is a subset of the RFC 7230 token charset a subprotocol value must use
/// — so the token needs no further encoding.
pub const VOX_BEARER_SUBPROTOCOL_PREFIX: &str = "vox.bearer.";

/// The identity presented at WebSocket **upgrade**, applied to every call
/// on the resulting connection (see
/// [`architect::permissions_gate::PermissionsGate::wrap_shared_with_bearer`]).
///
/// Two channels, because the two client families can do different things:
///
/// - `Authorization: Bearer <token>` — native clients (desktop, CLI, iOS,
///   the watch bridge) control their handshake request directly.
/// - `Sec-WebSocket-Protocol: vox.v1, vox.bearer.<token>` — browsers
///   cannot set arbitrary WebSocket headers, and the token must NOT ride
///   a URL query parameter (it would land in every proxy + access log
///   along the path). The subprotocol list is the one client-controlled
///   field on a browser handshake.
fn upgrade_bearer(headers: &axum::http::HeaderMap) -> Option<String> {
    if let Some(token) = crate::watch_bridge::bearer(headers) {
        return Some(token);
    }
    headers
        .get_all(axum::http::header::SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .filter_map(|p| p.trim().strip_prefix(VOX_BEARER_SUBPROTOCOL_PREFIX))
        .map(str::to_owned)
        .find(|t| !t.is_empty())
}

/// The org router as it must actually be served: permission gate
/// outermost, snapshot write gate under it, `org_layer_router` inside.
///
/// A function rather than three lines inside the WebSocket handler,
/// because the composition is the security boundary and it was reachable
/// from exactly one transport. Anything else serving `org_layer_router`
/// — an iroh endpoint, a `LocalServer`, a test harness — got the bare
/// router: no permission gate, no snapshot parking, and no way to notice
/// from the call site, since the bare router answers every call
/// perfectly well. `files.topology.multi-server` means more than one
/// transport is the expected case, so the guarded form has to be the one
/// that is easy to reach.
///
/// `bearer` is the identity presented at establish, applied to every
/// call on the connection that does not carry its own `authorization`
/// metadata. Transports without a handshake to read it from pass `None`
/// and rely on per-call metadata.
pub fn org_router_guarded(
    org: &OrgAppState,
    gate: snapshot::WriteGate,
    bearer: Option<String>,
) -> architect::permissions_gate::PermissionedRouter<snapshot::GatedRouter> {
    // Every request parks at the snapshot write gate on dispatch
    // entry — see `snapshot::GatedRouter`. Free when no snapshot is
    // running.
    let router = snapshot::GatedRouter::new(org_layer_router(org), gate);
    // Outermost: the permission gate (deny before snapshot-parking or
    // dispatch). One shared gate per org, wrapped per connection — which
    // is why the connection's bearer can be baked in here.
    architect::permissions_gate::PermissionsGate::wrap_shared_with_bearer(
        org.permissions.clone(),
        router,
        bearer,
    )
}

/// Give an org's Files backend a way to reach other servers.
///
/// Federation and peering both need it: an accepted offer resolves
/// through its origin, and a relayed read fetches from the host that
/// holds the bytes. Without it those lanes answer `Unavailable` for
/// every remote root — mounted, permitted, and unable to do the one
/// thing they exist for.
///
/// [`iroh_host::start`] calls this once per hosted org as the process
/// comes up, with [`files::IrohRemotes`] over that org's own endpoint.
/// It did not, for a while, and the peering lanes were mounted and
/// permitted and answered `Unavailable` for every remote root — which is
/// worth remembering, because nothing about the router said so.
///
/// Takes `&mut` because the backend holds the port by value —
/// `FilesBackend::with_remotes` is a consuming builder, so installing it
/// on a *clone* leaves the org's own backend, and therefore the router,
/// exactly as it was. That is not a hypothetical: it is how the harness
/// first wired this, and every federated call over the wire answered
/// `Unavailable` while the in-process ones passed.
pub fn attach_peering(
    org: &mut OrgAppState,
    endpoint_id: impl Into<String>,
    remotes: std::sync::Arc<dyn files::lane::federation::RemoteFiles>,
) {
    org.files = org.files.clone().with_remotes(endpoint_id, remotes);
}

/// Serve an org's router on an iroh endpoint, one connection at a time.
///
/// The counterpart of [`serve_org_vox`], and it exists for the same
/// reason that one takes `headers`: the router has to be wrapped **per
/// connection**, because the identity is a property of the connection
/// rather than of the server.
///
/// On a WebSocket that identity is read from the upgrade. Here it is
/// `connection.remote_id()` — and the difference is that this one was
/// not presented, it was *proved*. An iroh connection is mutually
/// authenticated by construction, so by the time this runs the peer has
/// already demonstrated it holds the secret half of that endpoint id.
/// There is no token to check because there was never a claim to doubt.
///
/// `HostResolver` then decides whether this org admits that id. A
/// verified stranger is still a stranger.
///
/// The accept loop itself is [`files::peer::serve_over_iroh`], shared
/// with the device that serves its own replica lane
/// (`files_sync::serve_peer`). Only what gets wrapped differs: an org
/// router here, one lane there.
pub async fn serve_org_iroh(
    org: OrgAppState,
    gate: snapshot::WriteGate,
    endpoint: &architect::iroh_link::iroh::Endpoint,
) {
    files::peer::serve_over_iroh(endpoint, move |bearer| {
        org_router_guarded(&org, gate.clone(), bearer)
    })
    .await;
}

fn serve_org_vox(
    org: OrgAppState,
    gate: snapshot::WriteGate,
    ws: WebSocketUpgrade,
    headers: &axum::http::HeaderMap,
) -> axum::response::Response {
    let router = org_router_guarded(&org, gate, upgrade_bearer(headers));
    ws.protocols([VOX_SUBPROTOCOL])
        .on_upgrade(move |socket| architect::axum_ws::serve_router(socket, router))
        .into_response()
}

#[cfg(test)]
mod upgrade_bearer_tests {
    use axum::http::{HeaderMap, HeaderValue};

    use super::upgrade_bearer;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn no_identity_presented() {
        assert_eq!(upgrade_bearer(&HeaderMap::new()), None);
        // Offering the plain subprotocol is not an identity.
        assert_eq!(
            upgrade_bearer(&headers(&[("sec-websocket-protocol", "vox.v1")])),
            None
        );
    }

    #[test]
    fn native_authorization_header() {
        assert_eq!(
            upgrade_bearer(&headers(&[("authorization", "Bearer abc123")])),
            Some("abc123".to_owned())
        );
    }

    #[test]
    fn browser_subprotocol() {
        // How a browser actually sends it: one header, comma-separated,
        // with the spaces the client library inserts.
        assert_eq!(
            upgrade_bearer(&headers(&[(
                "sec-websocket-protocol",
                "vox.v1, vox.bearer.tok-en_9"
            )])),
            Some("tok-en_9".to_owned())
        );
    }

    #[test]
    fn subprotocol_split_across_headers() {
        // Equally legal per RFC 9110 §5.3, and what some proxies produce.
        assert_eq!(
            upgrade_bearer(&headers(&[
                ("sec-websocket-protocol", "vox.v1"),
                ("sec-websocket-protocol", "vox.bearer.xyz"),
            ])),
            Some("xyz".to_owned())
        );
    }

    #[test]
    fn header_wins_over_subprotocol() {
        assert_eq!(
            upgrade_bearer(&headers(&[
                ("authorization", "Bearer from-header"),
                ("sec-websocket-protocol", "vox.v1, vox.bearer.from-proto"),
            ])),
            Some("from-header".to_owned())
        );
    }

    #[test]
    fn empty_token_is_not_an_identity() {
        // An empty bearer must read as "anonymous", not as a token that
        // will resolve to nothing and muddy `auth.outcome`.
        assert_eq!(
            upgrade_bearer(&headers(&[(
                "sec-websocket-protocol",
                "vox.v1, vox.bearer."
            )])),
            None
        );
        assert_eq!(
            upgrade_bearer(&headers(&[("authorization", "Bearer ")])),
            None
        );
    }
}

#[cfg(test)]
mod range_tests {
    use super::parse_byte_range;

    #[test]
    fn full_open_range() {
        // Chrome's initial media probe: `bytes=0-`.
        assert_eq!(parse_byte_range("bytes=0-", 1000), Some((0, 999)));
    }

    #[test]
    fn clamped_end() {
        assert_eq!(parse_byte_range("bytes=0-100000", 1000), Some((0, 999)));
    }

    #[test]
    fn mid_range() {
        assert_eq!(
            parse_byte_range("bytes=500-799", 3_415_886),
            Some((500, 799))
        );
    }

    #[test]
    fn suffix_range() {
        assert_eq!(parse_byte_range("bytes=-500", 1000), Some((500, 999)));
    }

    #[test]
    fn rejects_bad() {
        assert_eq!(parse_byte_range("bytes=-0", 1000), None);
        assert_eq!(parse_byte_range("bytes=800-500", 1000), None); // start > end
        assert_eq!(parse_byte_range("bytes=0-", 0), None); // empty file
        assert_eq!(parse_byte_range("bytes=0-10,20-30", 1000), None); // multi-range
        assert_eq!(parse_byte_range("garbage", 1000), None);
        assert_eq!(parse_byte_range("bytes=5000-6000", 1000), None); // start past end
    }
}

/// The domain this deployment publishes its own Resources under.
///
/// A reference carries the publishing org's federation domain
/// (ADR 0002), and the first-party Resources — scripture today — are
/// published by us. The constant exists so the domain in a reference
/// and the domain in a subscription come from one place: a mismatch
/// between them is a reference that resolves for nobody.
pub const FIRST_PARTY_DOMAIN: &str = "fasttrackstudio.app";

/// Domain → org slug, for resolving a reference's publishing domain to
/// an org on this data root (`wiki.ref.format`: the domain is a name,
/// not an address).
///
/// Three sources, later ones winning: every org answers to its own
/// slug (`fasttrackstudios/music-theory::Ionian` resolves on the box
/// that hosts it, with nothing configured); the example's orgs answer
/// to `<name>.test`, which is what its seeded references carry; and
/// `TASK_WIKI_DOMAINS` — `domain=slug,domain=slug` — names the real
/// federation domains a deployment publishes under
/// (`fasttrackstudio.app=fasttrackstudios`).
#[cfg(feature = "plugin-wiki")]
#[must_use]
pub fn wiki_domains(
    orgs_dir: &std::path::Path,
    configured: Option<String>,
) -> std::collections::HashMap<String, String> {
    let mut domains = std::collections::HashMap::new();
    if let Ok(entries) = std::fs::read_dir(orgs_dir) {
        for entry in entries.flatten() {
            if !entry.path().is_dir() {
                continue;
            }
            if let Some(slug) = entry.file_name().to_str() {
                domains.insert(slug.to_owned(), slug.to_owned());
            }
        }
    }
    for (slug, _) in example_org::ORGS {
        domains.insert(
            format!("{}.test", slug.split('-').next().unwrap_or(slug)),
            (*slug).to_owned(),
        );
    }
    for pair in configured.as_deref().unwrap_or_default().split(',') {
        if let Some((domain, slug)) = pair.split_once('=') {
            let (domain, slug) = (domain.trim(), slug.trim());
            if !domain.is_empty() && !slug.is_empty() {
                domains.insert(domain.to_owned(), slug.to_owned());
            }
        }
    }
    domains
}

#[cfg(all(test, feature = "plugin-wiki"))]
mod wiki_domain_tests {
    #[test]
    fn slugs_examples_and_configured_domains_all_resolve() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("fasttrackstudios")).unwrap();
        std::fs::create_dir_all(dir.path().join("codywright")).unwrap();
        let d = super::wiki_domains(
            dir.path(),
            Some(" fasttrackstudio.app=fasttrackstudios, codywright.fasttrackstudio.app = codywright ,bad".into()),
        );
        assert_eq!(d["fasttrackstudios"], "fasttrackstudios");
        assert_eq!(d["fasttrackstudio.app"], "fasttrackstudios");
        assert_eq!(d["codywright.fasttrackstudio.app"], "codywright");
        assert_eq!(d["acme.test"], "acme-audio");
        assert!(!d.contains_key("bad"));
    }
}

/// The deployment's core set — subscribed by default in every vault
/// and every wiki (`wiki.core.default`).
///
/// Scripture is the founding member, and the reason the rule exists:
/// a note written in a brand-new vault should be able to reference a
/// verse in its first line, without anyone having gone looking for a
/// setting.
///
/// Core membership is a property of the *source*, so this is a list of
/// what everyone gets rather than a copy handed to each org. The
/// corpus behind it is still installed per org today; sharing one copy
/// is what the rest of the subscription work is for.
#[cfg(feature = "plugin-wiki")]
#[must_use]
pub fn core_subscriptions() -> Vec<wiki_proto::Subscription> {
    vec![wiki_live::subscriptions::core_resource(
        FIRST_PARTY_DOMAIN,
        "bible",
        "Bible",
    )]
}
