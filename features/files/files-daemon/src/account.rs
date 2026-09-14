//! The account this machine syncs as.
//!
//! Pairing is per org, and a person in seven orgs did it seven times —
//! then told the daemon about seven endpoints by hand, and did it all
//! again for the eighth org. What they wanted was to sign in once and
//! see everything their account can see. So the daemon can hold an
//! account: a Task server and a session token, kept here, and on a
//! cadence it asks that server to enrol this machine with every org
//! the account is in (`DeviceEnrollmentService::enroll_everywhere`,
//! one call), then admits each org's endpoint and pulls what it offers.
//!
//! This module is the pure half: where the token lives on disk and how
//! it is protected, what the server URL becomes, and — given what the
//! account was in last time and what it is in now — which endpoints to
//! admit and which to forget. The network round is on
//! [`crate::SyncDaemon`], which owns the peers.
//!
//! # The token is a secret and is filed like one
//!
//! `<data>/account.json` names the server; `<data>/account-token`
//! holds the token alone, mode `0600`, so a backup of the data dir or a
//! `cat` of the config does not leak it. Sign-out removes both.

use std::path::{Path, PathBuf};

use crate::error::{DaemonError, Result};

/// The Task server a daemon signs in to when nobody names one.
pub const DEFAULT_SERVER: &str = "wss://task.fasttrackstudio.app";

/// A stored sign-in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// The server, as given (`wss://…` or `https://…`); see
    /// [`server_vox_url`] for what is dialled.
    pub server: String,
    pub token: String,
}

/// Where the account lives.
#[derive(Debug, Clone)]
pub struct AccountStore {
    config: PathBuf,
    token: PathBuf,
}

#[derive(serde::Serialize, serde::Deserialize)]
struct StoredConfig {
    server: String,
}

impl AccountStore {
    #[must_use]
    pub fn open(data_dir: &Path) -> Self {
        Self {
            config: data_dir.join("account.json"),
            token: data_dir.join("account-token"),
        }
    }

    /// Keep a sign-in. The token file is created (or replaced) with
    /// owner-only permissions before the token is written into it.
    pub fn save(&self, account: &Account) -> Result<()> {
        if account.token.trim().is_empty() {
            return Err(DaemonError::BadRequest("a sign-in needs a token".into()));
        }
        let server = if account.server.trim().is_empty() {
            DEFAULT_SERVER.to_owned()
        } else {
            account.server.trim().to_owned()
        };
        if let Some(dir) = self.config.parent() {
            std::fs::create_dir_all(dir).map_err(|e| io(&self.config, &e))?;
        }
        let config = serde_json::to_string_pretty(&StoredConfig { server })
            .map_err(|e| DaemonError::Io(e.to_string()))?;
        std::fs::write(&self.config, config).map_err(|e| io(&self.config, &e))?;
        write_secret(&self.token, account.token.trim())?;
        Ok(())
    }

    /// The stored sign-in, if there is one. A config with no token
    /// file (or the reverse) is "signed out": half a sign-in is no
    /// sign-in, and reading it as one would dial a server with nothing
    /// to say.
    #[must_use]
    pub fn load(&self) -> Option<Account> {
        let config: StoredConfig =
            serde_json::from_str(&std::fs::read_to_string(&self.config).ok()?).ok()?;
        let token = std::fs::read_to_string(&self.token).ok()?;
        let token = token.trim();
        if token.is_empty() {
            return None;
        }
        Some(Account {
            server: config.server,
            token: token.to_owned(),
        })
    }

    /// Forget the sign-in. Missing files are already forgotten.
    pub fn clear(&self) -> Result<()> {
        for path in [&self.token, &self.config] {
            match std::fs::remove_file(path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(path, &e)),
            }
        }
        Ok(())
    }
}

fn io(path: &Path, e: &std::io::Error) -> DaemonError {
    DaemonError::Io(format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn write_secret(path: &Path, secret: &str) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| io(path, &e))?;
    // A file that already existed keeps its old mode through `open`;
    // set it again so a token file somebody once made world-readable
    // is not left that way.
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| io(path, &e))?;
    file.write_all(secret.as_bytes())
        .map_err(|e| io(path, &e))?;
    Ok(())
}

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

#[cfg(not(unix))]
fn write_secret(path: &Path, secret: &str) -> Result<()> {
    std::fs::write(path, secret).map_err(|e| io(path, &e))
}

