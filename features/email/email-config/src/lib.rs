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
    /// STARTTLS against a certificate we cannot verify.
    ///
    /// This exists for one thing: a **local mail bridge**. Proton Mail
    /// has no IMAP of its own — you run Proton Mail Bridge, which
    /// decrypts locally and re-serves the mailbox on `127.0.0.1:1143`
    /// under a certificate it generates on that machine. No CA has
    /// signed it and no CA ever will, so ordinary verification can only
    /// fail.
    ///
    /// Skipping verification is safe *here* and nowhere else: the
    /// connection never leaves the loopback interface, so there is no
    /// network position from which to substitute a certificate. The
    /// backends enforce exactly that — this mode is **refused for any
    /// host that is not loopback**, which is what keeps it from becoming
    /// a convenient way to silence a genuine certificate error on a real
    /// remote server.
    StarttlsSelfSigned,
    /// STARTTLS against an unverifiable certificate, on a host the
    /// operator has deliberately chosen to trust.
    ///
    /// The case this exists for is a bridge that is not on this machine.
    /// Proton Mail Bridge can run as its own workload — a pod in a
    /// cluster, a box on a home network — with the mail server reaching
    /// it across that network. Bridge still mints its own certificate,
    /// and it keeps that certificate inside an encrypted vault with no
    /// headless way to export it, so there is nothing to pin and
    /// verification cannot be made to succeed.
    ///
    /// **This one cannot be enforced, and that is the difference.**
    /// Loopback is checkable: the bytes provably never reach a network.
    /// "This network is trustworthy" is not something code can confirm,
    /// and a hostname cannot be classified without resolving it — which
    /// is the very step whose answer would have to be trusted. So the
    /// safety is the operator's judgement, and the variant is named for
    /// what it assumes so that assumption is legible in the account file
    /// instead of hiding behind a flag that reads like a detail.
    ///
    /// Use it for a private network you control. Aimed at a public
    /// server it sends the account's password to whoever answers.
    StarttlsTrustedNetwork,
    /// Plaintext. Tests / loopback only.
    None,
}

/// Does this host name a loopback interface?
///
/// Gates [`TlsMode::StarttlsSelfSigned`] in every backend, so it lives
/// here beside the mode rather than being written twice.
///
/// String comparison on purpose: the name is what the TLS handshake
/// will be told, and `localhost` resolving elsewhere — a hosts-file
/// entry, a search domain — is precisely the case this should refuse
/// rather than accept. A host that has to be resolved first is not one
/// we can call loopback.
#[must_use]
pub fn is_loopback_host(host: &str) -> bool {
    if host == "localhost" {
        return true;
    }
    // Accept a bracketed IPv6 literal the way a URL would write it.
    let bare = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    bare.parse::<std::net::IpAddr>()
        .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod loopback_tests {
    use super::is_loopback_host;

    #[test]
    fn the_two_unverified_modes_are_distinguishable_on_the_wire() {
        // They serialize differently on purpose. An account file is where
        // the trust decision has to be readable months later, and
        // "starttls_self_signed" (provably not a network) must not be
        // mistaken for "starttls_trusted_network" (somebody's judgement).
        let local = serde_json::to_string(&super::TlsMode::StarttlsSelfSigned).expect("encode");
        let remote =
            serde_json::to_string(&super::TlsMode::StarttlsTrustedNetwork).expect("encode");
        assert_eq!(local, "\"starttls_self_signed\"");
        assert_eq!(remote, "\"starttls_trusted_network\"");
        assert_ne!(local, remote);

        // And an old config keeps meaning what it meant.
        let back: super::TlsMode =
            serde_json::from_str("\"starttls_self_signed\"").expect("decode legacy");
        assert_eq!(back, super::TlsMode::StarttlsSelfSigned);
    }

    #[test]
    fn only_the_local_machine_counts_as_loopback() {
        for host in ["127.0.0.1", "127.0.1.1", "::1", "[::1]", "localhost"] {
            assert!(is_loopback_host(host), "{host} is loopback");
        }
        // The whole safety argument for skipping verification is that
        // the bytes never reach a network. A name that resolves — even
        // one that looks local — is off the table, because what it
        // resolves to is not ours to decide.
        for host in [
            "imap.gmail.com",
            "192.168.1.10",
            "10.0.0.1",
            "localhost.evil.test",
            "127.0.0.1.evil.test",
            "",
        ] {
            assert!(!is_loopback_host(host), "{host} must not pass as loopback");
        }
    }
}
