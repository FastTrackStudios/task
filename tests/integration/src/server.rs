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
//! The secret key is fresh per run. A real server persists it
//! (`EngineHost::iroh(key_path, id_path)`) so its id survives a restart
//! — which is the entire point of registering a device against an id
//! rather than an address.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use architect::iroh_link;
use files::FilesBackend;
use task_server::{AppState, AuthState, capability::ServerKeypair};

use crate::transport::IrohRemotes;

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
    known: Arc<Mutex<HashMap<String, iroh::EndpointAddr>>>,
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
    pub async fn start(
        name: &'static str,
        slug: &str,
        known: Arc<Mutex<HashMap<String, iroh::EndpointAddr>>>,
        fixture: impl Fn(&Path),
    ) -> Self {
        let data = tempfile::tempdir().expect("data dir");
        let auth = AuthState::open("sqlite::memory:", SECRET)
            .await
            .expect("auth db");

        let state = {
            let _guard = ENV.lock().await;
            // SAFETY: held under `ENV` for the whole window in which
            // `AppState` reads these.
            unsafe {
                std::env::set_var("TASK_DATA_ROOT", data.path());
                std::env::set_var("TASK_ENFORCE_PERMISSIONS", "1");
            }
            org_proto::DataRoot::from_env()
                .expect("data root")
                .init_org(slug, name, true)
                .expect("scaffold the org");
            AppState::new_with_auth(auth.clone(), ServerKeypair::generate_ephemeral())
                .await
                .expect("boot AppState")
        };

        let mut org = state.org(slug).expect("the org we just scaffolded");
        let tree = data.path().join("orgs").join(slug).join("files");
        std::fs::create_dir_all(&tree).expect("files dir");
        fixture(&tree);

        let key = iroh::SecretKey::generate();
        let endpoint = iroh_link::bind_endpoint(key.clone())
            .await
            .expect("bind endpoint");
        known
            .lock()
            .expect("known peers")
            .insert(endpoint.id().to_string(), endpoint.addr());

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
            Arc::new(IrohRemotes {
                endpoint: endpoint.clone(),
                known: Arc::clone(&known),
            }),
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
            known,
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
    /// The auth database does *not* survive — it is `sqlite::memory:`,
    /// so sessions and accounts go with the old process. That is a
    /// property of the harness rather than of the product, and it is why
    /// the restart chapter asserts about catalogues and admissions
    /// rather than about people.
    pub async fn restart(self) -> Self {
        let Self {
            name,
            key,
            known,
            slug,
            endpoint,
            _data: data,
            ..
        } = self;
        // Close the old endpoint before rebinding the same key, or the
        // two race for the identity.
        endpoint.close().await;

        let auth = AuthState::open("sqlite::memory:", SECRET)
            .await
            .expect("auth db");
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
        let endpoint = iroh_link::bind_endpoint(key.clone())
            .await
            .expect("rebind the same endpoint");
        known
            .lock()
            .expect("known peers")
            .insert(endpoint.id().to_string(), endpoint.addr());

        task_server::attach_peering(
            &mut org,
            endpoint.id().to_string(),
            Arc::new(IrohRemotes {
                endpoint: endpoint.clone(),
                known: Arc::clone(&known),
            }),
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
            known,
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
    pub async fn dial_replica(&self, origin: &Self) -> files_sync::SyncServiceClient {
        let link = architect::iroh_link::connect(&self.endpoint, origin.endpoint.addr())
            .await
            .expect("dial the origin");
        vox_core::initiator_on(link)
            .establish()
            .await
            .expect("establish the replica lane")
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
