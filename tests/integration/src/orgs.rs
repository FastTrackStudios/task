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

/// `len` bytes that do not compress to nothing and are the same every
/// run.
///
/// A deterministic pattern rather than random: two calls with the same
/// `seed` must produce identical files or the dedup assertion is
/// measuring the random number generator. Not all-zeroes either — a
/// content-defined chunker splits on the data, and a uniform file gives
/// it nothing to split on.
fn take_bytes(len: usize, seed: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(len);
    let mut x = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    while out.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        out.extend_from_slice(&x.to_le_bytes());
    }
    out.truncate(len);
    out
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
            // One take big enough to cross a chunk boundary. Everything
            // else here is a few hundred bytes against a 1 MiB average
            // chunk, so without this the whole storage layer — chunking,
            // dedup, what a transfer resumes from — is exercised by
            // nothing. Generated rather than committed: a fixture that
            // has to be large is a fixture that does not belong in git.
            std::fs::write(
                session.join("Audio Files").join("drums.wav"),
                take_bytes(3 << 20, 1),
            )
            .unwrap();
            // Byte-identical to the take above. Two files with the same
            // content are the ordinary case in a session folder — a
            // bounced stem, a duplicated take — and what the store does
            // about it is the difference between a project costing its
            // size and costing twice its size.
            std::fs::write(
                session.join("Audio Files").join("drums-copy.wav"),
                take_bytes(3 << 20, 1),
            )
            .unwrap();
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
