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

/// `<app data>/task`, as the desktop app resolves it
/// ([`task_dial::default_app_dir`], the one place the path is spelled).
#[must_use]
pub fn app_dir() -> Option<PathBuf> {
    task_dial::default_app_dir()
}

/// The desktop node's data root ([`task_dial::default_node_root`]).
#[must_use]
pub fn node_root() -> Option<PathBuf> {
    task_dial::default_node_root()
}

/// The running desktop node's `ws://` base, if it says it is serving
/// ([`task_dial::read_local_endpoint`]: loopback only).
#[must_use]
pub fn node_endpoint() -> Option<String> {
    task_dial::read_local_endpoint(&node_root()?)
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