/// The `/server/vox` URL for a server given as `wss://host`,
/// `https://host`, `host`, or already as the full lane URL.
///
/// The server lane is where `DeviceEnrollmentService` lives: it acts
/// across orgs, so no org lane could host it.
#[must_use]
pub fn server_vox_url(server: &str) -> String {
    let raw = server.trim().trim_end_matches('/');
    let raw = if raw.is_empty() { DEFAULT_SERVER } else { raw };
    let with_scheme = if let Some(rest) = raw.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = raw.strip_prefix("http://") {
        format!("ws://{rest}")
    } else if raw.starts_with("ws://") || raw.starts_with("wss://") {
        raw.to_owned()
    } else {
        format!("wss://{raw}")
    };
    if with_scheme.ends_with("/server/vox") {
        with_scheme
    } else {
        format!("{with_scheme}/server/vox")
    }
}

/// What an enrolment round changes about the peer set.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Reconcile {
    /// Org endpoints the account reaches now and did not last time —
    /// admit and pull.
    pub added: Vec<String>,
    /// Endpoints it reached last time and still does.
    pub kept: Vec<String>,
    /// Endpoints it reached last time and no longer does — the account
    /// left the org, or was removed. Forget them: an org somebody was
    /// removed from must not keep arriving on their disk.
    pub removed: Vec<String>,
}

/// Given the endpoints the account's orgs answered with last round and
/// this round, which to admit and which to forget. Empty ids (an org
/// with no peering endpoint yet) are not peers and are ignored.
#[must_use]
pub fn reconcile(previous: &[String], now: &[String]) -> Reconcile {
    let now: Vec<&String> = now.iter().filter(|e| !e.trim().is_empty()).collect();
    let mut out = Reconcile::default();
    for endpoint in &now {
        if previous.iter().any(|p| p == *endpoint) {
            out.kept.push((*endpoint).clone());
        } else {
            out.added.push((*endpoint).clone());
        }
    }
    for endpoint in previous {
        if endpoint.trim().is_empty() {
            continue;
        }
        if !now.contains(&endpoint) {
            out.removed.push(endpoint.clone());
        }
    }
    out
}

/// This machine's name for an org's device list.
#[must_use]
pub fn machine_name() -> String {
    std::process::Command::new("hostname")
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "this machine".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_sign_in_round_trips_and_the_token_is_owner_only() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path());
        assert!(store.load().is_none(), "nothing stored yet");

        store
            .save(&Account {
                server: "https://task.example".into(),
                token: "  tok-123  ".into(),
            })
            .unwrap();
        let loaded = store.load().expect("stored");
        assert_eq!(loaded.server, "https://task.example");
        assert_eq!(loaded.token, "tok-123", "trimmed");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(dir.path().join("account-token"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777;
            assert_eq!(mode, 0o600, "the token file is owner-only");
        }

        store.clear().unwrap();
        assert!(store.load().is_none());
        store.clear().unwrap();
    }

    #[test]
    fn half_a_sign_in_is_signed_out() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path());
        std::fs::write(
            dir.path().join("account.json"),
            r#"{"server":"wss://task.example"}"#,
        )
        .unwrap();
        assert!(store.load().is_none(), "a server with no token is nothing");
        std::fs::write(dir.path().join("account-token"), "   ").unwrap();
        assert!(store.load().is_none(), "a blank token is nothing");
    }

    #[test]
    fn a_blank_token_is_refused_and_a_blank_server_is_the_default() {
        let dir = tempfile::tempdir().unwrap();
        let store = AccountStore::open(dir.path());
        assert!(
            store
                .save(&Account {
                    server: String::new(),
                    token: "  ".into()
                })
                .is_err()
        );
        store
            .save(&Account {
                server: String::new(),
                token: "t".into(),
            })
            .unwrap();
        assert_eq!(store.load().unwrap().server, DEFAULT_SERVER);
    }

    #[test]
    fn the_server_lane_url_is_derived_from_any_spelling() {
        assert_eq!(
            server_vox_url("https://task.example/"),
            "wss://task.example/server/vox"
        );
        assert_eq!(
            server_vox_url("http://localhost:18080"),
            "ws://localhost:18080/server/vox"
        );
        assert_eq!(
            server_vox_url("task.example"),
            "wss://task.example/server/vox"
        );
        assert_eq!(
            server_vox_url("wss://task.example/server/vox"),
            "wss://task.example/server/vox"
        );
        assert_eq!(
            server_vox_url(""),
            "wss://task.fasttrackstudio.app/server/vox"
        );
    }

    #[test]
    fn a_round_admits_new_orgs_keeps_old_ones_and_forgets_the_departed() {
        let previous = vec!["a".to_owned(), "b".to_owned(), String::new()];
        let now = vec!["b".to_owned(), "c".to_owned(), String::new()];
        let r = reconcile(&previous, &now);
        assert_eq!(r.added, ["c"]);
        assert_eq!(r.kept, ["b"]);
        assert_eq!(r.removed, ["a"]);
        assert_eq!(reconcile(&[], &[]), Reconcile::default());
    }
}
