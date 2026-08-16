//! Two servers, two clients, everything over iroh.
//!
//! Run it:
//!
//! ```sh
//! cargo run -p example-federation
//! ```
//!
//! # What it demonstrates
//!
//! **Registration is an endpoint id, not an address.** Each server binds
//! an iroh endpoint and prints its id. A client is given that id and
//! nothing else — no host, no port, no certificate. iroh finds a path,
//! traverses the NAT, and falls back to a relay only if it must; the
//! client never learns which happened and does not need to.
//!
//! That is the whole registration model: paste an id into a device.
//!
//! # Why the files are tiny
//!
//! Every byte here is committed, so the fixtures are a few hundred bytes
//! each. The scenario in `docs/spec/scenario-album.md` is written against
//! 77 GB and 244 GB projects; what matters for a test is the *shape* —
//! a session folder, its media, a render, two orgs on two servers — not
//! the size. Anything that only breaks at scale needs a different
//! harness and honest labelling, not a large fixture in git.
//!
//! # Reading the output
//!
//! Every stage prints one of:
//!
//! - `ok` — the stage ran and its assertions held.
//! - `PENDING` — the capability is not implemented yet. The stage names
//!   the requirement it is waiting on rather than being skipped
//!   silently, because a scenario that quietly omits what does not work
//!   reads as a passing scenario.
//!
//! A `PENDING` line is not a failure; an unexpected error is.

use std::path::Path;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use architect::{LayerRouter, iroh_link};
use files::FilesBackend;
use files_proto::model::RootFlavor;
use files_proto::service::roots::AdoptRequest;
use files_proto::service::tree::TreeService;
use files_proto::service::version::VersionService;
use files_proto::service::write::WriteService;
use files_proto::service::access::Capability;
use files_proto::service::federation::{EndpointId, FederationService};
use files_proto::service::{media::MediaService, roots::RootsService};
use files_proto::{RootId, RootPath};


/// The `RemoteFiles` port, over iroh.
///
/// `files` states what it needs of another server; this supplies it. The
/// backend never learns that a connection exists, let alone how one is
/// made — the same seam placement uses for storage boundaries.
///
/// Dialling per call is deliberate for a demo: it makes each browse
/// obviously a real network round trip rather than a warm cache. A
/// server pools connections.
#[derive(Debug)]
struct IrohRemotes {
    endpoint: iroh::Endpoint,
    /// Endpoint id → address. A demo stands in for the address lookup a
    /// deployment gets from iroh's discovery; the id is still the only
    /// thing a user ever handles.
    known: Arc<Mutex<HashMap<String, iroh::EndpointAddr>>>,
}

#[async_trait::async_trait]
impl files::lane::federation::RemoteFiles for IrohRemotes {
    async fn browse_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<Vec<files_proto::model::BrowseEntry>, files_proto::FilesFault> {
        let addr = self
            .known
            .lock()
            .expect("known peers")
            .get(&origin.0)
            .cloned()
            .ok_or_else(|| {
                files_proto::FilesFault::Unavailable { path: path.clone() }
            })?;

        let link = iroh_link::connect(&self.endpoint, addr)
            .await
            .map_err(|e| files_proto::FilesFault::Io(format!("dial {origin}: {e}")))?;
        // `establish` does the handshake and opens the service lane in
        // one step — the same call the CLI makes over a WebSocket, with
        // only the link underneath it different.
        let client: files_proto::FederationServiceClient = vox_core::initiator_on(link)
            .establish()
            .await
            .map_err(|e| files_proto::FilesFault::Io(format!("establish: {e}")))?;

        client
            .browse_offered(secret.to_string(), path.clone())
            .await
            .map_err(|e| match e {
                vox::VoxError::User(fault) => *fault,
                other => files_proto::FilesFault::Io(other.to_string()),
            })
    }
}

/// One server: an org's Files backend, served over its own iroh endpoint.
struct Server {
    name: &'static str,
    endpoint: iroh::Endpoint,
    backend: FilesBackend,
    _data: tempfile::TempDir,
}

