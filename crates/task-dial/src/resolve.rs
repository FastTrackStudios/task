//! **Which node an app dials.** Local node first, then the account's
//! other nodes, and a public host only as the last resort.
//!
//! ADR 0006 §3: an app talks to *a node*, not to "the server". On a
//! machine with a Task node (the desktop app's own, `offline.local` /
//! `offline.cli`) the app dials that node, so it works offline exactly as
//! far as the node does and its edits ride the same peer mesh as
//! everything else. With no local node it tries another of the account's
//! nodes (the device address book the issuer keeps, `offline.account`),
//! and only then a public host — one the *caller* names. This crate names
//! none: there is no default host here, on purpose (guardrail 7). An app
//! that wants `wss://task.fasttrackstudio.app` as its discovery default
//! passes it in [`NodePrefs::public_host`].
//!
//! ```ignore
//! let node = task_dial::resolve_node(&account, &NodePrefs {
//!     public_host: Some("wss://task.example.com".into()),
//!     ..NodePrefs::default()
//! }).await?;
//! let tasks: TaskServiceClient = task_dial::establish_at(&node.url, Some(&token)).await?;
//! ```
//!
//! # The three stages
//!
//! 1. **The local node.** A node serving on loopback writes its `ws://`
//!    base to `<node root>/local-endpoint` ([`LOCAL_ENDPOINT_FILE`]). The
//!    resolver reads it, then asks that base for its org discovery
//!    document (`/.well-known/task-server.json`, public) — which both
//!    proves something is answering (a killed app leaves the file behind,
//!    naming a dead port) and says *which slug* the node holds the
//!    account's org under: a device that joined the account's org when
//!    its slug was taken there holds it under one of its own, so the
//!    account's slug is matched by org id first.
//! 2. **The account's other nodes.** Every device in the address book
//!    that has an endpoint id for the org, except this one, most recently
//!    seen first. Peers are dialled by endpoint id over iroh, so the URL
//!    is `iroh://<endpoint id>` ([`peer_url`]) — dial it with an iroh
//!    transport (`task-ui-core`'s `iroh_transport`). [`establish_at`]
//!    refuses it with a message saying so, rather than mis-dialling it as
//!    a WebSocket.
//! 3. **A public host**, if the caller named one: `<host>/org/<slug>/vox`.
//!
//! [`candidates`] returns every stage's answer in order, for a caller
//! that wants to fall through on a failed dial; [`resolve_node`] returns
//! the first.
//!
//! [`establish_at`]: crate::establish_at

// t[impl offline.app-node]

use std::fmt;
use std::path::PathBuf;

/// Where a node serving on loopback says its `ws://` base, in its data
/// root (owner-only). `task_server::local_node` writes it under this name.
pub const LOCAL_ENDPOINT_FILE: &str = "local-endpoint";

/// The scheme of a peer node's address: `iroh://<endpoint id>`.
pub const IROH_SCHEME: &str = "iroh://";

/// The account whose org an app wants, and its device address book.
///
/// Plain strings rather than `architect_auth`'s wire types, so a browser
/// app can fill it without pulling the issuer's crates in: copy
/// `DeviceDirectory::account_org` and `::devices` across.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Account {
    /// The org's slug as the account knows it (`AccountOrg::slug`, or the
    /// slug `ensure_personal_org` returned).
    pub org_slug: String,
    /// The org's id (`AccountOrg::id`). When set, a node is matched by it
    /// rather than by slug — the slug is local to each node.
    pub org_id: Option<String>,
    /// This device's id in the address book, so it is not offered as its
    /// own peer.
    pub this_device: Option<String>,
    /// Every device signed in as the account (`DeviceDirectory::devices`).
    pub devices: Vec<AccountDevice>,
}

/// One device in the account's address book (`PeerDevice`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountDevice {
    pub device_id: String,
    /// Its endpoint id for the account's org; `None` for a device that
    /// holds no node of it.
    pub endpoint_id: Option<String>,
    /// When the issuer last heard from it, as Unix seconds — the order
    /// peers are tried in, most recent first.
    pub last_seen_unix: Option<i64>,
}

/// The caller's say over the resolution. The default is "local node at
/// the desktop's data root, then peers, and no public host".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NodePrefs {
    /// The local node's data root. `None` is the desktop app's
    /// ([`default_node_root`]). Native only: a browser cannot read files.
    pub node_root: Option<PathBuf>,
    /// The local node's `ws://` base, given outright instead of read from
    /// the data root — what a browser app served by the desktop is told.
    pub local_endpoint: Option<String>,
    /// Skip stage 1.
    pub skip_local: bool,
    /// Skip stage 2.
    pub skip_peers: bool,
    /// The public host to fall back to, as a base (`wss://host` or
    /// `https://host`; a trailing `/` is fine). `None` means there is no
    /// fallback — the crate has no default.
    pub public_host: Option<String>,
}

