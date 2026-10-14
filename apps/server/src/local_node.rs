//! A device's own node: this backend, hosted inside the app.
//!
//! `offline.local` — the app is fully functional with no server — and
//! `offline.peers` — every native install is a node, and a host is just
//! the one that is usually on — are the same fact seen from two sides.
//! The app does not keep a private cache of somebody else's server; it
//! runs the server, over a data root in its own data dir, and its pages
//! talk to that node through the same typed clients they would send
//! over a socket. Nothing above the transport can tell the difference,
//! and that is the whole design: a page that works against a host works
//! against the device.
//!
//! # What a node is here
//!
//! Exactly what `task-server`'s `main` boots, minus the HTTP listener:
//!
//! - an [`AppState`] over the device's data root ([`AppState::open_at`],
//!   never `$TASK_DATA_ROOT` — a GUI process must not steer itself by a
//!   process-wide env var, and the sync agent it spawns would inherit it);
//! - the org's **iroh presence** ([`crate::iroh_host::start`]) — one
//!   endpoint per org, its key on disk at `orgs/<slug>/iroh-key.ed25519`
//!   — so other devices, and hosts, can dial this one by id and it can
//!   dial them;
//! - the device pull ([`crate::device_sync::start`]), so admitted peers
//!   are swept the way a host sweeps its devices. There is no hub.
//!
//! Each org lane is served in-process ([`LocalNode::caller`]) through
//! [`crate::org_router_guarded`] with the caller's bearer baked in — the
//! same wrapping a WebSocket upgrade or an iroh connection gets. So the
//! permission gate, the auth middleware and every service see a signed-in
//! person exactly as they would over the wire: a device's node is not a
//! back door to its own data root.
//!
//! # Who you are on a fresh device
//!
//! A node with no org provisions one on first boot: a **personal org**,
//! home (the identity authority for its own accounts), owned by a local
//! account with a generated password kept beside the data root in
//! `local-owner.json` (0600). The app signs in with it through the
//! org's ordinary `AuthService` — no central issuer, no network — and
//! the owner gets an `admin` role and an owner membership row, which is
//! what the Files lanes read a role from. Debug builds can plant the
//! example studio instead ([`NodeOptions::plant_example`]), the same
//! planting `admin demo` runs, and then the owner is the example's owner.
//!
//! # Joining an org that lives elsewhere
//!
//! [`LocalNode::join`]: the device takes a node for an org a host (or
//! another device) already holds — same slug, same org id — admits that
//! peer's endpoint id, binds the org's endpoint here, and grants the
//! device's own person membership in it. The caller hands the returned
//! endpoint id to the peer's `enroll_device`, and from then the two
//! nodes admit each other. Moving the org's *documents* between them is
//! the document sync's job (`offline.doc.replica`), not this module's.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use eyre::{WrapErr, bail};
use tracing::{info, warn};
use uuid::Uuid;

use crate::AppState;
use crate::iroh_host::IrohHost;

/// The personal org's slug on a fresh device.
pub const PERSONAL_SLUG: &str = "personal";

/// The file the local owner's credentials live in, beside `orgs/`.
pub const OWNER_FILE: &str = "local-owner.json";

/// The account a device's node signs its own person in as.
///
/// Credentials, not a session: the app signs in with these through the
/// org's own `AuthService`, so the session it holds is an ordinary one —
/// revocable, listed, checked by the gate.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LocalOwner {
    /// The org whose auth store holds the account (sign in there).
    pub org: String,
    pub email: String,
    pub name: String,
    pub password: String,
}

/// How to open a node.
#[derive(Clone, Debug)]
pub struct NodeOptions {
    /// The device's data root — `<app data dir>/node` in the apps.
    pub data_root: PathBuf,
    /// The person's display name for a freshly provisioned personal org.
    pub owner_name: String,
    /// Plant the example studio (acme-audio + alice-personal) when the
    /// root holds no org yet. Debug builds only: it plants
    /// known-password accounts. Ignored in release.
    pub plant_example: bool,
    /// Bind each org's iroh endpoint and run the device pull. Off in
    /// tests that only exercise the in-process lanes.
    pub peers: bool,
}

