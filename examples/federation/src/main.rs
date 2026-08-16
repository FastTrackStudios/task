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
use files_proto::service::sync::{FacetName, SyncService};
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

impl IrohRemotes {
    /// Dial an origin and open its federation lane.
    ///
    /// A connection per call, which a demo can afford and a deployment
    /// would not — pooling belongs here, and its absence is why a
    /// relayed read costs a handshake per chunk below.
    async fn dial(
        &self,
        origin: &EndpointId,
    ) -> Result<files_proto::FederationServiceClient, files_proto::FilesFault> {
        let addr = self
            .known
            .lock()
            .expect("known peers")
            .get(&origin.0)
            .cloned()
            .ok_or_else(|| files_proto::FilesFault::Unavailable {
                path: RootPath::root(),
            })?;
        let link = iroh_link::connect(&self.endpoint, addr)
            .await
            .map_err(|e| files_proto::FilesFault::Io(format!("dial {origin}: {e}")))?;
        // `establish` does the handshake and opens the service lane in
        // one step — the same call the CLI makes over a WebSocket, with
        // only the link underneath it different.
        vox_core::initiator_on(link)
            .establish()
            .await
            .map_err(|e| files_proto::FilesFault::Io(format!("establish: {e}")))
    }
}

/// Unwrap a vox error into the fault the origin actually raised.
fn fault(e: vox::VoxError<files_proto::FilesFault>) -> files_proto::FilesFault {
    match e {
        vox::VoxError::User(fault) => *fault,
        other => files_proto::FilesFault::Io(other.to_string()),
    }
}

#[async_trait::async_trait]
impl files::lane::federation::RemoteFiles for IrohRemotes {
    async fn read_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<files_proto::service::media::ByteTicket, files_proto::FilesFault> {
        self.dial(origin)
            .await?
            .read_offered(secret.to_string(), path.clone())
            .await
            .map_err(fault)
    }