/// Which stage answered.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Via {
    /// The node on this machine.
    Local,
    /// Another of the account's nodes, by its device id.
    Peer { device_id: String },
    /// The public host the caller named.
    Host,
}

/// A node to dial: its vox URL and which stage found it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Resolved {
    /// `ws://127.0.0.1:<port>/org/<slug>/vox`, `iroh://<endpoint id>`, or
    /// `<host>/org/<slug>/vox`.
    pub url: String,
    pub via: Via,
}

/// No stage found a node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoNode {
    pub org: String,
}

impl fmt::Display for NoNode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "no node for org `{}`: no local node is serving it, no other device of the \
             account has an endpoint for it, and no public host was named",
            self.org
        )
    }
}

impl std::error::Error for NoNode {}

/// The node to dial for `account`'s org: the first of [`candidates`].
///
/// # Errors
///
/// [`NoNode`] when no stage has an answer.
pub async fn resolve_node(account: &Account, prefs: &NodePrefs) -> Result<Resolved, NoNode> {
    candidates(account, prefs)
        .await
        .into_iter()
        .next()
        .ok_or_else(|| NoNode {
            org: account.org_slug.clone(),
        })
}

/// Every stage's answer, in the order to try them: the local node (only
/// when it answers and holds the org), the account's other nodes, then
/// the public host.
pub async fn candidates(account: &Account, prefs: &NodePrefs) -> Vec<Resolved> {
    let mut out = Vec::new();
    if !prefs.skip_local
        && let Some(url) = local_stage(account, prefs).await
    {
        out.push(Resolved {
            url,
            via: Via::Local,
        });
    }
    if !prefs.skip_peers {
        out.extend(peer_stage(account));
    }
    if let Some(url) = host_stage(account, prefs) {
        out.push(Resolved {
            url,
            via: Via::Host,
        });
    }
    out
}

/// `iroh://<endpoint id>`.
#[must_use]
pub fn peer_url(endpoint_id: &str) -> String {
    format!("{IROH_SCHEME}{endpoint_id}")
}

/// The endpoint id of a [`peer_url`].
#[must_use]
pub fn endpoint_id_of(url: &str) -> Option<&str> {
    url.strip_prefix(IROH_SCHEME)
        .map(|id| id.trim_end_matches('/'))
        .filter(|id| !id.is_empty())
}

// ── stage 1: the local node ─────────────────────────────────────────

/// The desktop app's node root: `$TASK_NODE_ROOT`, else
/// `<app data>/task/node` — `$XDG_DATA_HOME` when set, else
/// `~/Library/Application Support` on Apple and `~/.local/share`
/// elsewhere. The one place this path is spelled; `task-client` reads it
/// from here.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn default_node_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("TASK_NODE_ROOT").filter(|r| !r.is_empty()) {
        return Some(PathBuf::from(root));
    }
    Some(default_app_dir()?.join("node"))
}

/// `<app data>/task`, as the desktop app resolves it.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn default_app_dir() -> Option<PathBuf> {
    if let Some(xdg) = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
    {
        return Some(xdg.join("task"));
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    let base = if cfg!(target_vendor = "apple") {
        home.join("Library").join("Application Support")
    } else {
        home.join(".local").join("share")
    };
    Some(base.join("task"))
}

/// The `ws://` base a node under `root` says it serves on, if the file is
/// there and names loopback. Says nothing about whether it still answers.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn read_local_endpoint(root: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(root.join(LOCAL_ENDPOINT_FILE))
        .ok()
        .map(|s| s.trim().trim_end_matches('/').to_owned())
        .filter(|s| is_loopback_ws(s))
}

/// Whether `base` is a `ws://` base on loopback — the only thing a
/// `local-endpoint` file may name. Anything else is ignored rather than
/// dialled: the file is the local node's, and a token presented to it
/// must not leave the machine.
#[must_use]
pub fn is_loopback_ws(base: &str) -> bool {
    base.starts_with("ws://127.0.0.1:") || base.starts_with("ws://[::1]:")
}

async fn local_stage(account: &Account, prefs: &NodePrefs) -> Option<String> {
    let base = match prefs.local_endpoint.as_deref() {
        Some(given) => Some(given.trim().trim_end_matches('/').to_owned()),
        None => file_endpoint(prefs),
    }
    .filter(|b| is_loopback_ws(b))?;
    let slug = local_slug(&base, account).await?;
    Some(format!("{base}/org/{slug}/vox"))
}

