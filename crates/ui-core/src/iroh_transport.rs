//! The native iroh transport — dialling an org's server by bare
//! endpoint id.
//!
//! This is the registration model the rest of the platform runs on
//! (`tests/integration/src/lib.rs`: "paste an id into a device"): a
//! server binds an iroh endpoint per org, its id is the whole address —
//! no host, no port, no certificate — and a device dials it from its
//! own endpoint. The WebSocket URL is the browser's transport and the
//! dev fallback; a native client that knows an org's endpoint id (from
//! `/.well-known/task-server.json` discovery — see
//! [`note_org_endpoints`]) goes over iroh.
//!
//! # Identity
//!
//! A WebSocket presents the session on the upgrade; an iroh connection
//! has no upgrade. What it has instead is per-call metadata: the same
//! `authorization: Bearer <token>` the server's gate already parses,
//! pushed by [`BearerMiddleware`] — attached ONCE, to the root caller,
//! via `Caller::with_global_middleware`, so every typed client built
//! over the connection presents it on every call. That caller-level,
//! service-agnostic seam is exactly what the CLI's `dial_authenticated`
//! documents wishing for; it exists as of vox's
//! `feat/global-client-middleware` branch (see the `[patch.crates-io]`
//! note in the workspace manifest).
//!
//! # The device endpoint
//!
//! One per process, bound on first dial, its key persisted so the
//! device's own id survives restarts — an id another party registered
//! must keep meaning this device (`TASK_DEVICE_KEY` overrides the
//! path). Locally, with no internet for n0 discovery, the address gap
//! is bridged the same way the demo's servers bridge it:
//! `TASK_IROH_PEER_DIR` names a directory of published
//! `iroh::EndpointAddr` records (see `apps/server/src/iroh_host.rs`),
//! absorbed into the endpoint's address book before every dial.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, RwLock};

use architect::iroh_link::{self, iroh};
use iroh::address_lookup::memory::MemoryLookup;

/// Where discovery's slug → endpoint-id knowledge lives. Plain statics,
/// like the session token: `caller_for` is a free async fn and cannot
/// reach a dioxus context.
fn known() -> &'static RwLock<HashMap<String, String>> {
    static KNOWN: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    KNOWN.get_or_init(|| RwLock::new(HashMap::new()))
}

/// Record what discovery learned. Slugs the payload carried no id for
/// are *removed* — a server that stopped serving iroh must not keep
/// being dialled over it off stale knowledge.
pub fn note_org_endpoints<'a>(orgs: impl IntoIterator<Item = (&'a str, Option<&'a str>)>) {
    let mut known = known().write().expect("iroh endpoint registry poisoned");
    for (slug, id) in orgs {
        match id {
            Some(id) if !id.trim().is_empty() => {
                known.insert(slug.to_owned(), id.trim().to_owned());
            }
            _ => {
                known.remove(slug);
            }
        }
    }
}

/// The endpoint id to dial for `slug`, if discovery has produced one
/// and nothing has opted the process out (`TASK_VOX_FORCE_WS=1` — the
/// escape hatch while this transport is young).
#[must_use]
pub fn org_endpoint_id(slug: &str) -> Option<String> {
    if std::env::var("TASK_VOX_FORCE_WS").is_ok_and(|v| v != "0" && !v.is_empty()) {
        return None;
    }
    known()
        .read()
        .expect("iroh endpoint registry poisoned")
        .get(slug)
        .cloned()
}

/// Per-call bearer, the shape the integration suite's `Bearer` proved
/// against the real gate: `authorization: Bearer <token>` string
/// metadata on every call.
pub struct BearerMiddleware(pub String);

impl vox_types::ClientMiddleware for BearerMiddleware {
    fn pre<'a, 'call>(
        &'a self,
        _context: &'a vox_types::ClientContext<'a>,
        request: &'a mut vox_types::ClientRequest<'call, 'a>,
    ) -> vox_types::BoxMiddlewareFuture<'a> {
        let value = format!("Bearer {}", self.0);
        Box::pin(async move {
            request.push_string_metadata("authorization", value);
        })
    }
}

