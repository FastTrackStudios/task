//! The server's iroh presence: one endpoint per org.
//!
//! [`crate::serve_org_iroh`] has existed for as long as the peering
//! lanes have, and until now nothing but the integration suite called
//! it. A deployment therefore mounted federation, admitted hosts, minted
//! offers — and answered `Unavailable` for every one of them, because
//! the process had no endpoint to dial from and no endpoint to be dialled
//! at. This is the piece that was missing.
//!
//! # One endpoint per org, not one per process
//!
//! An org is the addressable thing. `files.peering.presence` lets an org
//! live on several servers and `files.peering.serving` says every host
//! serves the whole org, so what a person registers a device against —
//! what a peer admits, what an offer names — is the org, and giving each
//! org its own endpoint makes the id they hold mean exactly that.
//!
//! The alternative, one endpoint for the process with the org chosen
//! inside the lane, makes the id mean "this machine" and then needs a
//! second field everywhere to say which org was meant. It also leaks the
//! deployment's shape: two orgs that happen to share a box would be
//! visibly the same peer, which is nobody's business but the operator's.
//!
//! # The key is the identity, so the key is on disk
//!
//! `orgs/<slug>/iroh-key.ed25519`, mode 0600, created on first boot.
//! Registration is "paste this id into a device", and an id that changed
//! whenever the process restarted would make every registration a lie
//! with a short expiry. Losing this file is losing the org's address, not
//! its data — every device registered against it has to be re-registered.
//!
//! The id is also written to `orgs/<slug>/iroh-endpoint-id` as plain
//! text, because an operator needs to read it out and paste it somewhere
//! and should not have to derive it from a secret key to do so.
//!
//! # Where the addresses come from
//!
//! Nowhere, normally: [`files::bind_endpoint`] binds with the n0 preset,
//! so an endpoint publishes itself to n0's DNS as it comes up and is
//! dialled by bare id from anywhere. That is the deployed path and it
//! needs no configuration at all.
//!
//! It also needs the internet. Two servers on one laptop with no route
//! out publish to nobody, which is exactly the demo and exactly the
//! integration suite — so `TASK_IROH_PEER_DIR` names a directory the
//! servers sharing it write their own [`iroh::EndpointAddr`] into and
//! read each other's out of. It is a stand-in for discovery, on the
//! `MemoryLookup` seam iroh already provides, and everything above it
//! still dials by bare id and cannot tell.
//!
//! Unset in a deployment. If you find yourself setting it in one, what
//! you actually want is n0 discovery working, or a LAN address-lookup
//! service — not a directory of files describing where machines were.

use std::path::{Path, PathBuf};
use std::time::Duration;

use architect::iroh_link::{self, iroh};
use files::{AddressBook, IrohRemotes};
use tracing::{info, warn};

use crate::AppState;

/// How often the peer directory is re-read.
///
/// Servers in a demo start within seconds of each other, and whichever
/// binds first writes its address before the second exists to read it. A
/// one-shot scan would leave that ordering deciding whether federation
/// works, which is the kind of flake that gets diagnosed as a protocol
/// bug.
const RESCAN: Duration = Duration::from_secs(5);

/// The endpoints this process is serving, kept alive.
///
/// An [`iroh::Endpoint`] stops accepting when the last handle drops, so
/// this is not a receipt — it is the thing keeping the server reachable.
/// Hold it for as long as the process should be dialable.
pub struct IrohHost {
    /// Org slug → the endpoint serving it.
    pub endpoints: Vec<(String, iroh::Endpoint)>,
}

impl IrohHost {
    /// What an operator reads out and a device registers against.
    #[must_use]
    pub fn ids(&self) -> Vec<(String, String)> {
        self.endpoints
            .iter()
            .map(|(slug, e)| (slug.clone(), e.id().to_string()))
            .collect()
    }
}

