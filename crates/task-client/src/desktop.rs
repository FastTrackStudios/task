//! The desktop app's own node and sign-in, as the CLI's.
//!
//! The desktop app runs a node of its own (`task-node-host`) over
//! `<app data>/task/node`, and while it runs, serves it on loopback,
//! writing the `ws://` base to `<node root>/local-endpoint`
//! (`task_server::local_node::LOCAL_ENDPOINT_FILE`). The person's
//! sign-in lives beside it, in the desktop's token store
//! (`<app data>/task/ui-tokens/`: `active` names the email, `<email>.json`
//! holds the token).
//!
//! So with no `--server` and no CLI session of its own, the CLI talks to
//! that node as that person — one sign-in for both, and the same data the
//! desktop syncs. And a sign-in the CLI makes (`task auth login --device`)
//! is written to the same store, so the desktop has it too.
//!
//! Paths follow the desktop's: `$XDG_DATA_HOME` when set, else
//! `~/Library/Application Support` on Apple and `~/.local/share`
//! elsewhere; `$TASK_NODE_ROOT` overrides the node root as it does in the
//! app.

// t[impl offline.cli]
use std::path::PathBuf;

use architect_auth::client::{FileTokenStore, StoredSession, TokenStore as _};

/// `<app data>/task`, as the desktop app resolves it.
#[must_use]
pub fn app_dir() -> Option<PathBuf> {
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

/// The desktop node's data root.
#[must_use]
pub fn node_root() -> Option<PathBuf> {
    if let Some(root) = std::env::var_os("TASK_NODE_ROOT").filter(|r| !r.is_empty()) {
        return Some(PathBuf::from(root));
    }
    Some(app_dir()?.join("node"))
}

/// The running desktop node's `ws://` base, if it says it is serving.
#[must_use]
pub fn node_endpoint() -> Option<String> {
    let file = node_root()?.join(task_server::local_node::LOCAL_ENDPOINT_FILE);
    std::fs::read_to_string(file)
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| s.starts_with("ws://127.0.0.1:") || s.starts_with("ws://[::1]:"))
}

fn tokens_dir() -> Option<PathBuf> {
    Some(app_dir()?.join("ui-tokens"))
}

fn store_for(email: &str) -> Option<FileTokenStore> {
    let safe = email.replace(['/', '\\'], "_");
    Some(FileTokenStore::new(
        tokens_dir()?.join(format!("{safe}.json")),
    ))
}

/// The account signed in on the desktop: `(email, token)`.
#[must_use]
pub fn active_signin() -> Option<(String, String)> {
    let email = std::fs::read_to_string(tokens_dir()?.join("active"))
        .ok()?
        .trim()
        .to_owned();
    if email.is_empty() {
        return None;
    }
    let token = store_for(&email)?
        .load()
        .ok()
        .flatten()
        .map(|s| s.token)
        .filter(|t| !t.is_empty())?;
    Some((email, token))
}

/// Make `email` / `token` the desktop's sign-in too, so signing in from
/// the CLI signs the app in. Best effort: the CLI's own session is
/// already saved.
pub fn save_signin(email: &str, token: &str) -> std::io::Result<()> {
    let dir = tokens_dir().ok_or_else(|| std::io::Error::other("no home directory"))?;
    std::fs::create_dir_all(&dir)?;
    if let Some(store) = store_for(email) {
        store
            .save(&StoredSession::new(token.to_owned()).with_email(email.to_owned()))
            .map_err(|e| std::io::Error::other(e.to_string()))?;
    }
    std::fs::write(dir.join("active"), email)
}

/// Whether `url` is the desktop node's endpoint (same host and port).
#[must_use]
pub fn is_node_url(url: &str) -> bool {
    node_endpoint()
        .is_some_and(|base| crate::transport::authority(&base) == crate::transport::authority(url))
}
