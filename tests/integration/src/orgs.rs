//! One concept: the two companies.
//!
//! ACME Audio holds the sessions and stems. VNT Video holds the footage
//! and the cut. Separate companies, separate machines, one project —
//! which is the whole reason the rest of the example is interesting.
//! Nothing here is one org talking to itself.
//!
//! This file owns the fixtures too, so "what is on each server's disk
//! at the start" is one thing you can read in one place.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use files::RootId;
use files::model::RootFlavor;
use files::service::roots::{AdoptRequest, RootsService};

use crate::server::Server;

/// Adopt a directory that is already there, and wait for it.
///
/// `files.adopt.in-place`: nothing is moved, copied or renamed — the
/// applications that wrote this tree keep writing it.
///
/// The wait is the second half. `adopt` returns as soon as the root has
/// an identity and reads the tree behind that, so a scenario that did
/// not wait would be asking about a project mid-adoption and getting
/// different answers on different runs.
pub async fn adopt(server: &Server, dir: &str) -> RootId {
    let path = server.tree().join(dir);
    let root = server
        .backend
        .adopt(AdoptRequest {
            path: path.to_string_lossy().into_owned(),
            name: dir.to_string(),
            flavor: RootFlavor::Media,
            hash_content: true,
        })
        .await
        .expect("adopt");
    let root = RootId::new(root.id);
    server.backend.settled(root).await;
    root
}

/// Both companies, booted and reachable by endpoint id.
pub struct Orgs {
    pub acme: Server,
    pub vnt: Server,
    /// Endpoint id → address. A demo stands in for the address lookup a
    /// deployment gets from iroh's discovery; the id is still the only
    /// thing a person ever handles.
    pub known: Arc<Mutex<HashMap<String, iroh::EndpointAddr>>>,
}

impl Orgs {
    /// Start both servers with the fixtures the scenario needs.
    pub async fn boot() -> Self {
        let known = Arc::new(Mutex::new(HashMap::new()));

        // ACME Audio: the sessions and stems half of the job.
        let acme = Server::start("ACME Audio", "acme-audio", Arc::clone(&known), |tree| {
            let session = tree.join("Song");
            std::fs::create_dir_all(session.join("Audio Files")).unwrap();
            std::fs::write(session.join("Song.rpp"), b"REAPER project (fixture)").unwrap();
            std::fs::write(
                session.join("Audio Files").join("kick.wav"),
                b"kick take one",
            )
            .unwrap();
            std::fs::write(session.join("Audio Files").join("vox.wav"), b"vox take one").unwrap();
            // What leaves the building. The client sees this and nothing
            // else; the session internals above are not their business.
            std::fs::create_dir_all(session.join("Deliverables")).unwrap();
            std::fs::write(session.join("Deliverables").join("mix-v1.wav"), b"the mix").unwrap();
            // Platform junk the ignore layer must hide.
            std::fs::write(session.join(".DS_Store"), b"junk").unwrap();
            std::fs::write(session.join("Audio Files").join("._vox.wav"), b"junk").unwrap();
        })
        .await;

        // VNT Video: the footage and cut half, a different company on a
        // different server.
        let vnt = Server::start("VNT Video", "vnt-video", Arc::clone(&known), |tree| {
            let cut = tree.join("Cut");
            std::fs::create_dir_all(cut.join("Proxies")).unwrap();
            std::fs::write(cut.join("Cut.drp"), b"Resolve project (fixture)").unwrap();
            std::fs::write(cut.join("Proxies").join("reel.mov"), b"proxy reel bytes").unwrap();
        })
        .await;

        Self { acme, vnt, known }
    }
}
