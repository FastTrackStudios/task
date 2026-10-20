//! The device's own node, installed into a native app.
//!
//! `task_server::local_node::LocalNode` is the backend; `task_ui_core::node`
//! is the seam the UI reaches it through. This crate is the glue an app's
//! `main` calls once — [`install`] — and it exists as its own crate so
//! neither of those has to know about the other: the server stays free
//! of UI, the UI crates stay free of the server (and so keep compiling
//! for the browser, which holds no node).
//!
//! # Its own runtime
//!
//! The node runs the server's background work — notifiers, the device
//! pull, each org's iroh accept loop, the in-process acceptors — on a
//! dedicated multi-thread tokio runtime, started on first use. Sharing
//! the UI's runtime would let a long sweep or a busy acceptor delay a
//! frame, and on desktop the UI's runtime is dioxus's to own. Futures
//! cross between the two through [`tokio::task::JoinHandle`], which any
//! executor can poll, and the callers handed back drive their connection
//! from the node's runtime, so the UI only ever sends on a channel.
//!
//! # Lazy
//!
//! Installing costs nothing: the node boots on the first call that needs
//! it (the first org discovery, usually). A remote-only session never
//! opens the device's data root at all.

use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use task_server::local_node::{JoinSpec, LocalNode, NodeOptions};
use task_ui_core::node::{
    AccountReport, AccountSyncRequest, JoinRequest, NodeFuture, NodeHost, NodeOwner, PeerSummary,
};

pub use task_server::local_node::NodeOptions as Options;

/// `<app data dir>/node` — beside the registry and token store the app
/// already keeps (`ui::server_registry::data_dir`, mirrored here).
#[must_use]
pub fn default_data_root() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return Some(xdg.join("task").join("node"));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let base = if cfg!(target_vendor = "apple") {
        home.join("Library")
            .join("Application Support")
            .join("task")
    } else {
        home.join(".local").join("share").join("task")
    };
    Some(base.join("node"))
}

/// The options an app boots its node with: `TASK_NODE_ROOT` overrides
/// the data root (a second desk, a demo run), `TASK_NODE_EXAMPLE=1`
/// plants the example studio on first boot (debug builds; on by default
/// in them), and the owner is named after the OS user.
#[must_use]
pub fn options_from_env() -> Option<Options> {
    let data_root = std::env::var_os("TASK_NODE_ROOT")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .or_else(default_data_root)?;
    let plant_example = match std::env::var("TASK_NODE_EXAMPLE") {
        Ok(v) => v != "0" && !v.eq_ignore_ascii_case("false"),
        Err(_) => cfg!(debug_assertions),
    };
    let owner_name = std::env::var("USER")
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| "Me".to_owned());
    Some(NodeOptions {
        owner_name,
        plant_example,
        ..NodeOptions::at(data_root)
    })
}

/// Install the node behind the UI's seam. Call once from `main`, before
/// launch. Nothing boots until something asks.
pub fn install(opts: Options) {
    tracing::info!(root = %opts.data_root.display(), "node: installed (boots on first use)");
    task_ui_core::node::install(Arc::new(Host::new(opts)));
}

/// Drive `fut` to completion from a plain thread — a platform callback
/// (the phone's WatchConnectivity delegate) that is on no runtime. Never
/// call it from inside an async task.
pub fn block_on<T>(fut: impl std::future::Future<Output = T>) -> T {
    runtime().block_on(fut)
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("task-node")
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("node runtime")
    })
}

/// Run `fut` on the node's runtime and await it from anywhere.
async fn on_node<T, F>(fut: F) -> Result<T, String>
where
    T: Send + 'static,
    F: std::future::Future<Output = Result<T, String>> + Send + 'static,
{
    runtime()
        .spawn(fut)
        .await
        .map_err(|e| format!("node task: {e}"))?
}

/// The installed host: options until first use, a node after.
pub struct Host {
    opts: Options,
    node: Arc<tokio::sync::OnceCell<Arc<LocalNode>>>,
    server: Arc<tokio::sync::OnceCell<vox_core::Caller>>,
}

impl Host {
    #[must_use]
    pub fn new(opts: Options) -> Self {
        Self {
            opts,
            node: Arc::new(tokio::sync::OnceCell::new()),
            server: Arc::new(tokio::sync::OnceCell::new()),
        }
    }

    /// The node, booting it on first call. Runs on the node runtime.
    fn node(
        &self,
    ) -> impl std::future::Future<Output = Result<Arc<LocalNode>, String>> + Send + 'static {
        let cell = Arc::clone(&self.node);
        let opts = self.opts.clone();
        async move {
            cell.get_or_try_init(|| async move {
                let started = std::time::Instant::now();
                let node = LocalNode::open(opts)
                    .await
                    .map_err(|e| format!("node boot: {e:#}"))?;
                tracing::info!(
                    orgs = ?node.org_slugs(),
                    ms = started.elapsed().as_millis() as u64,
                    "node: up"
                );
                Ok::<_, String>(Arc::new(node))
            })
            .await
            .cloned()
        }
    }
}

