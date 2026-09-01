//! One concept: a server.
//!
//! Not a backend with some lanes bolted on — the real thing. An
//! [`AppState`] over its own data root, with the same
//! [`org_layer_router`] the deployed process mounts, served on an iroh
//! endpoint.
//!
//! # Why the real router, and not a hand-built one
//!
//! This harness used to assemble its own `LayerRouter` from the `files`
//! layers it happened to need. Every test then passed while telling us
//! nothing about whether a client could reach any of it, because the
//! router under test was one that exists nowhere else — and the two
//! failures that actually happened in this repo were both of that
//! shape: a lane implemented, tested, and never mounted; two services
//! whose names collided, so mounting one silently unmounted the other.
//!
//! A router built by the tests cannot catch either. This one is
//! `org_layer_router`, so a lane that is not mounted is not reachable
//! here either.
//!
//! # Why permissions are enforced
//!
//! `TASK_ENFORCE_PERMISSIONS=1`, deliberately. Off, the gate evaluates
//! and records what it *would* have refused; on, it refuses. A suite
//! running with it off would pass identically whether or not the permit
//! tables covered the methods it calls — and covering them is the
//! difference between a lane a signed-in user can call and one that
//! fails closed in production.
//!
//! # Registration is an endpoint id
//!
//! The endpoint's id is the whole address. A device is registered by
//! pasting one in: no host, no port, no certificate. iroh finds a path,
//! traverses the NAT, and falls back to a relay only if it must; the
//! caller never learns which happened.
//!
//! The secret key is fresh per run. A real server persists it at
//! `orgs/<slug>/iroh-key.ed25519` (`task_server::iroh_host`) so its id
//! survives a restart — which is the entire point of registering a
//! device against an id rather than an address. [`Server::restart`]
//! keeps the key for the same reason, in memory rather than on disk,
//! because a test process is the only thing it has to outlive.

use std::path::Path;
use std::sync::Arc;

use files::{FilesBackend, IrohRemotes};
use task_server::{AppState, AuthState, capability::ServerKeypair};

/// Serialises the env-var window around [`AppState`] construction.
///
/// `AppState` reads `TASK_DATA_ROOT` from the process environment at
/// construction, so two servers in one process have to be built one at
/// a time with the variable pointing at each one's own directory in
/// turn. The lock is held across the whole boot rather than just the
/// `set_var`, because the read happens inside `new_with_auth`.
///
/// The alternative is a constructor taking a data root, which belongs
/// in `apps/server` rather than here.
static ENV: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

/// Open this data root's auth store, creating it on first call.
///
/// A file rather than `sqlite::memory:`, so a restart comes back to the
/// same accounts. A test that has to re-hire everybody after a restart
/// cannot ask whether a token still works, and "does a session survive
/// the server" is a question about the product.
async fn open_auth(data: &Path) -> AuthState {
    let url = format!("sqlite://{}?mode=rwc", data.join("auth.sqlite").display());
    AuthState::open(&url, SECRET).await.expect("auth db")
}

/// The secret every test server signs sessions with. Fixed, because
/// nothing here protects anything — and long enough that
/// `ArchitectAuth` accepts it.
const SECRET: &str = "integration-suite-secret-at-least-32-bytes";

pub struct Server {
    pub name: &'static str,
    /// Kept so a restart can come back on the same endpoint id. A real
    /// server persists its key for exactly this reason — an id a device
    /// was registered against must survive the process that minted it.
    key: iroh::SecretKey,
    /// The org this server hosts, as a slug. Also the name of its
    /// directory under the data root.
    pub slug: String,
    pub endpoint: iroh::Endpoint,
    /// The org's Files backend — the same value the router dispatches
    /// into. Fixtures and setup use it directly; the tests go over the
    /// wire, which is the point of there being a wire.
    pub backend: FilesBackend,
    /// Where accounts live. `People` signs users up against this.
    pub auth: AuthState,
    pub state: AppState,
    _data: tempfile::TempDir,
}

