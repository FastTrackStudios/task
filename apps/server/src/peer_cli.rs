//! `task-server peer`: a server that is one more of a person's devices.
//!
//! A hosted server holds orgs of its own; a *personal* server holds the
//! person's, as a peer of their phone and laptop that is simply always
//! on. So it is a [`LocalNode`] like the apps hold — the same data root
//! layout, the same iroh endpoints, the same account pass
//! ([`crate::account_devices::sync_account`]) — bound to a public address
//! instead of loopback, with no window.
//!
//! ```text
//! task-server peer sign-in    sign this server in as your account (a code to approve on your phone)
//! task-server peer            run: serve, and keep in step with the account's devices
//! task-server peer status     who it is signed in as, and the orgs it holds
//! ```
//!
//! `TASK_DATA_ROOT` is the node root (default `./data`), `TASK_SERVER_BIND`
//! the address (default `0.0.0.0:9090`), `TASK_CENTRAL_AUTH_URL` the
//! issuer (default FastTrackStudio's). The account's session is kept in
//! `<root>/peer-account.json`, owner-only.

// t[impl offline.server-peer]

use std::path::{Path, PathBuf};
use std::time::Duration;

use eyre::{WrapErr as _, eyre};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::account_devices::{ThisDevice, device_id, install_id, sync_account};
use crate::local_node::{LocalNode, NodeOptions};

/// The issuer a personal server signs in at unless told otherwise.
pub const DEFAULT_ISSUER: &str = "https://auth.fasttrackstudio.app";

/// The account pass's cadence: the apps run one every five minutes.
const PASS_EVERY: Duration = Duration::from_secs(300);

const ACCOUNT_FILE: &str = "peer-account.json";

/// The account this server is signed in as.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PeerAccount {
    pub issuer: String,
    pub user_id: String,
    pub email: String,
    pub name: String,
    pub token: String,
}

/// Handle `task-server peer …`. `Ok(false)` when the arguments are not
/// a peer command, so `main` boots the hosted server as before.
pub async fn dispatch() -> eyre::Result<bool> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("peer") {
        return Ok(false);
    }
    // wss:// to the issuer: the graph carries two rustls backends.
    let _ = rustls::crypto::ring::default_provider().install_default();
    let root = data_root();
    match args.get(1).map(String::as_str) {
        None | Some("run") => {
            // A long-running process: say what it does, as the server does.
            let _ = tracing_subscriber::fmt()
                .with_env_filter(
                    tracing_subscriber::EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| "task_server=info".into()),
                )
                .try_init();
            run(&root).await?;
        }
        Some("sign-in") => {
            let account = device_sign_in(&issuer()).await?;
            save(&root, &account)?;
            println!(
                "Signed in as {} — run `task-server peer` to start.",
                account.email
            );
        }
        Some("status") => match load(&root) {
            Some(a) => {
                println!("signed in as {} ({}) at {}", a.email, a.user_id, a.issuer);
                if let Ok(entries) = std::fs::read_dir(root.join("orgs")) {
                    for e in entries.flatten() {
                        println!("  org {}", e.file_name().to_string_lossy());
                    }
                }
            }
            None => println!("not signed in — run `task-server peer sign-in`"),
        },
        Some(other) => {
            eprintln!(
                "unknown peer command `{other}`\n\nusage:\n  task-server peer sign-in\n  task-server peer\n  task-server peer status"
            );
            std::process::exit(2);
        }
    }
    Ok(true)
}

fn data_root() -> PathBuf {
    std::env::var_os("TASK_DATA_ROOT")
        .filter(|v| !v.is_empty())
        .map_or_else(|| PathBuf::from("data"), PathBuf::from)
}