fn org_on_disk(root: &Path, slug: &str) -> bool {
    !slug.is_empty() && root.join("orgs").join(slug).join("org.toml").is_file()
}

impl NodeHost for Host {
    fn caller(&self, slug: String, bearer: Option<String>) -> NodeFuture<vox_core::Caller> {
        let node = self.node();
        Box::pin(on_node(async move {
            node.await?
                .caller(&slug, bearer)
                .await
                .map_err(|e| format!("node lane for `{slug}`: {e:#}"))
        }))
    }

    fn server_caller(&self) -> NodeFuture<vox_core::Caller> {
        let node = self.node();
        let cell = Arc::clone(&self.server);
        Box::pin(on_node(async move {
            // One connection for the process, like a host's one socket:
            // these services take the token as an argument, so nothing
            // about the connection is per-identity.
            cell.get_or_try_init(|| async move {
                node.await?
                    .server_caller()
                    .await
                    .map_err(|e| format!("node /server lane: {e:#}"))
            })
            .await
            .cloned()
        }))
    }

    fn discovery(&self, bearer: Option<String>) -> NodeFuture<String> {
        let node = self.node();
        Box::pin(on_node(async move {
            let doc = node.await?.discovery(bearer).await;
            serde_json::to_string(&doc).map_err(|e| format!("encode discovery: {e}"))
        }))
    }

    fn owner(&self) -> NodeFuture<NodeOwner> {
        let node = self.node();
        Box::pin(on_node(async move {
            let o = node.await?.owner().clone();
            Ok(NodeOwner {
                org: o.org,
                email: o.email,
                name: o.name,
                password: o.password,
            })
        }))
    }

    fn join(&self, request: JoinRequest) -> NodeFuture<Option<String>> {
        let node = self.node();
        Box::pin(on_node(async move {
            let joined = node
                .await?
                .join(JoinSpec {
                    slug: request.slug,
                    id: request.id,
                    display_name: request.display_name,
                    peer: request.peer,
                    role: request.role,
                })
                .await
                .map_err(|e| format!("join: {e:#}"))?;
            Ok(joined.endpoint_id)
        }))
    }

    fn account_sync(&self, request: AccountSyncRequest) -> NodeFuture<AccountReport> {
        use task_server::account_devices::{ThisDevice, device_id, install_id, sync_account};
        use vox_core::FromVoxLane as _;
        let node = self.node();
        let data_root = self.opts.data_root.clone();
        Box::pin(on_node(async move {
            let node = node.await?;
            let install = install_id(&data_root).map_err(|e| format!("install id: {e:#}"))?;
            let me = ThisDevice {
                device_id: device_id(install, &request.account),
                name: request.device_name,
                platform: request.platform,
            };
            let issuer = architect_auth::DeviceServiceClient::from_vox_lane(request.issuer, None);
            let pass = sync_account(&node, &issuer, &request.token, &me)
                .await
                .map_err(|e| format!("account sync: {e:#}"))?;
            Ok(AccountReport {
                slug: pass.slug,
                joined: pass.joined,
                others: pass
                    .others
                    .into_iter()
                    .map(|d| PeerSummary {
                        name: d.name,
                        platform: d.platform,
                        reachable: d.endpoint_id.is_some(),
                        last_seen: d.last_seen_at.to_rfc3339(),
                    })
                    .collect(),
            })
        }))
    }

    fn watch(
        &self,
        method: String,
        path: String,
        bearer: Option<String>,
        body: Vec<u8>,
    ) -> NodeFuture<(u16, Vec<u8>)> {
        let node = self.node();
        Box::pin(on_node(async move {
            Ok(node.await?.watch(&method, &path, bearer, body).await)
        }))
    }

    fn holds(&self, slug: &str) -> bool {
        // From disk, not from the node: the fallback asks this before the
        // node has necessarily booted, and booting it to find out would
        // turn every unreachable host into a data-root open.
        org_on_disk(&self.opts.data_root, slug)
    }

    fn endpoint_id(&self, slug: &str) -> Option<String> {
        if let Some(node) = self.node.get() {
            return node.endpoint_id(slug);
        }
        // Bound on an earlier run: `iroh_host` writes it beside the key.
        std::fs::read_to_string(
            self.opts
                .data_root
                .join("orgs")
                .join(slug)
                .join("iroh-endpoint-id"),
        )
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
    }

    fn data_root(&self) -> PathBuf {
        self.opts.data_root.clone()
    }
}