impl NodeOptions {
    /// Defaults for `data_root`: personal org, peers on, no example.
    #[must_use]
    pub fn at(data_root: impl Into<PathBuf>) -> Self {
        Self {
            data_root: data_root.into(),
            owner_name: "Me".to_owned(),
            plant_example: false,
            peers: true,
        }
    }
}

/// An org this node took from a peer ([`LocalNode::join`]).
#[derive(Clone, Debug)]
pub struct JoinSpec {
    pub slug: String,
    /// The org's stable id on the peer — the replica must carry the same
    /// one, or it is a different org that happens to share a slug.
    pub id: Uuid,
    pub display_name: String,
    /// The peer's endpoint id for this org (its `coordinator()`), admitted
    /// here so it may dial this node. `None` admits nobody yet.
    pub peer: Option<String>,
    /// The membership role the device's person holds in it (`None` = a
    /// plain member). The peer stays the authority: this only decides
    /// what the device's own node lets its own person do offline.
    pub role: Option<String>,
}

/// What [`LocalNode::join`] made.
#[derive(Clone, Debug)]
pub struct Joined {
    pub slug: String,
    /// This node's endpoint id for the org — what the peer must enroll.
    /// `None` when peers are off or the bind failed.
    pub endpoint_id: Option<String>,
}

/// A running node. Cheap to share behind an `Arc`.
pub struct LocalNode {
    state: AppState,
    owner: LocalOwner,
    peers: bool,
    /// Keeps every bound endpoint accepting — dropping the last handle
    /// stops it. Grows as orgs are joined.
    iroh: std::sync::Mutex<Vec<(String, architect::iroh_link::iroh::Endpoint)>>,
}

impl std::fmt::Debug for LocalNode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalNode")
            .field("data_root", &self.state.data_root.path())
            .field("owner", &self.owner.email)
            .finish_non_exhaustive()
    }
}

impl LocalNode {
    /// Provision (first run) and boot a node over `opts.data_root`.
    ///
    /// # Errors
    ///
    /// The data root cannot be created, provisioning the first org
    /// fails, or the backend cannot boot over it.
    // t[impl offline.local] — the backend the app hosts: an AppState over
    // the device's own data root, provisioned with a personal org and its
    // owner on first run, with the org's iroh presence bound
    pub async fn open(opts: NodeOptions) -> eyre::Result<Self> {
        let data_root = org_proto::DataRoot::new(opts.data_root.clone());
        data_root
            .ensure()
            .map_err(|e| eyre::eyre!("ensure node data root: {e}"))?;
        let owner = provision(&data_root, &opts).await?;

        let state = AppState::open_at(data_root, None).await?;
        let node = Self {
            state,
            owner,
            peers: opts.peers,
            iroh: std::sync::Mutex::new(Vec::new()),
        };
        if opts.peers {
            // Before any lane is served: `iroh_host::start` installs the
            // dialler on each org's backend in the org map, and a router
            // built before that would dispatch into the backends as they
            // were (see `iroh_host::start`).
            if let Some(host) = crate::iroh_host::start(&node.state).await {
                for (slug, id) in host.ids() {
                    info!(%slug, endpoint_id = %id, "node: reachable by endpoint id");
                }
                crate::device_sync::start(&node.state, &host);
                node.hold(host);
            }
        }
        Ok(node)
    }

    fn hold(&self, host: IrohHost) {
        if let Ok(mut held) = self.iroh.lock() {
            held.extend(host.endpoints);
        }
    }

    /// The backend itself.
    #[must_use]
    pub fn state(&self) -> &AppState {
        &self.state
    }

    /// Who this device's person signs in as.
    #[must_use]
    pub fn owner(&self) -> &LocalOwner {
        &self.owner
    }

