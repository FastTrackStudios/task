//! Account configuration for the email feature.
//!
//! - [`AccountConfig`] — what a UI / app needs to construct a backend
//!   instance: identity, backend choice, credentials, folder aliases.
//! - [`FolderAliases`] — case-insensitive `Alias → BackendName` map.
//!   Lets the UI keep stable folder IDs (`"Sent"`) while the backend
//!   sees whatever weird name the server uses (`"[Gmail]/Sent Mail"`).
//! - [`BackendKind`] — open enum naming the implementations under
//!   `features/email/email-{imap,jmap,maildir,nextcloud}`.
//!
//! Config is a **typed document the UI owns and mutates.** OAuth
//! refresh writes new tokens here, add-account wizards append
//! entries. Don't fall into the himalaya `himalaya.toml` shape of
//! re-reading on every call — load once, hold in memory, persist
//! on change.

mod aliases;

pub use aliases::{FolderAliases, FolderName};

use email_proto::{Account, AccountId};
use email_secret::Secret;
use facet::Facet;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Self-contained config for one account.
#[derive(Debug, Clone, Serialize, Deserialize, Facet)]
pub struct AccountConfig {
    pub id: AccountId,
    pub name: String,
    pub address: String,
    pub display_name: Option<String>,
    pub backend: BackendKind,
    /// Default signature appended to outgoing messages.
    #[serde(default)]
    pub signature: Option<String>,
    /// Case-insensitive alias → backend-name map. Empty by default.
    #[serde(default)]
    pub folder_aliases: FolderAliases,
}

impl AccountConfig {
    #[must_use]
    pub fn to_account(&self) -> Account {
        Account {
            id: self.id.clone(),
            name: self.name.clone(),
            address: self.address.clone(),
            display_name: self.display_name.clone(),
        }
    }

    /// Load one account config from a JSON file (the
    /// `<account_root>/account.json` convention the server's
    /// maildir account discovery reads). Returns `Ok(None)` when
    /// the file doesn't exist; a present-but-invalid file is an
    /// error so a typo'd config fails loudly instead of silently
    /// degrading the account.
    pub fn load_json(path: &std::path::Path) -> Result<Option<Self>, String> {
        let bytes = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(format!("read {}: {e}", path.display())),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| format!("parse {}: {e}", path.display()))
    }

    /// Persist as pretty JSON at `path` (companion of
    /// [`Self::load_json`]).
    pub fn save_json(&self, path: &std::path::Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::write(path, json).map_err(|e| format!("write {}: {e}", path.display()))
    }
}

/// Which backend serves this account. Open in the wire format —
/// we serialize the tag, so new variants don't break existing
/// configs.
#[derive(Debug, Clone, Serialize, Deserialize, Facet)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[repr(u8)]
pub enum BackendKind {
    /// Local Maildir at `root`. `submit` optionally wires an SMTP
    /// submission endpoint so the account can *send* — the maildir
    /// stays the local store, outgoing mail goes over SMTP and the
    /// sent copy lands in the maildir's `Sent` folder.
    Maildir {
        root: PathBuf,
        #[serde(default)]
        submit: Option<SmtpConfig>,
    },
    /// IMAP + SMTP server pair. The SMTP host is configured
    /// separately so we can submit through a different relay
    /// (corp gateway, Fastmail's app password endpoint, etc).
    Imap {
        host: String,
        port: u16,
        tls: TlsMode,
        username: String,
        password: Secret,
        submit: Option<SmtpConfig>,
    },
    /// JMAP endpoint + bearer/OAuth token.
    Jmap {
        session_url: String,
        credentials: Secret,
    },
    /// Nextcloud Mail HTTP API hosted on an existing NC instance.
    Nextcloud {
        base_url: String,
        username: String,
        /// App password recommended; password resolver lets us
        /// pull from the NC client keyring.
        password: Secret,
    },
}

/// SMTP submission endpoint (used by [`BackendKind::Imap`]).
#[derive(Debug, Clone, Serialize, Deserialize, Facet)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub username: String,
    pub password: Secret,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Facet)]
#[serde(rename_all = "snake_case")]
#[repr(u8)]
pub enum TlsMode {
    /// TLS on connect (port 993 / 465).
    Implicit,
    /// Plain socket, upgrade with STARTTLS (port 143 / 587).
    Starttls,
    /// Plaintext. Tests / loopback only.
    None,
}
