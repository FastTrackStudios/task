//! One concept: a server.
//!
//! An org's Files backend, bound to its own iroh endpoint, serving the
//! Files lanes on it. Nothing here knows about the scenario, the other
//! company, or the people — a server is just a machine an org runs.
//!
//! # Registration is an endpoint id
//!
//! The endpoint's id is the whole address. A device is registered by
//! pasting one of these in: no host, no port, no certificate. iroh
//! finds a path, traverses the NAT, and falls back to a relay only if
//! it must; the caller never learns which happened.
//!
//! The secret key is fresh per run because this is a demo. A real
//! server persists it (`EngineHost::iroh(key_path, id_path)`) so its id
//! survives a restart — which is the entire point of registering a
//! device against an id rather than an address.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use architect::{LayerRouter, iroh_link};
use files::FilesBackend;

use crate::transport::IrohRemotes;

pub struct Server {
    pub name: &'static str,
    pub endpoint: iroh::Endpoint,
    pub backend: FilesBackend,
    _data: tempfile::TempDir,
}

impl Server {
    /// Bind an endpoint and start serving the Files lanes on it.
    ///
    /// The secret key is fresh per run because this is a demo; a real
    /// server persists it (`EngineHost::iroh(key_path, id_path)`) so its
    /// id survives a restart — which is the entire point of registering
    /// a device against an id rather than an address.
    pub async fn start(
        name: &'static str,
        known: Arc<Mutex<HashMap<String, iroh::EndpointAddr>>>,
        fixture: impl Fn(&Path),
    ) -> Self {
        let data = tempfile::tempdir().expect("data dir");
        let tree = data.path().join("tree");
        std::fs::create_dir_all(&tree).expect("tree");
        fixture(&tree);

        let key = iroh::SecretKey::generate();
        let endpoint = iroh_link::bind_endpoint(key).await.expect("bind endpoint");
        known
            .lock()
            .expect("known peers")
            .insert(endpoint.id().to_string(), endpoint.addr());

        // The backend reaches other servers through the port, and knows
        // its own id so the offers it mints say where to come back to.
        let backend = FilesBackend::new(data.path(), data.path().join("vault"))
            .expect("files backend")
            .with_remotes(
                endpoint.id().to_string(),
                Arc::new(IrohRemotes {
                    endpoint: endpoint.clone(),
                    known: Arc::clone(&known),
                }),
            );

        let router = LayerRouter::new()
            .merge(files::roots_layer(backend.clone()))
            .merge(files::tree_layer(backend.clone()))
            .merge(files::write_layer(backend.clone()))
            .merge(files::version_layer(backend.clone()))
            .merge(files::media_layer(backend.clone()))
            .merge(files::media_stream_layer(backend.clone()))
            .merge(files::federation_layer(backend.clone()))
            // The replica sync surface. Structure converges over this:
            // commits and trees say what exists, manifests say how big
            // it is, and a peer pulls the graph without the chunks.
            .merge(files_sync::layer(files_sync::SyncHost::new(
                backend.clone(),
            )));

        let serving = endpoint.clone();
        tokio::spawn(async move {
            iroh_link::serve_router(&serving, router).await;
        });

        Self {
            name,
            endpoint,
            backend,
            _data: data,
        }
    }

    pub fn tree(&self) -> std::path::PathBuf {
        self._data.path().join("tree")
    }
}