    /// Hosted org slugs, sorted.
    #[must_use]
    pub fn org_slugs(&self) -> Vec<String> {
        self.state.org_slugs()
    }

    /// Whether `slug` is held by this node.
    #[must_use]
    pub fn hosts(&self, slug: &str) -> bool {
        self.state.org(slug).is_some()
    }

    /// This node's endpoint id for `slug`, if it is bound.
    #[must_use]
    pub fn endpoint_id(&self, slug: &str) -> Option<String> {
        self.iroh.lock().ok().and_then(|held| {
            held.iter()
                .find(|(s, _)| s == slug)
                .map(|(_, e)| e.id().to_string())
        })
    }

    /// This node's endpoint for `slug`, if it is bound — what a sweep
    /// dials from (`device_sync::sweep`).
    #[must_use]
    pub fn endpoint(&self, slug: &str) -> Option<architect::iroh_link::iroh::Endpoint> {
        self.iroh
            .lock()
            .ok()
            .and_then(|held| held.iter().find(|(s, _)| s == slug).map(|(_, e)| e.clone()))
    }

    /// The org-discovery document this node would serve at
    /// `/.well-known/task-server.json`, for `bearer`.
    pub async fn discovery(&self, bearer: Option<String>) -> serde_json::Value {
        crate::discovery_document(&self.state, bearer).await
    }

    /// One in-process connection to `slug`'s full org router, as the
    /// holder of `bearer` — the raw caller every typed client is a view
    /// over. The bearer is baked into the permission gate exactly as a
    /// socket upgrade bakes it, so the lane is no more trusted than a
    /// remote one.
    ///
    /// # Errors
    ///
    /// `slug` is not hosted here, or the in-memory handshake fails.
    pub async fn caller(
        &self,
        slug: &str,
        bearer: Option<String>,
    ) -> eyre::Result<vox_core::Caller> {
        let org = self
            .state
            .org(slug)
            .ok_or_else(|| eyre::eyre!("org `{slug}` is not held by this node"))?;
        let router = crate::org_router_guarded(&org, self.state.write_gate.clone(), bearer);
        serve_in_process(router, &self.state.scope).await
    }

    /// The node's `/server/vox` surface in-process — org management,
    /// snapshots, the identity locker, device enrolment. NOT the
    /// local-trusted variant: the app is a client of its node, and the
    /// services check the token they are handed exactly as on a host.
    ///
    /// # Errors
    ///
    /// The in-memory handshake fails.
    pub async fn server_caller(&self) -> eyre::Result<vox_core::Caller> {
        let router = crate::snapshot::GatedRouter::new(
            crate::server_layer_router(&self.state, false),
            self.state.write_gate.clone(),
        );
        serve_in_process(router, &self.state.scope).await
    }