#[cfg(not(target_arch = "wasm32"))]
fn file_endpoint(prefs: &NodePrefs) -> Option<String> {
    let root = prefs.node_root.clone().or_else(default_node_root)?;
    read_local_endpoint(&root)
}

#[cfg(target_arch = "wasm32")]
fn file_endpoint(_prefs: &NodePrefs) -> Option<String> {
    None
}

/// The slug the node at `base` holds the account's org under, asked of
/// its discovery document; `None` when it does not answer or does not
/// hold the org.
#[cfg(not(target_arch = "wasm32"))]
async fn local_slug(base: &str, account: &Account) -> Option<String> {
    let doc = fetch_discovery(base).await?;
    org_slug_in(&doc, account)
}

/// A browser cannot read the discovery document cross-origin without the
/// node's CORS say-so, so a given endpoint is trusted to hold the org
/// under the account's slug.
#[cfg(target_arch = "wasm32")]
async fn local_slug(_base: &str, account: &Account) -> Option<String> {
    Some(account.org_slug.clone()).filter(|s| !s.is_empty())
}

/// The account's org in a discovery document (`{"orgs": [{"slug", "id"}]}`):
/// by id when the account has one, else by slug.
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
fn org_slug_in(doc: &serde_json::Value, account: &Account) -> Option<String> {
    let orgs = doc.get("orgs")?.as_array()?;
    let field =
        |o: &serde_json::Value, k: &str| o.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    let found = match account.org_id.as_deref() {
        Some(id) => orgs
            .iter()
            .find(|o| field(o, "id").is_some_and(|v| v.eq_ignore_ascii_case(id))),
        None => orgs
            .iter()
            .find(|o| field(o, "slug").as_deref() == Some(account.org_slug.as_str())),
    }?;
    field(found, "slug").filter(|s| !s.is_empty())
}

/// GET `/.well-known/task-server.json` from a loopback `ws://` base, with
/// a short deadline: a stale `local-endpoint` must cost a moment, not a
/// hang. HTTP/1.0 so the reply is a plain body read to close.
#[cfg(not(target_arch = "wasm32"))]
async fn fetch_discovery(base: &str) -> Option<serde_json::Value> {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    const DEADLINE: std::time::Duration = std::time::Duration::from_secs(2);
    const MAX: u64 = 4 << 20;

    let authority = base.strip_prefix("ws://")?.split('/').next()?.to_owned();
    let fetch = async {
        let mut stream = tokio::net::TcpStream::connect(authority.as_str())
            .await
            .ok()?;
        let request = format!(
            "GET /.well-known/task-server.json HTTP/1.0\r\nHost: {authority}\r\n\
             Accept: application/json\r\nConnection: close\r\n\r\n"
        );
        stream.write_all(request.as_bytes()).await.ok()?;
        let mut raw = Vec::new();
        stream.take(MAX).read_to_end(&mut raw).await.ok()?;
        Some(raw)
    };
    let raw = tokio::time::timeout(DEADLINE, fetch).await.ok()??;
    let split = raw.windows(4).position(|w| w == b"\r\n\r\n")?;
    let head = std::str::from_utf8(&raw[..split]).ok()?;
    let status = head.lines().next()?.split_whitespace().nth(1)?;
    if status != "200" {
        return None;
    }
    serde_json::from_slice(&raw[split + 4..]).ok()
}

// ── stage 2: the account's other nodes ──────────────────────────────

fn peer_stage(account: &Account) -> Vec<Resolved> {
    let mut peers: Vec<&AccountDevice> = account
        .devices
        .iter()
        .filter(|d| account.this_device.as_deref() != Some(d.device_id.as_str()))
        .filter(|d| {
            d.endpoint_id
                .as_deref()
                .is_some_and(|e| !e.trim().is_empty())
        })
        .collect();
    // Most recently seen first; never-seen last. Stable, so the address
    // book's own order breaks ties.
    peers.sort_by_key(|d| std::cmp::Reverse(d.last_seen_unix.unwrap_or(i64::MIN)));
    let mut seen = std::collections::HashSet::new();
    peers
        .into_iter()
        .filter_map(|d| {
            let id = d.endpoint_id.as_deref()?.trim();
            seen.insert(id.to_owned()).then(|| Resolved {
                url: peer_url(id),
                via: Via::Peer {
                    device_id: d.device_id.clone(),
                },
            })
        })
        .collect()
}

// ── stage 3: the public host ────────────────────────────────────────

