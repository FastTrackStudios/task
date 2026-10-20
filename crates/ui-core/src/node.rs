//! The device's own node, as the UI sees it — the seam that makes every
//! page work with no server (`offline.local`).
//!
//! A native app can host the backend itself (`task_server::local_node`,
//! installed by the app's `main` through `task-node-host`). This module
//! is how the rest of the UI reaches it *without* depending on the
//! server crate: the app installs a [`NodeHost`], and the connection
//! layer ([`crate::vox_clients::caller_for`] and friends) resolves an
//! org to an in-process caller instead of dialling, whenever the active
//! address is [`LOCAL_VOX_URL`]. Every typed client is a view over that
//! caller, so no page changes.
//!
//! # Where an org is served from
//!
//! [`route`] is the whole rule, kept pure so it is tested as a table:
//!
//! - the active address is this device ([`LOCAL_VOX_URL`]) and a node is
//!   installed → **local**;
//! - otherwise → **remote** (dial the host);
//! - and a remote dial that fails as *unreachable* for an org the node
//!   also holds (a joined org, `offline.peers`) → **local** — the device
//!   is one of that org's nodes, so serving it from here is not a
//!   substitute world but the same org. An org the node does not hold
//!   keeps the old answer (the error, and the device copies of
//!   `offline.boot`); quietly showing a *different* org's data would be
//!   the wrong kind of helpful.
//!
//! The browser holds no node: on wasm nothing can be installed and every
//! route is remote.

/// The address that means "the node on this device". Selected like any
/// server in the registry; `vox_url()` defaults to it on a native build
/// that has a node installed and no server configured.
pub const LOCAL_VOX_URL: &str = "local://this-device";

/// Whether `url` names this device's node (`local://…`, or the bare
/// word `local` an env var might carry).
#[must_use]
pub fn is_local(url: &str) -> bool {
    let t = url.trim();
    t.starts_with("local://") || t == "local"
}

/// Where one org's calls go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// The in-process node.
    Local,
    /// Dial the active address.
    Remote,
}

/// The routing rule for the active address (see the module docs).
#[must_use]
pub fn route(active_url: &str, node_installed: bool) -> Route {
    if node_installed && is_local(active_url) {
        Route::Local
    } else {
        Route::Remote
    }
}

/// After a failed remote dial: serve from the node instead? Only when
/// the failure is "unreachable" (not a refusal — a host that answered
/// "no" must stay a no) and the node holds this very org.
#[must_use]
pub fn fall_back_to_node(unreachable: bool, node_holds_org: bool) -> bool {
    unreachable && node_holds_org
}

/// Whether a dial error means the host could not be reached at all (as
/// opposed to answering). The dial-failure half of architect's
/// `classify` markers — that function lives behind architect's atom
/// features, which this tiny crate does not carry.
#[must_use]
pub fn dial_unreachable(message: &str) -> bool {
    const MARKERS: &[&str] = &[
        "ws connect",
        "ws request",
        "WebSocket open failed",
        "establish `",
        "dial cancelled",
        "iroh dial",
        "vox establish over iroh",
        "Connection refused",
        "connection refused",
        "TimedOut",
        "timed out",
        "ConnectionClosed",
        "dns error",
    ];
    MARKERS.iter().any(|m| message.contains(m))
}

/// The account the node signs this device's person in as. Cross-target
/// so call sites need no `cfg`; only a native build ever has one.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeOwner {
    /// The org whose auth store holds the account — sign in THERE.
    pub org: String,
    pub email: String,
    pub name: String,
    pub password: String,
}

/// The device's person, when the app is pointed at its own node. `None`
/// on the browser, with no node installed, or pointed at a host.
pub async fn local_owner() -> Option<NodeOwner> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        if !local_active() {
            return None;
        }
        match host()?.owner().await {
            Ok(owner) => Some(owner),
            Err(e) => {
                tracing::warn!(error = %e, "node: no owner to sign in as");
                None
            }
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        None
    }
}

/// One of the account's other devices.
///
/// Plain data, so it exists on every target: the web build (no node)
/// still names [`AccountReport`] in `ui::account_devices`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerSummary {
    pub name: String,
    pub platform: String,
    /// Whether it has said where it is (holds the shared org).
    pub reachable: bool,
    /// RFC 3339.
    pub last_seen: String,
}

/// What a pass of `NodeHost::account_sync` found (native builds; the
/// web build only ever holds `None` of it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountReport {
    /// The shared org's slug on this device, once held.
    pub slug: Option<String>,
    /// Whether this pass joined it.
    pub joined: bool,
    pub others: Vec<PeerSummary>,
}

#[cfg(not(target_arch = "wasm32"))]
pub use native::*;

#[cfg(not(target_arch = "wasm32"))]
mod native {
    use std::sync::{Arc, OnceLock};

    /// A boxed future the host's methods return — `Send`, because the
    /// desktop polls from whichever runtime thread picks it up.
    pub type NodeFuture<T> = futures_util::future::BoxFuture<'static, Result<T, String>>;

    use super::{AccountReport, NodeOwner};

    /// Taking a node for an org a host or another device holds.
    #[derive(Clone, Debug)]
    pub struct JoinRequest {
        pub slug: String,
        pub id: uuid::Uuid,
        pub display_name: String,
        /// The peer's endpoint id for the org (its `coordinator()`).
        pub peer: Option<String>,
        pub role: Option<String>,
    }