/// Bind an endpoint for every hosted org, install the dialler, and serve.
///
/// Call before handing `state` to [`crate::router`]: this replaces each
/// org's stored state with one whose backend holds the remotes port, and
/// a router built first would dispatch into the backends as they were.
///
/// # Failure is not fatal
///
/// A bind that fails is logged and that org is left unserved over iroh.
/// The process still serves HTTP, so the failure mode is "federation is
/// down" rather than "the server did not start" — and an operator whose
/// UDP is blocked gets a working server and a loud log rather than a
/// boot loop. Returns `None` when nothing bound at all.
pub async fn start(state: &AppState) -> Option<IrohHost> {
    if std::env::var("TASK_IROH_DISABLE").is_ok_and(|v| v != "0") {
        info!("iroh disabled by TASK_IROH_DISABLE");
        return None;
    }

    let peer_dir = std::env::var_os("TASK_IROH_PEER_DIR").map(PathBuf::from);
    let book = peer_dir.as_ref().map(|dir| {
        let book = AddressBook::new();
        // Read once before binding, so an endpoint that dials
        // immediately already knows about whoever came up first.
        absorb_addrs(dir, &book);
        book
    });

    let slugs: Vec<String> = {
        let orgs = state.orgs.read().ok()?;
        orgs.keys().cloned().collect()
    };

    let mut endpoints = Vec::new();
    for slug in slugs {
        match bind_org(state, &slug, book.clone(), peer_dir.as_deref()).await {
            Ok(endpoint) => endpoints.push((slug, endpoint)),
            Err(e) => warn!(%slug, error = %e, "iroh: this org is not reachable by endpoint id"),
        }
    }

    if endpoints.is_empty() {
        return None;
    }

    // Keep reading the peer directory. See `RESCAN`.
    if let (Some(dir), Some(book)) = (peer_dir, book) {
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(RESCAN).await;
                absorb_addrs(&dir, &book);
            }
        });
    }

    Some(IrohHost { endpoints })
}

/// Bind, publish, install and serve one org.
async fn bind_org(
    state: &AppState,
    slug: &str,
    book: Option<AddressBook>,
    peer_dir: Option<&Path>,
) -> eyre::Result<iroh::Endpoint> {
    let org_root = state.data_root.org(slug);
    let key = iroh_link::load_or_create_secret_key(&org_root.path().join("iroh-key.ed25519"))?;
    let endpoint = files::bind_endpoint(key, book).await?;
    let id = endpoint.id().to_string();

    // Readable without a key parser — see the module docs.
    std::fs::write(org_root.path().join("iroh-endpoint-id"), format!("{id}\n"))?;
    if let Some(dir) = peer_dir {
        publish(dir, slug, &endpoint);
    }

    // The backend dials from this org's own endpoint, so the id a peer
    // admits is the id it will see connecting. Installed on the org
    // *stored in the map* — `attach_peering` takes `&mut` because the
    // port is held by value, and a copy left in a local would leave the
    // router dispatching into a backend without it.
    let mut org = state
        .org(slug)
        .ok_or_else(|| eyre::eyre!("org vanished between listing and binding"))?;
    crate::attach_peering(&mut org, id.clone(), IrohRemotes::port(endpoint.clone()));
    state
        .orgs
        .write()
        .map_err(|_| eyre::eyre!("orgs lock poisoned"))?
        .insert(slug.to_owned(), org.clone());

    let serving = endpoint.clone();
    let gate = state.write_gate.clone();
    tokio::spawn(async move {
        crate::serve_org_iroh(org, gate, &serving).await;
    });

    info!(%slug, endpoint_id = %id, "iroh: serving this org");
    Ok(endpoint)
}

/// Write this endpoint's address where sibling servers will read it.
///
/// # Errors
///
/// Any filesystem error creating the directory or writing the record.
pub fn publish_addr(dir: &Path, slug: &str, endpoint: &iroh::Endpoint) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let json = serde_json::to_vec_pretty(&endpoint.addr())
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    // Write-then-rename: a reader scanning this directory on its own
    // clock must never see half a record.
    let tmp = dir.join(format!(".{slug}.addr.json.tmp"));
    std::fs::write(&tmp, json)?;
    std::fs::rename(tmp, dir.join(format!("{slug}.addr.json")))
}

/// Publish, and log rather than fail — a server that cannot write its
/// address is still a server, just one nobody offline can find.
fn publish(dir: &Path, slug: &str, endpoint: &iroh::Endpoint) {
    if let Err(e) = publish_addr(dir, slug, endpoint) {
        warn!(dir = %dir.display(), error = %e, "iroh: could not publish this endpoint's address");
    }
}

/// Read every address in `dir` into `book`.
///
/// Silently skips anything unreadable or unparseable: the directory is
/// written by peers on their own schedule, so a file being absent, half
/// written or left over from an older version is ordinary rather than an
/// error this process can act on.
pub fn absorb_addrs(dir: &Path, book: &AddressBook) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "json") {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        if let Ok(addr) = serde_json::from_slice::<iroh::EndpointAddr>(&bytes) {
            book.add_endpoint_info(addr);
        }
    }
}