/// Where this device's endpoint key lives. `TASK_DEVICE_KEY` wins;
/// default is the app's data dir.
fn device_key_path() -> PathBuf {
    if let Ok(explicit) = std::env::var("TASK_DEVICE_KEY") {
        return PathBuf::from(explicit);
    }
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var("HOME")
                .ok()
                .map(|h| Path::new(&h).join(".local/share"))
        })
        .unwrap_or_else(std::env::temp_dir);
    base.join("task").join("iroh-device.ed25519")
}

/// The device endpoint every dial goes out from. One, bound on first
/// use, key persisted — the same "sessions share one device endpoint"
/// shape the integration harness settled on after per-session endpoints
/// produced drop-order flakes.
async fn device_endpoint() -> Result<iroh::Endpoint, String> {
    static DEVICE: tokio::sync::OnceCell<(iroh::Endpoint, Option<MemoryLookup>)> =
        tokio::sync::OnceCell::const_new();
    let (endpoint, book) = DEVICE
        .get_or_try_init(|| async {
            let path = device_key_path();
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let key = iroh_link::load_or_create_secret_key(&path)
                .map_err(|e| format!("device key {}: {e}", path.display()))?;
            let book = std::env::var_os("TASK_IROH_PEER_DIR").map(|_| MemoryLookup::new());
            let mut builder = iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                .secret_key(key)
                .alpns(vec![iroh_link::VOX_ALPN.to_vec()]);
            if let Some(book) = &book {
                builder = builder.address_lookup(book.clone());
            }
            let endpoint = builder
                .bind()
                .await
                .map_err(|e| format!("bind device endpoint: {e}"))?;
            tracing::info!(id = %endpoint.id(), "iroh: device endpoint bound");
            Ok::<_, String>((endpoint, book))
        })
        .await?;
    // Re-absorb the peer directory on every dial rather than on a
    // timer: a dial is exactly the moment stale addresses hurt, and a
    // directory scan is microseconds.
    if let (Some(book), Some(dir)) = (book, std::env::var_os("TASK_IROH_PEER_DIR")) {
        absorb_peer_dir(Path::new(&dir), book);
    }
    Ok(endpoint.clone())
}

/// Read every published `EndpointAddr` record in `dir` into the book —
/// the client half of `iroh_host::publish_addr`, same JSON, same
/// tolerance: an unreadable or half-written record is skipped, because
/// the publisher writes-then-renames and the reader is on its own clock.
fn absorb_peer_dir(dir: &Path, book: &MemoryLookup) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        match serde_json::from_slice::<iroh::EndpointAddr>(&bytes) {
            Ok(addr) => {
                book.add_endpoint_info(addr);
            }
            Err(e) => {
                tracing::debug!(file = %entry.path().display(), error = %e,
                    "iroh: unparseable peer record skipped");
            }
        }
    }
}

/// One raw lane over an iroh connection — [`vox_core::FromVoxLane`] for
/// the untyped root, mirroring `vox_clients::RootLane` but with both
/// halves reachable, since the dial needs the caller *before* wrapping
/// it with the bearer.
struct RawLane {
    caller: vox_core::Caller,
    connection: Option<vox_core::ConnectionHandle>,
}

impl vox_core::FromVoxLane for RawLane {
    const SERVICE_NAME: &'static str = "Noop";

    fn from_vox_lane(
        caller: vox_core::Caller,
        connection: Option<vox_core::ConnectionHandle>,
    ) -> Self {
        Self { caller, connection }
    }
}

/// Dial `id` and establish one vox connection, returning the raw caller
/// and the handle that keeps it alive. The bearer, when given, becomes
/// a global middleware on the caller — every typed client built from a
/// clone presents it on every call.
pub async fn dial(
    id: &str,
    bearer: Option<&str>,
) -> Result<(vox_core::Caller, Option<vox_core::ConnectionHandle>), String> {
    let endpoint = device_endpoint().await?;
    let remote: iroh::EndpointId = id.parse().map_err(|e| format!("endpoint id `{id}`: {e}"))?;
    let link = iroh_link::connect(&endpoint, remote)
        .await
        .map_err(|e| format!("iroh dial {id}: {e}"))?;
    let lane: RawLane = vox_core::initiator_on(link)
        .establish()
        .await
        .map_err(|e| format!("vox establish over iroh: {e:?}"))?;
    let caller = match bearer {
        Some(token) => lane
            .caller
            .with_global_middleware(BearerMiddleware(token.to_owned())),
        None => lane.caller,
    };
    Ok((caller, lane.connection))
}