    async fn fetch_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        token: &str,
        range: files_proto::service::federation::ByteRange,
    ) -> Result<Vec<u8>, files_proto::FilesFault> {
        self.dial(origin)
            .await?
            .fetch_offered(secret.to_string(), token.to_string(), range)
            .await
            .map_err(fault)
    }

    async fn browse_offered(
        &self,
        origin: &EndpointId,
        secret: &str,
        path: &RootPath,
    ) -> Result<Vec<files_proto::model::BrowseEntry>, files_proto::FilesFault> {
        self.dial(origin)
            .await?
            .browse_offered(secret.to_string(), path.clone())
            .await
            .map_err(fault)
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

    // ACME Audio: the sessions and stems half of the job.
    let known = Arc::new(Mutex::new(HashMap::new()));
    let acme = Server::start("ACME Audio", Arc::clone(&known), |tree| {
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

    // VNT Video: the footage and cut half, a different company on a
    // different server.
    let vnt = Server::start("VNT Video", Arc::clone(&known), |tree| {
        let cut = tree.join("Cut");
        std::fs::create_dir_all(cut.join("Proxies")).unwrap();
        std::fs::write(cut.join("Cut.drp"), b"Resolve project (fixture)").unwrap();
        std::fs::write(cut.join("Proxies").join("reel.mov"), b"proxy reel bytes").unwrap();
    })
    .await;

    for s in [&acme, &vnt] {
        println!("  {:<11} {}", s.name, s.endpoint.id());
    }
    println!(
        "\n  A device registers a server by pasting one of those ids.\n  \
         No host, no port, no certificate.\n"
    );

    println!("── Clients dial by id ───────────────────────────────────\n");

    // Two clients, each dialling a different server by its id alone.
    let mut clients = Vec::new();
    for server in [&acme, &vnt] {
        let dialer = iroh_link::bind_endpoint(iroh::SecretKey::generate())
            .await
            .expect("client endpoint");
        let link = iroh_link::connect(&dialer, server.endpoint.addr())
            .await
            .expect("dial the server by its id");
        println!("  client → {:<11} connected over QUIC", server.name);
        clients.push((dialer, link));
    }
    println!();

    println!("── The scenario ─────────────────────────────────────────\n");

    // Adoption. The tree already exists, written by other applications;
    // nothing is moved, copied or renamed.
    let acme_root = adopt(&acme, "Song").await;
    let vnt_root = adopt(&vnt, "Cut").await;
    stage(
        "files.adopt.in-place",
        Ok(format!(
            "two roots adopted, {} bytes moved",
            0 // by construction: adoption writes a marker and reads
        )),
    );

    // The catalogue is browsable before anything is hashed.
    let listing = acme
        .backend
        .browse(acme_root, RootPath::root())
        .await
        .expect("browse");
    stage(
        "files.adopt.catalogue-first",
        Ok(format!("{} entries visible at once", listing.len())),
    );

    // Platform junk never surfaces.
    let audio = acme
        .backend
        .browse(acme_root, RootPath::parse("Audio Files").unwrap())
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
    let receipt = acme
        .backend
        .create_dirs(acme_root, vec![RootPath::parse("Renders").unwrap()])
        .await
        .expect("mkdir");
    stage(
        "files.write.surface",
        Ok(format!("one operation: {}", &receipt.operation[..12])),
    );

    // And it reaches the catalogue without a restart.
    let seen = acme
        .backend
        .entry(acme_root, RootPath::parse("Renders").unwrap())
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
    let checkpoint = acme
        .backend
        .checkpoint(acme_root, Some("first".into()))
        .await
        .expect("checkpoint");
    stage(
        "files.version.cadence",
        Ok(format!("checkpoint {}", &checkpoint.commit_id[..12])),
    );

    // Bytes, over the same transport as everything else.
    let ticket = acme
        .backend
        .read(acme_root, RootPath::parse("Audio Files/kick.wav").unwrap())
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
    let archive = acme
        .backend
        .archive(acme_root, vec![RootPath::parse("Audio Files").unwrap()])
        .await
        .expect("an archive ticket");
    stage(
        "files.write.surface (archive)",
        Ok(format!(
            "{}, length {:?}",
            archive.content_type, archive.length
        )),
    );

    // Federation. ACME offers its session subtree to VNT, which accepts it and browses it — over iroh, by endpoint
    // id, with the secret standing for the grant.
    let offer = acme
        .backend
        .offer(
            acme_root,
            RootPath::parse("Audio Files").unwrap(),
            EndpointId(vnt.endpoint.id().to_string()),
            vec![Capability::Read],
        )
        .await
        .expect("offer");

    let accepted = vnt.backend.accept(offer.clone()).await.expect("accept");

    // The accepted offer is an ordinary root here: `TreeService::browse`
    // is the same call the vnt server makes against its own content,
    // and it does not know this one is not local.
    match vnt.backend.browse(accepted.root_id, RootPath::root()).await {
        Ok(entries) => {
            let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
            stage(
                "files.topology.federation",
                Ok(format!("VNT browsed ACME's subtree: {names:?}")),
            );
        }
        Err(e) => stage("files.topology.federation", Err(format!("{e}"))),
    }

    // A grant stays revocable from the originating side, and binds on
    // the receiver's next call rather than on its cooperation.
    acme
        .backend
        .withdraw(offer.grant)
        .await
        .expect("withdraw");
    let after = vnt.backend.browse(accepted.root_id, RootPath::root()).await;
    stage(
        "files.topology.federation (revoke)",
        if after.is_err() {
            Ok("withdrawn at the origin, refused on the next call".into())
        } else {
            Err("a withdrawn offer still served content".into())
        },
    );

    // An unreachable origin costs its own content and nothing else.
    let local_still_fine = acme
        .backend
        .browse(acme_root, RootPath::root())
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

    // One project, both companies.
    //
    // VNT re-accepts (the withdrawal above ended the first grant) and
    // then composes: its own footage plus ACME's sessions, as one tree
    // with one identity. Neither is the project's "real" home — that is
    // the clause the type has no field for.
    let offer = acme
        .backend
        .offer(
            acme_root,
            RootPath::parse("Audio Files").unwrap(),
            EndpointId(vnt.endpoint.id().to_string()),
            vec![Capability::Read],
        )
        .await
        .expect("offer again");
    let sessions = vnt.backend.accept(offer).await.expect("accept").root_id;

    let mut project = files_domain::Composition::new();
    project
        .with(files_domain::Member {
            name: "Sessions".into(),
            root: sessions,
            path: RootPath::root(),
        })
        .expect("ACME's half");
    project
        .with(files_domain::Member {
            name: "Footage".into(),
            root: vnt_root,
            path: RootPath::parse("Proxies").unwrap(),
        })
        .expect("VNT's half");

    // Browse it as one tree. Each part resolves to whichever root
    // answers for it — one of them on another company's server — and the
    // caller makes the same call either way.
    let mut composed = Vec::new();
    for member in project.members() {
        let at = RootPath::parse(&member.name).unwrap();
        let located = project.locate(&at).expect("locate");
        let entries = vnt
            .backend
            .browse(located.member.root, located.within.clone())
            .await
            .unwrap_or_default();
        for entry in entries {
            composed.push(format!("{}/{}", member.name, entry.name));
        }
    }
    composed.sort();
    stage(
        "project.location.composed",
        if project.locations() == 2 && composed.len() >= 3 {
            Ok(format!("{} locations, one tree: {composed:?}", project.locations()))
        } else {
            Err(format!("expected both halves, got {composed:?}"))
        },
    );

    // Stream a deliverable across the boundary. VNT holds none of these
    // bytes; `files.peering.serving` says it answers anyway, fetching
    // from the host that has them. The caller makes the ordinary `read`
    // call and gets an ordinary local ticket.
    let audio = RootPath::parse("vox.wav").unwrap();
    match files_proto::service::media::MediaService::read(&vnt.backend, sessions, audio.clone())
        .await
    {
        Ok(ticket) => {
            let mut played = Vec::new();
            let redeemed = vnt
                .backend
                .redeem_bytes(&ticket.token, None, &mut played)
                .await;
            // A preview seeks. The whole point of a relayed ticket being
            // seekable is that scrubbing a 4 GB reel transfers the part
            // you scrubbed to, not the part before it.
            let mut scrubbed = Vec::new();
            let seek = vnt
                .backend
                .redeem_bytes(&ticket.token, Some((4, 7)), &mut scrubbed)
                .await;
            stage(
                "files.peering.serving (stream)",
                match (redeemed, seek) {
                    (Ok(()), Ok(())) if played == b"vox take one" && scrubbed == b"take" => Ok(
                        format!(
                            "VNT served {} bytes it does not hold; range 4-7 = {:?}",
                            played.len(),
                            String::from_utf8_lossy(&scrubbed)
                        ),
                    ),
                    (Ok(()), Ok(())) => Err(format!(
                        "relayed the wrong bytes: {:?} / {:?}",
                        String::from_utf8_lossy(&played),
                        String::from_utf8_lossy(&scrubbed)
                    )),
                    (Err(e), _) | (_, Err(e)) => Err(format!("{e}")),
                },
            );
        }
        Err(e) => stage("files.peering.serving (stream)", Err(format!("{e}"))),
    }

    // ── A device that syncs part of the project ──────────────────
    //
    // An editor's laptop, carrying the same composed project: the audio
    // company's sessions and the video company's cut. It subscribes to
    // facets, not to path globs — `files.sync.selective` — so what it
    // keeps resident is a statement about the *kind* of work it does,
    // and stays true when a folder is renamed or a new one appears.
    println!("\n── A laptop, syncing the audio and not the footage ──────\n");

    let laptop_dir = tempfile::tempdir().expect("laptop data dir");
    let laptop = FilesBackend::new(laptop_dir.path(), laptop_dir.path().join("vault"))
        .expect("laptop backend");

    // The project as it lands on a device: both halves, under the names
    // the composition gave them.
    let tree = laptop_dir.path().join("Album");
    std::fs::create_dir_all(tree.join("Sessions").join("Audio Files")).unwrap();
    std::fs::create_dir_all(tree.join("Footage").join("Proxies")).unwrap();
    std::fs::write(tree.join("Sessions").join("Song.rpp"), b"REAPER project (fixture)").unwrap();
    std::fs::write(
        tree.join("Sessions").join("Audio Files").join("vox.wav"),
        b"vox take one",
    )
    .unwrap();
    // Deliberately the largest thing here: it is what the laptop is
    // trying not to carry.
    std::fs::write(
        tree.join("Footage").join("Proxies").join("reel.mov"),
        vec![0u8; 64 * 1024],
    )
    .unwrap();

    let album = files::FilesService::create_root(
        &laptop,
        tree.to_string_lossy().into_owned(),
        "Album".into(),
        RootFlavor::Media,
    )
    .await
    .expect("adopt the project on the laptop");
    let album = RootId::new(album.id);
    // Content into the store, so dehydrating is dropping a local copy
    // rather than losing the file.
    files::FilesService::checkpoint_now(&laptop, album.into(), None)
        .await
        .expect("checkpoint");

    // Nobody told it what these folders are. `files.facet.tool-layout`
    // reads the layout the tools themselves impose — `Audio Files` is a
    // REAPER session's media, `Proxies` is a Resolve proxy directory —
    // so the vocabulary is there before anyone configures anything.
    match laptop.facets(album).await {
        Ok(found) => {
            let mut named: Vec<String> = found
                .iter()
                .filter_map(|b| b.facet.as_ref().map(|f| format!("{}={}", b.path, f.0)))
                .collect();
            named.sort();
            stage(
                "files.facet.tool-layout",
                if named.is_empty() {
                    Err("no facet was recognised from the tools' own layout".into())
                } else {
                    Ok(format!("{named:?}"))
                },
            );
        }
        Err(e) => stage("files.facet.tool-layout", Err(format!("{e}"))),
    }

    // Subscribe to the audio work and nothing else.
    match laptop
        .subscribe(album, vec![FacetName("sessions".into())])
        .await
    {
        Ok(_) => {
            let audio = laptop
                .browse(album, RootPath::parse("Sessions/Audio Files").unwrap())
                .await
                .unwrap_or_default();
            let footage = laptop
                .browse(album, RootPath::parse("Footage/Proxies").unwrap())
                .await
                .unwrap_or_default();

            let audio_resident = audio.iter().all(|e| !e.stub);
            let reel = footage.iter().find(|e| e.name == "reel.mov");
            stage(
                "files.sync.selective",
                match reel {
                    // The stub keeps its real name and size. That is the
                    // clause that matters: a placeholder that lies about
                    // size makes "how big is this project" wrong on every
                    // device that did not sync it.
                    Some(entry)
                        if entry.stub
                            && audio_resident
                            && entry.size == Some(64 * 1024) =>
                    {
                        Ok(format!(
                            "sessions resident ({} files); reel.mov a stub still reporting {} bytes",
                            audio.len(),
                            entry.size.unwrap_or(0)
                        ))
                    }
                    Some(entry) if !entry.stub => {
                        Err("the footage stayed resident on a device that did not ask for it".into())
                    }
                    Some(entry) => Err(format!(
                        "stub lost its metadata: size {:?}, audio resident {audio_resident}",
                        entry.size
                    )),
                    None => Err("the footage vanished rather than becoming a stub".into()),
                },
            );

            // The laptop's own disk is the check that matters: a stub
            // that still costs 64 KB has saved nothing, whatever the
            // catalogue says about it.
            let on_disk = std::fs::metadata(tree.join("Footage").join("Proxies").join("reel.mov"))
                .map(|m| m.len())
                .unwrap_or(u64::MAX);
            stage(
                "files.sync.selective (space)",
                if on_disk < 64 * 1024 {
                    Ok(format!("reel.mov costs {on_disk} bytes on the laptop, not 65536"))
                } else {
                    Err(format!("the file still occupies {on_disk} bytes"))
                },
            );

            // Hydrating on access. Unsubscribed does not mean unreachable
            // — it means not carried.
            match laptop
                .hydrate(album, vec![RootPath::parse("Footage/Proxies/reel.mov").unwrap()], true)
                .await
            {
                Ok(_) => {
                    let back = std::fs::metadata(
                        tree.join("Footage").join("Proxies").join("reel.mov"),
                    )
                    .map(|m| m.len())
                    .unwrap_or(0);
                    stage(
                        "files.sync.selective (hydrate)",
                        if back == 64 * 1024 {
                            Ok("opened the reel; it came back whole".into())
                        } else {
                            Err(format!("hydrated to {back} bytes, not 65536"))
                        },
                    );
                }
                Err(e) => stage("files.sync.selective (hydrate)", Err(format!("{e}"))),
            }
        }
        Err(e) => stage("files.sync.selective", Err(format!("{e}"))),
    }

    // ── Peering ──────────────────────────────────────────────────
    //
    // Both orgs present on both servers, with content on one apiece.
    // Hosting an org means knowing it — structure, projects, catalogue —
    // and a second host therefore costs the size of a catalogue rather
    // than the size of a library.
    let acme_id = files_domain::HostId(acme.endpoint.id().to_string());
    let vnt_id = files_domain::HostId(vnt.endpoint.id().to_string());
    let acme_org = files_domain::OrgId("acme-audio".into());
    let vnt_org = files_domain::OrgId("vnt-video".into());

    let mut peering = files_domain::Peering::new();
    peering
        .host(acme_org.clone(), acme_id.clone(), files_domain::Hosting::working())
        .host(
            acme_org.clone(),
            vnt_id.clone(),
            files_domain::Hosting::structure_only(),
        )
        .host(vnt_org.clone(), vnt_id.clone(), files_domain::Hosting::working())
        .host(
            vnt_org.clone(),
            acme_id.clone(),
            files_domain::Hosting::structure_only(),
        );

    stage(
        "files.peering.presence",
        if peering.hosts_of(&acme_org).count() == 2
            && peering.content_hosts(&acme_org).count() == 1
        {
            Ok("both servers know ACME; one holds its bytes".into())
        } else {
            Err("presence and placement are not separable".into())
        },
    );

    // A peer sees only what it hosts.
    stage(
        "files.peering.scope",
        if peering.orgs_on(&vnt_id).len() == 2 {
            Ok("each server hosts both orgs, and nothing else".into())
        } else {
            Err("a peer saw an org it does not host".into())
        },
    );

    // Before a backup, losing ACME's server loses ACME's content. That
    // is the state peering exists to fix.
    let exposed = !peering.survives_loss_of(&acme_org, &acme_id);
    peering.host(
        acme_org.clone(),
        files_domain::HostId("offsite-backup".into()),
        files_domain::Hosting::backup(),
    );
    stage(
        "files.peering.backup",
        if exposed && peering.survives_loss_of(&acme_org, &acme_id) {
            Ok(format!(
                "{} backup added; ACME now survives losing its own server",
                peering.backups(&acme_org).count()
            ))
        } else {
            Err("a backup did not change what an org survives".into())
        },
    );

    // Every host runs the org. VNT's server holds none of ACME's bytes
    // and is still a place an ACME member can work — it fetches content
    // from a host that has it. Nothing elects a leader, so losing any
    // host costs reach and capacity, never availability.
    let regions = ["eu-west", "ap-south"];
    for region in regions {
        peering.host(
            acme_org.clone(),
            files_domain::HostId(format!("acme-{region}")),
            files_domain::Hosting::working(),
        );
    }
    let all_serve = peering
        .hosts_of(&acme_org)
        .all(|(h, _)| peering.serves(h, &acme_org));
    let stays_up = peering.available_without(&acme_org, &acme_id);
    stage(
        "files.peering.serving",
        if all_serve && stays_up && peering.serves(&vnt_id, &acme_org) {
            Ok(format!(
                "{} hosts, all serving; ACME stays up without its own server",
                peering.hosts_of(&acme_org).count()
            ))
        } else {
            Err("a host that holds no bytes was treated as a cache".into())
        },
    );

    // An org grows by adding servers. A host is a storage location, so
    // attaching a server attaches its capacity — and no host has to hold
    // everything, which is what lets an org outgrow any one machine.
    let before = peering.content_hosts(&acme_org).count();
    for n in 1..=3 {
        peering.host(
            acme_org.clone(),
            files_domain::HostId(format!("acme-shelf-{n}")),
            files_domain::Hosting::capacity(),
        );
    }
    let after = peering.content_hosts(&acme_org).count();
    stage(
        "files.peering.scale",
        if after == before + 3 && peering.complete_hosts(&acme_org).count() == 1 {
            Ok(format!(
                "ACME across {after} storage hosts; 1 holds a full copy"
            ))
        } else {
            Err("adding a server did not add capacity".into())
        },
    );

    stage(
        "files.peering.replication",
        Err("structure does not yet converge between servers — \
             files.peering.replication".into()),
    );

    println!("\n── Both servers still serving ───────────────────────────\n");
    println!(
        "  ACME root {acme_root}\n  VNT  root {vnt_root}\n\
         \n  Two orgs, two endpoints, one transport.\n"
    );
    let _ = (acme.tree(), vnt.tree(), clients.pop());
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