    /// Take a node for an org a peer holds: same slug and id, the peer
    /// admitted, this org's endpoint bound, and the device's person a
    /// member. Idempotent — joining an org already held refreshes the
    /// admission and membership and returns the existing endpoint.
    ///
    /// # Errors
    ///
    /// An invalid slug, a slug already held here under a DIFFERENT org
    /// id, or any failure writing the replica or booting its lanes.
    // t[impl offline.peers] — a device joins an org as a node: the org's
    // identity (slug + id), the peer it came from admitted, its own
    // endpoint bound for that peer to enroll
    pub async fn join(&self, spec: JoinSpec) -> eyre::Result<Joined> {
        let data_root = &self.state.data_root;
        let org_root = data_root.org(spec.slug.as_str());
        if org_root.path().exists() {
            let manifest = org_root
                .manifest()
                .map_err(|e| eyre::eyre!("read `{}` manifest: {e}", spec.slug))?;
            if manifest.id != spec.id {
                bail!(
                    "this device already holds a different org called `{}` ({}), not {}",
                    spec.slug,
                    manifest.id,
                    spec.id
                );
            }
        } else {
            let made = data_root
                .init_org(&spec.slug, &spec.display_name, false)
                .map_err(|e| eyre::eyre!("create `{}` here: {e}", spec.slug))?;
            let mut manifest = made
                .manifest()
                .map_err(|e| eyre::eyre!("read new manifest: {e}"))?;
            manifest.id = spec.id;
            manifest
                .write_to_dir(made.path())
                .map_err(|e| eyre::eyre!("write manifest: {e}"))?;
        }
        if let Some(peer) = spec.peer.as_deref() {
            admit(org_root.path(), peer)?;
        }
        grant_membership(data_root, &self.owner, &spec.slug, spec.role.as_deref()).await?;

        if !self.hosts(&spec.slug) {
            let auth_db_url = format!("sqlite://{}?mode=rwc", org_root.auth_db().display());
            let auth = crate::AuthState::open(&auth_db_url, &crate::auth_secret()).await?;
            let built = crate::build_org_state(
                auth,
                &self.state.keypair,
                org_root,
                &self.state.scope,
                &self.state.storage,
                self.state.home_identity.as_ref(),
                &crate::blob_root_of(data_root)?,
            )
            .await?;
            self.state
                .insert_org(spec.slug.clone(), built)
                .map_err(|e| eyre::eyre!("{e}"))?;
        }
        Ok(Joined {
            endpoint_id: self.bind(&spec.slug).await,
            slug: spec.slug,
        })
    }

    /// Make `slug` dialable on this node, if it is not already — for an
    /// org that arrived after boot (joined, or created over the node's
    /// `/server` lane). Returns its endpoint id; `None` with peers off or
    /// when the bind fails (logged).
    pub async fn bind(&self, slug: &str) -> Option<String> {
        if !self.peers || !self.hosts(slug) {
            return None;
        }
        if let Some(id) = self.endpoint_id(slug) {
            return Some(id);
        }
        match crate::iroh_host::bind_one(&self.state, slug).await {
            Ok(endpoint) => {
                let id = endpoint.id().to_string();
                // And it sweeps its peers like the orgs bound at boot.
                crate::device_sync::start_one(&self.state, slug, &endpoint);
                if let Ok(mut held) = self.iroh.lock() {
                    held.push((slug.to_owned(), endpoint));
                }
                Some(id)
            }
            Err(e) => {
                warn!(%slug, error = %e, "node: org not bound");
                None
            }
        }
    }
}

/// Serve `router` over an in-memory link and return the client's raw
/// caller. The acceptor task and the client connection both live until
/// `scope` closes — server→client streams (`Tx` subscriptions) need the
/// connection held, which is why it is parked on the scope rather than
/// dropped here.
async fn serve_in_process<H>(
    router: H,
    scope: &Arc<architect::Scope>,
) -> eyre::Result<vox_core::Caller>
where
    H: vox::Handler<vox::DriverReplySink> + Clone + Send + Sync + 'static,
{
    let (client_link, server_link) = vox_core::memory_link_pair(256);
    let server = async move {
        match vox_core::acceptor_on(server_link)
            .on_lane(architect::layer::handler_acceptor(router))
            .establish_connection()
            .await
        {
            Ok(connection) => {
                let _connection = connection;
                std::future::pending::<()>().await;
            }
            Err(e) => warn!(error = %e, "node: in-process acceptor failed"),
        }
    };
    let task = tokio::spawn(server);
    let connection = vox_core::initiator_on(client_link)
        .establish_connection()
        .await
        .map_err(|e| eyre::eyre!("in-process establish_connection: {e:?}"))?;
    let lane = connection
        .open_lane::<architect::local::RawLaneCaller>()
        .await
        .map_err(|e| eyre::eyre!("in-process open_lane: {e:?}"))?;
    scope.defer(move || async move {
        drop(connection);
        task.abort();
    });
    Ok(lane.caller)
}