impl Server {
    /// Boot a server hosting one org, and serve it on an endpoint.
    ///
    /// `fixture` writes the tree this org starts with, under the org's
    /// own files directory: the tree is *already there* before anyone
    /// adopts it, which is the premise the adoption chapter rests on.
    ///
    /// `examples/studio` is planted first, so this org holds the same
    /// projects `task-server admin demo` gives you — `Example Album`,
    /// `Shared Project`, the folders named to break a reader. The
    /// closure then adds what a chapter needs on top.
    ///
    /// Both, rather than one: the example is a studio's disk and says
    /// nothing about chunk boundaries, while the generated takes the
    /// `scale` chapter needs are megabytes and do not belong in git. A
    /// world that had only the first could not test transfer, and one
    /// with only the second is not a world anybody could be shown.
    pub async fn start(name: &'static str, slug: &str, fixture: impl Fn(&Path)) -> Self {
        let data = tempfile::tempdir().expect("data dir");
        let auth = open_auth(data.path()).await;

        let state = {
            let _guard = ENV.lock().await;
            // SAFETY: held under `ENV` for the whole window in which
            // `AppState` reads these.
            unsafe {
                std::env::set_var("TASK_DATA_ROOT", data.path());
                std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
            }
            let data_root = org_proto::DataRoot::from_env().expect("data root");
            data_root
                .init_org(slug, name, true)
                .expect("scaffold the org");
            // Planted BEFORE the server boots, because that is the
            // order a deployment sees: `admin demo` runs, then the
            // server starts and reads what is on disk. The org's set
            // of wikis in particular is read once at boot, so a wiki
            // planted afterwards would be on disk and unreachable.
            task_server::example_org::install(&data_root.org(slug), slug)
                .expect("plant the example studio");
            AppState::new_with_auth(auth.clone(), ServerKeypair::generate_ephemeral())
                .await
                .expect("boot AppState")
        };

        let mut org = state.org(slug).expect("the org we just scaffolded");
        let tree = data.path().join("orgs").join(slug).join("files");
        std::fs::create_dir_all(&tree).expect("files dir");
        fixture(&tree);

        let key = iroh::SecretKey::generate();
        let endpoint = crate::net::bind(key.clone()).await;

        // The backend reaches other servers through the port, and knows
        // its own id so the offers it mints say where to come back to.
        //
        // Installed on the org's *own* backend, which is the one the
        // router dispatches into. Putting it on a clone leaves the served
        // backend without it, and every federated call over the wire
        // answers `Unavailable` while the in-process ones pass — which is
        // exactly how this was wired until the chapters went over the
        // wire and said so.
        task_server::attach_peering(
            &mut org,
            endpoint.id().to_string(),
            Arc::new(IrohRemotes::new(endpoint.clone())),
        );
        let backend = org.files.clone();

        // Served the way a deployment serves it: one wrapped router per
        // connection, so the identity the handshake proved is the
        // identity the gate sees. Nothing is added here — a lane this
        // harness had to mount for itself would be a lane no real peer
        // could reach, which is how the replica sync surface went
        // unmounted for as long as it did.
        let serving = endpoint.clone();
        let org_for_serving = org.clone();
        let gate = state.write_gate.clone();
        tokio::spawn(async move {
            task_server::serve_org_iroh(org_for_serving, gate, &serving).await;
        });

        Self {
            name,
            key,
            slug: slug.to_string(),
            endpoint,
            backend,
            auth,
            state,
            _data: data,
        }
    }

    /// Stop this server and start it again on the same disk.
    ///
    /// Same data root, same endpoint key, new process state: a new
    /// `AppState`, a new auth store, a freshly bound endpoint. What
    /// survives is whatever was written down, which is the only thing a
    /// restart test can honestly be about.
    ///
    /// The auth database survives too, because it is a file on the same
    /// disk. It was `sqlite::memory:` for a while, which meant accounts
    /// and sessions went with the process and the restart chapter could
    /// only ask about catalogues and admissions. That was a property of
    /// the harness rather than of the product, and it put
    /// `scenario.album.rebuild` — "delete every database and lose
    /// nothing a human wrote" — out of reach, since the thing under test
    /// there is precisely what a restart finds on disk.
    pub async fn restart(self) -> Self {
        self.restart_with(|_| {}).await
    }