    /// What the app's `main` installs: the node, behind the calls the
    /// UI needs from it. Implemented by `task-node-host` over
    /// `task_server::local_node::LocalNode`.
    pub trait NodeHost: Send + Sync + 'static {
        /// An in-process connection to `slug`'s org router, as the
        /// holder of `bearer`.
        fn caller(&self, slug: String, bearer: Option<String>) -> NodeFuture<vox_core::Caller>;
        /// The node's `/server/vox` surface, in-process.
        fn server_caller(&self) -> NodeFuture<vox_core::Caller>;
        /// The node's discovery document (the JSON body a host serves at
        /// `/.well-known/task-server.json`) for `bearer`.
        fn discovery(&self, bearer: Option<String>) -> NodeFuture<String>;
        /// The device's person.
        fn owner(&self) -> NodeFuture<NodeOwner>;
        /// Take a node for an org held elsewhere; returns this node's
        /// endpoint id for it (what the peer enrolls).
        fn join(&self, request: JoinRequest) -> NodeFuture<Option<String>>;
        /// One watch-API call (`/org/{slug}/watch/v1/…`, path and query),
        /// answered in-process: the phone relays its paired watch's
        /// requests here, since the watch cannot dial this node. Returns
        /// the HTTP status and body.
        fn watch(
            &self,
            method: String,
            path: String,
            bearer: Option<String>,
            body: Vec<u8>,
        ) -> NodeFuture<(u16, Vec<u8>)>;
        /// Whether the node holds `slug` — answered from what the node
        /// has said so far, so it is cheap and never boots the node.
        fn holds(&self, slug: &str) -> bool;
        /// This node's endpoint id for `slug`, once bound.
        fn endpoint_id(&self, slug: &str) -> Option<String>;
        /// Where the node keeps its data, for Settings to show.
        fn data_root(&self) -> std::path::PathBuf;
        /// One pass of the account's device address book on the issuer
        /// (`issuer`, a lane to `fts-auth`): register this install, join
        /// the org the account's devices share if this one does not hold
        /// it, and admit the account's other devices
        /// (`task_server::account_devices`). `account` is the issuer's
        /// subject for the signed-in person.
        fn account_sync(&self, request: AccountSyncRequest) -> NodeFuture<AccountReport>;
    }

    /// What [`NodeHost::account_sync`] needs.
    #[derive(Clone)]
    pub struct AccountSyncRequest {
        pub issuer: vox_core::Caller,
        /// The account's token on the issuer (session or access token).
        pub token: String,
        /// The issuer's subject for the account.
        pub account: String,
        /// What a person calls this device ("Cody's iPhone").
        pub device_name: String,
        /// `ios`, `macos`, `linux`, `windows`.
        pub platform: String,
    }

    static HOST: OnceLock<Arc<dyn NodeHost>> = OnceLock::new();

    /// Install the node. Once per process, from the app's `main`; a
    /// second install is ignored (the first node owns the data root).
    pub fn install(host: Arc<dyn NodeHost>) {
        if HOST.set(host).is_err() {
            tracing::warn!("node: a host is already installed; ignoring the second");
        }
    }

    /// The installed node, if this build hosts one.
    #[must_use]
    pub fn host() -> Option<Arc<dyn NodeHost>> {
        HOST.get().cloned()
    }

    /// Whether a node is installed at all.
    #[must_use]
    pub fn installed() -> bool {
        HOST.get().is_some()
    }

    /// Whether the app is pointed at this device's node right now.
    #[must_use]
    pub fn local_active() -> bool {
        super::route(&crate::vox_session::vox_url(), installed()) == super::Route::Local
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_only_when_selected_and_installed() {
        assert_eq!(route(LOCAL_VOX_URL, true), Route::Local);
        assert_eq!(route("local", true), Route::Local);
        // Selected, but this build hosts no node (the browser): remote,
        // which then fails loud on the meaningless address.
        assert_eq!(route(LOCAL_VOX_URL, false), Route::Remote);
        // A host is a host, whatever the device can do.
        assert_eq!(
            route("wss://task.starcommand.live/vox", true),
            Route::Remote
        );
        assert_eq!(route("ws://127.0.0.1:18080/vox", true), Route::Remote);
        assert_eq!(route("", true), Route::Remote);
    }

    #[test]
    fn falls_back_only_for_a_held_org_that_is_unreachable() {
        assert!(fall_back_to_node(true, true));
        // Not ours: the error (and offline.boot's device copies), never
        // a different org's data.
        assert!(!fall_back_to_node(true, false));
        // Refused, not unreachable: the host's answer stands.
        assert!(!fall_back_to_node(false, true));
    }

    #[test]
    fn a_dead_host_is_unreachable_and_a_refusal_is_not() {
        assert!(dial_unreachable(
            "ws connect `ws://127.0.0.1:18080/org/acme-audio/vox`: IO error: Connection refused (os error 61)"
        ));
        assert!(dial_unreachable("iroh dial abc: timed out"));
        assert!(!dial_unreachable("sign in: invalid credentials"));
        assert!(!dial_unreachable(
            "permission denied: anonymous is not a member"
        ));
    }

    #[test]
    fn local_addresses() {
        assert!(is_local("local://this-device"));
        assert!(is_local(" local "));
        assert!(!is_local("ws://localhost:18080/vox"));
        assert!(!is_local("wss://local.example/vox"));
    }
}