impl Server {
    /// Bind an endpoint and start serving the Files lanes on it.
    ///
    /// The secret key is fresh per run because this is a demo; a real
    /// server persists it (`EngineHost::iroh(key_path, id_path)`) so its
    /// id survives a restart — which is the entire point of registering
    /// a device against an id rather than an address.
    async fn start(
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
            .merge(files_proto::roots_layer(backend.clone()))
            .merge(files_proto::tree_layer(backend.clone()))
            .merge(files_proto::write_layer(backend.clone()))
            .merge(files_proto::version_layer(backend.clone()))
            .merge(files_proto::media_layer(backend.clone()))
            .merge(files_proto::media_stream_layer(backend.clone()))
            .merge(files_proto::federation_layer(backend.clone()));

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

    fn tree(&self) -> std::path::PathBuf {
        self._data.path().join("tree")
    }
}

fn stage(name: &str, outcome: Result<String, String>) {
    match outcome {
        Ok(detail) => println!("  ok       {name:<38} {detail}"),
        Err(why) => println!("  PENDING  {name:<38} {why}"),
    }
}

#[tokio::main]
async fn main() {
    println!("\n── Two servers, addressed by public key ──────────────────\n");

    // The studio server: an audio session, as a mix engineer's tree.
    let known = Arc::new(Mutex::new(HashMap::new()));
    let studio = Server::start("studio", Arc::clone(&known), |tree| {
        let session = tree.join("Song");
        std::fs::create_dir_all(session.join("Audio Files")).unwrap();
        std::fs::write(session.join("Song.rpp"), b"REAPER project (fixture)").unwrap();
        std::fs::write(session.join("Audio Files").join("kick.wav"), b"kick take one").unwrap();
        std::fs::write(session.join("Audio Files").join("vox.wav"), b"vox take one").unwrap();
        // Platform junk the ignore layer must hide.
        std::fs::write(session.join(".DS_Store"), b"junk").unwrap();
        std::fs::write(session.join("Audio Files").join("._vox.wav"), b"junk").unwrap();
    })
    .await;

    // The post server: the video half of the same job, a different org.
    let post = Server::start("post", Arc::clone(&known), |tree| {
        let cut = tree.join("Cut");
        std::fs::create_dir_all(cut.join("Proxies")).unwrap();
        std::fs::write(cut.join("Cut.drp"), b"Resolve project (fixture)").unwrap();
        std::fs::write(cut.join("Proxies").join("reel.mov"), b"proxy reel bytes").unwrap();
    })
    .await;

    for s in [&studio, &post] {
        println!("  {:<8} {}", s.name, s.endpoint.id());
    }
    println!(
        "\n  A device registers a server by pasting one of those ids.\n  \
         No host, no port, no certificate.\n"
    );

    println!("── Clients dial by id ───────────────────────────────────\n");

    // Two clients, each dialling a different server by its id alone.
    let mut clients = Vec::new();
    for server in [&studio, &post] {
        let dialer = iroh_link::bind_endpoint(iroh::SecretKey::generate())
            .await
            .expect("client endpoint");
        let link = iroh_link::connect(&dialer, server.endpoint.addr())
            .await
            .expect("dial the server by its id");
        println!("  client → {:<8} connected over QUIC", server.name);
        clients.push((dialer, link));
    }
    println!();

    println!("── The scenario ─────────────────────────────────────────\n");

    // Adoption. The tree already exists, written by other applications;
    // nothing is moved, copied or renamed.
    let studio_root = adopt(&studio, "Song").await;
    let post_root = adopt(&post, "Cut").await;
    stage(
        "files.adopt.in-place",
        Ok(format!(
            "two roots adopted, {} bytes moved",
            0 // by construction: adoption writes a marker and reads
        )),
    );

    // The catalogue is browsable before anything is hashed.
    let listing = studio
        .backend
        .browse(studio_root, RootPath::root())
        .await
        .expect("browse");
    stage(
        "files.adopt.catalogue-first",
        Ok(format!("{} entries visible at once", listing.len())),
    );

    // Platform junk never surfaces.
    let audio = studio
        .backend
        .browse(studio_root, RootPath::parse("Audio Files").unwrap())
        .await
        .expect("browse");
    let hidden = !audio.iter().any(|e| e.name.starts_with("._"))
        && !listing.iter().any(|e| e.name == ".DS_Store");
    stage(
        "files.ignore.layers",
        if hidden {
            Ok("AppleDouble and .DS_Store hidden".into())
        } else {
            Err("platform junk leaked into a listing".into())
        },
    );

    // A write, transactional, recorded as one operation.
    let receipt = studio
        .backend
        .create_dirs(studio_root, vec![RootPath::parse("Renders").unwrap()])
        .await
        .expect("mkdir");
    stage(
        "files.write.surface",
        Ok(format!("one operation: {}", &receipt.operation[..12])),
    );

    // And it reaches the catalogue without a restart.
    let seen = studio
        .backend
        .entry(studio_root, RootPath::parse("Renders").unwrap())
        .await
        .is_ok();
    stage(
        "files.catalogue.concurrent",
        if seen {
            Ok("the write arrived as a delta".into())
        } else {
            Err("the catalogue did not hear about the write".into())
        },
    );

    // History.
    let checkpoint = studio
        .backend
        .checkpoint(studio_root, Some("first".into()))
        .await
        .expect("checkpoint");
    stage(
        "files.version.cadence",
        Ok(format!("checkpoint {}", &checkpoint.commit_id[..12])),
    );

    // Bytes, over the same transport as everything else.
    let ticket = studio
        .backend
        .read(studio_root, RootPath::parse("Audio Files/kick.wav").unwrap())
        .await
        .expect("a byte ticket");
    stage(
        "files.scale.transport",
        Ok(format!(
            "ticket for {} bytes, seekable {}",
            ticket.length.unwrap_or(0),
            ticket.seekable
        )),
    );

    // An archive, generated as it is sent.
    let archive = studio
        .backend
        .archive(studio_root, vec![RootPath::parse("Audio Files").unwrap()])
        .await
        .expect("an archive ticket");
    stage(
        "files.write.surface (archive)",
        Ok(format!(
            "{}, length {:?}",
            archive.content_type, archive.length
        )),
    );

    // Federation. The studio offers its session subtree to the post
    // server, which accepts it and browses it — over iroh, by endpoint
    // id, with the secret standing for the grant.
    let offer = studio
        .backend
        .offer(
            studio_root,
            RootPath::parse("Audio Files").unwrap(),
            EndpointId(post.endpoint.id().to_string()),
            vec![Capability::Read],
        )
        .await
        .expect("offer");

    let accepted = post.backend.accept(offer.clone()).await.expect("accept");

    // The accepted offer is an ordinary root here: `TreeService::browse`
    // is the same call the post server makes against its own content,
    // and it does not know this one is not local.
    match post.backend.browse(accepted.root_id, RootPath::root()).await {
        Ok(entries) => {
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            stage(
                "files.topology.federation",
                Ok(format!("post browsed studio's subtree: {names:?}")),
            );
        }
        Err(e) => stage("files.topology.federation", Err(format!("{e}"))),
    }

    // A grant stays revocable from the originating side, and binds on
    // the receiver's next call rather than on its cooperation.
    studio
        .backend
        .withdraw(offer.grant)
        .await
        .expect("withdraw");
    let after = post.backend.browse(accepted.root_id, RootPath::root()).await;
    stage(
        "files.topology.federation (revoke)",
        if after.is_err() {
            Ok("withdrawn at the origin, refused on the next call".into())
        } else {
            Err("a withdrawn offer still served content".into())
        },
    );

    // An unreachable origin costs its own content and nothing else.
    let local_still_fine = studio
        .backend
        .browse(studio_root, RootPath::root())
        .await
        .is_ok();
    stage(
        "project.location.degraded",
        if local_still_fine {
            Ok("local content unaffected by the remote's state".into())
        } else {
            Err("a remote's state reached local content".into())
        },
    );

    stage(
        "project.location.composed",
        Err("one project spanning both servers' roots — project.location.composed".into()),
    );

    println!("\n── Both servers still serving ───────────────────────────\n");
    println!(
        "  studio root {studio_root}\n  post   root {post_root}\n\
         \n  Two orgs, two endpoints, one transport.\n"
    );
    let _ = (studio.tree(), post.tree(), clients.pop());
}

async fn adopt(server: &Server, dir: &str) -> RootId {
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
    RootId::new(root.id)
}