    /// Stop, do something to the disk, and start again.
    ///
    /// The ordering is the whole point and it is the ordering an
    /// operator has: **stop the server, then touch its files, then
    /// start it.** `before_boot` runs with nothing holding the data
    /// root — every pool closed, the store flushed, the endpoint shut.
    ///
    /// Deleting a sqlite file out from under a live pool is not a
    /// smaller version of losing a database; it is a different event,
    /// and the one it produces here is a boot that never finishes. The
    /// rebuild chapter found that by hanging for thirty seconds, which
    /// is a fair description of what the same mistake does in
    /// production.
    pub async fn restart_with(self, before_boot: impl FnOnce(&Path)) -> Self {
        let Self {
            name,
            key,
            slug,
            endpoint,
            state: old_state,
            backend: old_backend,
            _data: data,
            ..
        } = self;
        // Close the old endpoint before rebinding the same key, or the
        // two race for the identity.
        endpoint.close().await;
        // Then everything holding the disk: the content store's own
        // handles first, then every DB pool the scope owns, in the LIFO
        // order the real shutdown path uses.
        old_backend.shutdown().await;
        old_state.scope.close().await;

        before_boot(data.path());

        // The same file the first boot opened — see `open_auth`.
        let auth = open_auth(data.path()).await;
        let state = {
            let _guard = ENV.lock().await;
            // SAFETY: held under `ENV` for the whole window in which
            // `AppState` reads these.
            unsafe {
                std::env::set_var("TASK_DATA_ROOT", data.path());
                std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
            }
            AppState::new_with_auth(auth.clone(), ServerKeypair::generate_ephemeral())
                .await
                .expect("boot AppState")
        };

        let mut org = state.org(&slug).expect("the org is still on this disk");
        // Same id, new addresses — so the book is told again, exactly as
        // a deployed endpoint republishes its pkarr record on boot. A
        // device registered against this id notices nothing, which is
        // the entire point of registering against an id.
        let endpoint = crate::net::bind(key.clone()).await;

        task_server::attach_peering(
            &mut org,
            endpoint.id().to_string(),
            Arc::new(IrohRemotes::new(endpoint.clone())),
        );
        let backend = org.files.clone();

        let serving = endpoint.clone();
        let org_for_serving = org.clone();
        let gate = state.write_gate.clone();
        tokio::spawn(async move {
            task_server::serve_org_iroh(org_for_serving, gate, &serving).await;
        });

        Self {
            name,
            key,
            slug,
            endpoint,
            backend,
            auth,
            state,
            _data: data,
        }
    }

    /// This server's own identity, as the admitted-host set records it.
    #[must_use]
    pub fn host_id(&self) -> files_domain::HostId {
        files_domain::HostId(self.endpoint.id().to_string())
    }

    /// Open the replica lane on `origin` **as this server**.
    ///
    /// The distinction from [`crate::client::Session`] is the whole
    /// point: a session dials from a fresh device endpoint and signs
    /// each call with a person's token, and this dials from the
    /// server's own endpoint and signs nothing. The identity that
    /// reaches the gate is the one iroh proved during the handshake, so
    /// what authorises the pull is `origin` having admitted this
    /// machine — not a login someone lent it.
    /// By `origin`'s bare id, like everything else here — a server that
    /// had to be handed an address could not be the thing a person
    /// registers by pasting one string.
    pub async fn dial_replica(&self, origin: &Self) -> files_sync::SyncServiceClient {
        let link = architect::iroh_link::connect(&self.endpoint, origin.endpoint.id())
            .await
            .expect("dial the origin");
        vox_core::initiator_on(link)
            .establish()
            .await
            .expect("establish the replica lane")
    }

    /// Take this server off the network, leaving its disk alone.
    ///
    /// Closes the endpoint, so a peer dialling this org's id finds
    /// nothing. Deliberately **not** a shutdown: the process is still
    /// running, the content is still on disk, and the org is still
    /// hosted. What is gone is reach — which is the distinction
    /// `files.catalogue.offline` and `project.location.degraded` are
    /// both about, and which a test that killed the whole server could
    /// not draw.
    ///
    /// There is no coming back. Rebinding the same key needs a fresh
    /// `Endpoint`, which is [`Self::restart`]; an outage that ends is a
    /// restart, and modelling it as one keeps a single path.
    pub async fn go_offline(&self) {
        self.endpoint.close().await;
    }

    /// This org's root on disk — `orgs/<slug>/`.
    ///
    /// Its vault, its wiki, its sqlite projections and its files. The
    /// rebuild chapter needs it to delete things a lane would never
    /// offer to delete.
    pub fn org_root(&self) -> std::path::PathBuf {
        self._data.path().join("orgs").join(&self.slug)
    }

    /// This org's files directory — where its trees sit on this disk.
    pub fn tree(&self) -> std::path::PathBuf {
        self._data
            .path()
            .join("orgs")
            .join(&self.slug)
            .join("files")
    }
}