fn issuer() -> String {
    std::env::var("TASK_CENTRAL_AUTH_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ISSUER.to_owned())
        .trim_end_matches('/')
        .to_owned()
}

/// The issuer's vox endpoint, where its `DeviceService` answers.
#[must_use]
pub fn issuer_vox(issuer: &str) -> String {
    let ws = issuer
        .replacen("https://", "wss://", 1)
        .replacen("http://", "ws://", 1);
    format!("{}/vox", ws.trim_end_matches('/'))
}

fn account_file(root: &Path) -> PathBuf {
    root.join(ACCOUNT_FILE)
}

/// The saved account, if this server has been signed in.
#[must_use]
pub fn load(root: &Path) -> Option<PeerAccount> {
    let text = std::fs::read_to_string(account_file(root)).ok()?;
    serde_json::from_str(&text).ok()
}

/// Keep `account` as this server's sign-in (owner-only).
pub fn save(root: &Path, account: &PeerAccount) -> eyre::Result<()> {
    std::fs::create_dir_all(root).wrap_err("create the data root")?;
    let path = account_file(root);
    std::fs::write(&path, serde_json::to_vec_pretty(account)?)
        .wrap_err_with(|| format!("write {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Sign in from another device (RFC 8628) — the code is approved on a
/// phone that is signed in, since a server has no browser.
async fn device_sign_in(issuer: &str) -> eyre::Result<PeerAccount> {
    use serde_json::{Value, json};
    let http = reqwest::Client::new();
    let post = |path: &str, body: Value| {
        http.post(format!("{issuer}/auth/{path}"))
            .json(&body)
            .send()
    };
    let started: Value = post("device/code", json!({ "client_id": "task-server" }))
        .await
        .wrap_err_with(|| format!("start device sign-in at {issuer}"))?
        .json()
        .await
        .wrap_err("read the device code")?;
    let field = |k: &str| {
        started
            .get(k)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned()
    };
    println!("To sign this server in, open this on your phone:\n");
    println!("    {issuer}{}\n", field("verification_uri_complete"));
    println!(
        "or go to {issuer}{} and enter the code  {}\n",
        field("verification_uri"),
        field("user_code")
    );
    println!("Waiting for it to be approved…");
    let mut interval = Duration::from_secs(
        started
            .get("interval_seconds")
            .and_then(Value::as_u64)
            .unwrap_or(5)
            .max(1),
    );
    let body = json!({ "device_code": field("device_code"), "user_agent": "task-server" });
    loop {
        tokio::time::sleep(interval).await;
        let response = post("device/token", body.clone())
            .await
            .wrap_err_with(|| format!("poll device sign-in at {issuer}"))?;
        let ok = response.status().is_success();
        let text = response.text().await?;
        let v: Value = serde_json::from_str(&text).unwrap_or_default();
        if ok {
            let s = |path: &[&str]| {
                path.iter()
                    .try_fold(&v, |at, k| at.get(*k))
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned()
            };
            let account = PeerAccount {
                issuer: issuer.to_owned(),
                user_id: s(&["user", "id"]),
                email: s(&["user", "email"]),
                name: s(&["user", "name"]),
                token: s(&["token"]),
            };
            if account.token.is_empty() || account.user_id.is_empty() {
                return Err(eyre!(
                    "{issuer} answered a session this cannot read: {text}"
                ));
            }
            return Ok(account);
        }
        match v.get("code").and_then(Value::as_str) {
            Some("verification_required") => {}
            Some("invalid_input") if text.contains("slow_down") => {
                interval += Duration::from_secs(5)
            }
            Some("permission_denied") => return Err(eyre!("the sign-in was refused")),
            Some("invalid_credentials") => {
                return Err(eyre!(
                    "the code expired before it was approved — run it again"
                ));
            }
            _ => return Err(eyre!("device sign-in at {issuer} failed: {text}")),
        }
    }
}

fn this_device(root: &Path, account: &PeerAccount) -> eyre::Result<ThisDevice> {
    let install = install_id(root)?;
    let host = std::env::var("TASK_PEER_NAME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| {
            std::process::Command::new("hostname")
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
                .filter(|h| !h.is_empty())
        })
        .unwrap_or_else(|| "Server".to_owned());
    Ok(ThisDevice {
        device_id: device_id(install, &account.user_id),
        name: host,
        platform: "server".to_owned(),
        account: account
            .user_id
            .parse()
            .map_err(|_| eyre!("the account id is not a uuid: {}", account.user_id))?,
    })
}

/// Boot the node, serve it, and run the account pass on a cadence.
async fn run(root: &Path) -> eyre::Result<()> {
    let account = load(root).ok_or_else(|| {
        eyre!("this server is not signed in — run `task-server peer sign-in` first")
    })?;
    // Accept the account's sessions (and its bots' keys) on every org.
    // SAFETY: before the node or any other thread reads the environment.
    if std::env::var_os("TASK_CENTRAL_AUTH_URL").is_none() {
        unsafe { std::env::set_var("TASK_CENTRAL_AUTH_URL", &account.issuer) };
    }
    let bind: std::net::SocketAddr = std::env::var("TASK_SERVER_BIND")
        .unwrap_or_else(|_| "0.0.0.0:9090".into())
        .parse()
        .wrap_err("invalid TASK_SERVER_BIND")?;
    let node = LocalNode::open(NodeOptions {
        owner_name: if account.name.is_empty() {
            account.email.clone()
        } else {
            account.name.clone()
        },
        ..NodeOptions::at(root)
    })
    .await
    .wrap_err("open the node")?;
    let node = std::sync::Arc::new(node);
    let me = this_device(root, &account)?;
    info!(account = %account.email, device = %me.name, "peer: signed in as the account");

    let passes = {
        let node = node.clone();
        let account = account.clone();
        tokio::spawn(async move {
            loop {
                match pass(&node, &account, &me).await {
                    Ok(said) => info!("peer: {said}"),
                    Err(e) => warn!("peer: account pass failed — {e:#}"),
                }
                tokio::time::sleep(PASS_EVERY).await;
            }
        })
    };
    let served = node.serve_at(bind).await;
    passes.abort();
    served
}

async fn pass(node: &LocalNode, account: &PeerAccount, me: &ThisDevice) -> eyre::Result<String> {
    let issuer: architect_auth::DeviceServiceClient =
        task_dial::establish_at(&issuer_vox(&account.issuer), None)
            .await
            .map_err(|e| eyre!("reach the issuer: {e}"))?;
    let sync = sync_account(node, &issuer, &account.token, me).await?;
    Ok(match sync.slug {
        Some(slug) => format!(
            "holding `{slug}`{} with {} other device(s), {} admitted",
            if sync.joined {
                " (joined this pass)"
            } else {
                ""
            },
            sync.others.len(),
            sync.admitted.len()
        ),
        None => format!(
            "waiting for one of the account's {} other device(s) to be reachable",
            sync.others.len()
        ),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_issuer_vox_lane_follows_its_scheme() {
        assert_eq!(
            super::issuer_vox("https://auth.fasttrackstudio.app/"),
            "wss://auth.fasttrackstudio.app/vox"
        );
        assert_eq!(
            super::issuer_vox("http://127.0.0.1:8080"),
            "ws://127.0.0.1:8080/vox"
        );
    }

    #[test]
    fn the_account_is_kept_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let account = super::PeerAccount {
            issuer: "https://a".into(),
            user_id: uuid::Uuid::new_v4().to_string(),
            email: "sam@example.com".into(),
            name: "Sam".into(),
            token: "t".into(),
        };
        super::save(dir.path(), &account).unwrap();
        assert_eq!(super::load(dir.path()).unwrap().email, "sam@example.com");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join(super::ACCOUNT_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