fn host_stage(account: &Account, prefs: &NodePrefs) -> Option<String> {
    let base = normalize_host(prefs.public_host.as_deref()?)?;
    if account.org_slug.is_empty() {
        return None;
    }
    Some(format!("{base}/org/{}/vox", account.org_slug))
}

/// A host base as a vox base: `https→wss`, `http→ws`, a bare host gets
/// `wss://`, trailing `/` dropped. Empty is no host.
fn normalize_host(raw: &str) -> Option<String> {
    let raw = raw.trim().trim_end_matches('/');
    if raw.is_empty() {
        return None;
    }
    let base = if let Some(rest) = raw.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = raw.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if raw.starts_with("wss://") || raw.starts_with("ws://") {
        raw.to_owned()
    } else {
        format!("wss://{raw}")
    };
    Some(base)
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    const ORG_ID: &str = "0b5e2f1c-7a4d-4e3b-9c1a-2d6f8e0a4b71";

    fn account() -> Account {
        Account {
            org_slug: "sam".into(),
            org_id: Some(ORG_ID.into()),
            this_device: Some("desk".into()),
            devices: vec![
                AccountDevice {
                    device_id: "desk".into(),
                    endpoint_id: Some("e-desk".into()),
                    last_seen_unix: Some(300),
                },
                AccountDevice {
                    device_id: "phone".into(),
                    endpoint_id: Some("e-phone".into()),
                    last_seen_unix: Some(100),
                },
                AccountDevice {
                    device_id: "laptop".into(),
                    endpoint_id: Some("e-laptop".into()),
                    last_seen_unix: Some(200),
                },
                AccountDevice {
                    device_id: "tablet".into(),
                    endpoint_id: None,
                    last_seen_unix: Some(400),
                },
            ],
        }
    }

    /// A node answering discovery on loopback with `doc`, for as many
    /// requests as come. Returns its `ws://` base.
    async fn fake_node(doc: serde_json::Value) -> String {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let body = doc.to_string();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = listener.accept().await {
                let body = body.clone();
                tokio::spawn(async move {
                    let mut buf = [0u8; 2048];
                    let n = s.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
                    let reply = if req.starts_with("GET /.well-known/task-server.json ") {
                        format!(
                            "HTTP/1.0 200 OK\r\ncontent-type: application/json\r\n\
                             content-length: {}\r\n\r\n{body}",
                            body.len()
                        )
                    } else {
                        "HTTP/1.0 404 Not Found\r\ncontent-length: 0\r\n\r\n".to_owned()
                    };
                    let _ = s.write_all(reply.as_bytes()).await;
                });
            }
        });
        format!("ws://{addr}")
    }

    fn root_saying(endpoint: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join(LOCAL_ENDPOINT_FILE),
            format!("{endpoint}\n"),
        )
        .unwrap();
        dir
    }

    fn prefs_at(root: &std::path::Path) -> NodePrefs {
        NodePrefs {
            node_root: Some(root.to_owned()),
            ..NodePrefs::default()
        }
    }

    /// A port nothing answers on: bound, then dropped.
    fn dead_base() -> String {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = l.local_addr().unwrap();
        drop(l);
        format!("ws://{addr}")
    }

    // ── stage 1 ──

    // t[verify offline.app-node]
    #[tokio::test]
    async fn the_local_node_comes_first_when_it_answers_and_holds_the_org() {
        let base = fake_node(serde_json::json!({
            "orgs": [{ "slug": "sam", "id": ORG_ID }]
        }))
        .await;
        let root = root_saying(&base);
        let prefs = NodePrefs {
            public_host: Some("wss://host.example".into()),
            ..prefs_at(root.path())
        };
        let got = resolve_node(&account(), &prefs).await.unwrap();
        assert_eq!(got.via, Via::Local);
        assert_eq!(got.url, format!("{base}/org/sam/vox"));
    }

    /// A device that joined the account's org while `sam` was taken holds
    /// it under its own slug; the id finds it.
    #[tokio::test]
    async fn the_local_node_is_matched_by_org_id_not_slug() {
        let base = fake_node(serde_json::json!({
            "orgs": [
                { "slug": "sam", "id": "11111111-1111-4111-8111-111111111111" },
                { "slug": "sam-2", "id": ORG_ID.to_uppercase() }
            ]
        }))
        .await;
        let root = root_saying(&base);
        let got = resolve_node(&account(), &prefs_at(root.path()))
            .await
            .unwrap();
        assert_eq!(got.url, format!("{base}/org/sam-2/vox"));
    }

    #[tokio::test]
    async fn without_an_org_id_the_slug_matches() {
        let base = fake_node(serde_json::json!({ "orgs": [{ "slug": "sam", "id": "x" }] })).await;
        let root = root_saying(&base);
        let mut account = account();
        account.org_id = None;
        let got = resolve_node(&account, &prefs_at(root.path()))
            .await
            .unwrap();
        assert_eq!(got.url, format!("{base}/org/sam/vox"));
    }

    /// A killed app leaves `local-endpoint` behind naming a dead port: the
    /// stage is skipped, not dialled.
    #[tokio::test]
    async fn a_stale_local_endpoint_falls_through_to_a_peer() {
        let root = root_saying(&dead_base());
        let got = resolve_node(&account(), &prefs_at(root.path()))
            .await
            .unwrap();
        assert_eq!(
            got.via,
            Via::Peer {
                device_id: "laptop".into()
            }
        );
    }

    #[tokio::test]
    async fn a_local_node_without_the_org_is_skipped() {
        let base = fake_node(serde_json::json!({
            "orgs": [{ "slug": "someone-else", "id": "22222222-2222-4222-8222-222222222222" }]
        }))
        .await;
        let root = root_saying(&base);
        let got = resolve_node(&account(), &prefs_at(root.path()))
            .await
            .unwrap();
        assert!(matches!(got.via, Via::Peer { .. }), "{got:?}");
    }

    /// The file is the local node's; a token must never follow it off the
    /// machine.
    #[tokio::test]
    async fn a_local_endpoint_off_loopback_is_ignored() {
        let root = root_saying("wss://elsewhere.example:443");
        let prefs = NodePrefs {
            skip_peers: true,
            ..prefs_at(root.path())
        };
        assert!(resolve_node(&account(), &prefs).await.is_err());
        assert!(!is_loopback_ws("ws://localhost.evil:1"));
        assert!(is_loopback_ws("ws://[::1]:4000"));
    }

    #[tokio::test]
    async fn a_given_local_endpoint_needs_no_data_root() {
        let base =
            fake_node(serde_json::json!({ "orgs": [{ "slug": "sam", "id": ORG_ID }] })).await;
        let empty = tempfile::tempdir().unwrap();
        let prefs = NodePrefs {
            local_endpoint: Some(format!("{base}/")),
            ..prefs_at(empty.path())
        };
        let got = resolve_node(&account(), &prefs).await.unwrap();
        assert_eq!(got.url, format!("{base}/org/sam/vox"));
    }

    // ── stage 2 ──

    /// Not this device, not one without an endpoint, most recent first.
    #[tokio::test]
    async fn peers_are_the_other_devices_with_an_endpoint_most_recent_first() {
        let empty = tempfile::tempdir().unwrap();
        let all = candidates(&account(), &prefs_at(empty.path())).await;
        assert_eq!(
            all,
            vec![
                Resolved {
                    url: "iroh://e-laptop".into(),
                    via: Via::Peer {
                        device_id: "laptop".into()
                    }
                },
                Resolved {
                    url: "iroh://e-phone".into(),
                    via: Via::Peer {
                        device_id: "phone".into()
                    }
                },
            ]
        );
        assert_eq!(endpoint_id_of(&all[0].url), Some("e-laptop"));
        assert_eq!(endpoint_id_of("wss://x"), None);
    }

    // ── stage 3 ──

    #[tokio::test]
    async fn the_public_host_is_the_last_resort() {
        let empty = tempfile::tempdir().unwrap();
        let prefs = NodePrefs {
            public_host: Some("https://host.example/".into()),
            ..prefs_at(empty.path())
        };
        let all = candidates(&account(), &prefs).await;
        let last = all.last().unwrap();
        assert_eq!(last.via, Via::Host);
        assert_eq!(last.url, "wss://host.example/org/sam/vox");

        let only_host = NodePrefs {
            skip_peers: true,
            ..prefs.clone()
        };
        let got = resolve_node(&account(), &only_host).await.unwrap();
        assert_eq!(got.via, Via::Host);
    }

    /// Guardrail 7: nothing configured, nothing local, no peers — an
    /// error, not a dial to a host the crate made up.
    #[tokio::test]
    async fn there_is_no_default_host() {
        let empty = tempfile::tempdir().unwrap();
        let mut account = account();
        account.devices.clear();
        let err = resolve_node(&account, &prefs_at(empty.path()))
            .await
            .unwrap_err();
        assert_eq!(err, NoNode { org: "sam".into() });
        assert_eq!(normalize_host("  "), None);
        assert_eq!(
            normalize_host("host.example").as_deref(),
            Some("wss://host.example")
        );
        assert_eq!(
            normalize_host("http://127.0.0.1:9").as_deref(),
            Some("ws://127.0.0.1:9")
        );
    }
}