/// First-run provisioning, then the owner's credentials.
async fn provision(
    data_root: &org_proto::DataRoot,
    opts: &NodeOptions,
) -> eyre::Result<LocalOwner> {
    let owner_path = data_root.path().join(OWNER_FILE);
    if let Ok(raw) = std::fs::read_to_string(&owner_path)
        && let Ok(owner) = serde_json::from_str::<LocalOwner>(&raw)
    {
        return Ok(owner);
    }
    let empty = data_root
        .scan_orgs()
        .map_err(|e| eyre::eyre!("scan orgs: {e}"))?
        .is_empty();
    if !empty {
        bail!(
            "{} holds orgs but no {OWNER_FILE} — not a device node's data root \
             (refusing to guess whose it is)",
            data_root.path().display()
        );
    }

    let owner = if opts.plant_example && cfg!(debug_assertions) {
        plant_example(data_root).await?
    } else {
        provision_personal(data_root, &opts.owner_name).await?
    };
    write_private(
        &owner_path,
        &serde_json::to_vec_pretty(&owner).wrap_err("encode owner")?,
    )?;
    Ok(owner)
}

/// A personal org, home, owned by a fresh local account.
async fn provision_personal(
    data_root: &org_proto::DataRoot,
    name: &str,
) -> eyre::Result<LocalOwner> {
    let display = if name.trim().is_empty() {
        "Me"
    } else {
        name.trim()
    };
    let org = data_root
        .init_org(PERSONAL_SLUG, display, true)
        .map_err(|e| eyre::eyre!("create the personal org: {e}"))?;
    let owner = LocalOwner {
        org: PERSONAL_SLUG.to_owned(),
        // `.local` is reserved for exactly this — a name that means "on
        // this machine" and can never be somebody's real mailbox.
        email: "me@this-device.local".to_owned(),
        name: display.to_owned(),
        password: generated_password(),
    };
    let url = format!("sqlite://{}?mode=rwc", org.auth_db().display());
    let auth = crate::AuthState::open(&url, &crate::auth_secret())
        .await
        .wrap_err("open the personal org's auth store")?;
    crate::admin_cli::seed_account(
        &auth,
        &owner.email,
        &owner.password,
        &owner.name,
        None,
        Some("admin"),
        "owner",
    )
    .await?;
    grant_membership(data_root, &owner, PERSONAL_SLUG, Some("admin")).await?;
    info!(slug = PERSONAL_SLUG, "node: provisioned the personal org");
    Ok(owner)
}

/// The example studio, planted the way `admin demo` plants it — Alice's
/// company and her own org — with the memberships the demo script
/// records by hand (`adopt-principal`).
#[cfg(debug_assertions)]
async fn plant_example(data_root: &org_proto::DataRoot) -> eyre::Result<LocalOwner> {
    const ORGS: [&str; 2] = ["acme-audio", "alice-personal"];
    for slug in ORGS {
        crate::demo_cli::plant(data_root, slug).await?;
    }
    let mut owner = None;
    for slug in ORGS {
        for member in crate::example_org::cast_of(slug) {
            if member.org != slug {
                continue;
            }
            let who = LocalOwner {
                org: member.org.to_owned(),
                email: member.email.to_owned(),
                name: member.name.to_owned(),
                password: crate::example_org::PASSWORD.to_owned(),
            };
            // Casey is a client — one granted folder, no membership — the
            // same omission the demo script makes on purpose.
            if !matches!(member.holds, crate::example_org::Holds::Client) {
                adopt_everywhere(data_root, &who).await?;
            }
            if owner.is_none() && matches!(member.holds, crate::example_org::Holds::Owner) {
                owner = Some(who);
            }
        }
    }
    owner.ok_or_else(|| eyre::eyre!("the example has no owner"))
}

#[cfg(not(debug_assertions))]
async fn plant_example(data_root: &org_proto::DataRoot) -> eyre::Result<LocalOwner> {
    provision_personal(data_root, "Me").await
}

/// `admin adopt-principal` for one person: a membership row in every org
/// on the root where their address has an account, with that account's
/// role, keyed to their home-org id.
#[cfg(debug_assertions)]
async fn adopt_everywhere(data_root: &org_proto::DataRoot, who: &LocalOwner) -> eyre::Result<()> {
    let orgs = data_root
        .scan_orgs()
        .map_err(|e| eyre::eyre!("scan orgs: {e}"))?;
    let Some(home) = orgs.iter().find(|(_, m)| m.is_home).map(|(r, _)| r.clone()) else {
        return Ok(());
    };
    let Some(principal) = user_id_in(&home, &who.email).await?.map(|(id, _)| id) else {
        // Not an account of the identity authority: nothing to key rows to.
        return Ok(());
    };
    let store = crate::memberships::Memberships::open(&home.memberships_db()).await?;
    for (root, _) in &orgs {
        if let Some((_, role)) = user_id_in(root, &who.email).await? {
            store
                .upsert(principal, root.slug(), role.as_deref())
                .await?;
        }
    }
    Ok(())
}

/// The account `email` holds in `org`'s own store, with its role.
async fn user_id_in(
    org: &org_proto::OrgRoot,
    email: &str,
) -> eyre::Result<Option<(Uuid, Option<String>)>> {
    if !org.auth_db().exists() {
        return Ok(None);
    }
    let url = format!("sqlite://{}?mode=rwc", org.auth_db().display());
    let auth = crate::AuthState::open(&url, &crate::auth_secret()).await?;
    let user = auth
        .auth
        .find_user_by_email(email)
        .await
        .map_err(|e| eyre::eyre!("look up `{email}` in `{}`: {e:?}", org.slug()))?;
    Ok(user.map(|u| (u.id, u.role)))
}

/// A membership row for the device's person in `slug`, in the home org's
/// table — which is how a token the home org minted reaches every other
/// org on the node (`HomeIdentity`), joined ones included.
async fn grant_membership(
    data_root: &org_proto::DataRoot,
    owner: &LocalOwner,
    slug: &str,
    role: Option<&str>,
) -> eyre::Result<()> {
    let orgs = data_root
        .scan_orgs()
        .map_err(|e| eyre::eyre!("scan orgs: {e}"))?;
    let Some(home) = orgs.iter().find(|(_, m)| m.is_home).map(|(r, _)| r.clone()) else {
        bail!("no home org on this node to keep memberships in");
    };
    let owner_home = data_root.org(owner.org.as_str());
    let Some((principal, _)) = user_id_in(&owner_home, &owner.email).await? else {
        bail!(
            "the device's person ({}) has no account in `{}`",
            owner.email,
            owner.org
        );
    };
    let store = crate::memberships::Memberships::open(&home.memberships_db()).await?;
    store.upsert(principal, slug, role).await?;
    Ok(())
}

/// Add `peer` to the org's `admitted-devices` (the operator file
/// `device_sync` reconciles), once.
fn admit(org_dir: &Path, peer: &str) -> eyre::Result<()> {
    let peer = peer.trim();
    if peer.is_empty() {
        return Ok(());
    }
    let path = crate::device_sync::admitted_path(org_dir);
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == peer) {
        return Ok(());
    }
    let mut next = existing;
    if !next.is_empty() && !next.ends_with('\n') {
        next.push('\n');
    }
    next.push_str("# admitted when this device joined the org\n");
    next.push_str(peer);
    next.push('\n');
    std::fs::write(&path, next).wrap_err_with(|| format!("write {}", path.display()))
}

fn generated_password() -> String {
    use rand::Rng as _;
    let bytes: [u8; 24] = rand::thread_rng().r#gen();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn write_private(path: &Path, bytes: &[u8]) -> eyre::Result<()> {
    std::fs::write(path, bytes).wrap_err_with(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .wrap_err_with(|| format!("chmod {}", path.display()))?;
    }
    Ok(())
}
