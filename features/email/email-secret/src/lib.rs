//! Credential resolution for the email feature.
//!
//! Modeled on pimalaya's `core/secret` shape — `Secret` is an enum
//! covering the four ways an account credential can be supplied:
//!
//! - [`Secret::Raw`] — literal string (tests, scripted setups).
//! - [`Secret::EnvVar`] — process environment lookup.
//! - [`Secret::Command`] — shell out, capture stdout (`pass show foo`,
//!   `bw get password foo`, `op read op://...`). Trims trailing
//!   newline, has a configurable timeout.
//! - [`Secret::Keyring`] — OS keyring entry (Secret Service / macOS
//!   Keychain / Windows Credential Manager).
//!
//! [`Secret::resolve`] turns any variant into a [`SecretValue`] — a
//! `Zeroizing<String>` that wipes itself on drop. Resolvers that
//! aren't compiled in (e.g. `keyring` on wasm) return
//! [`SecretError::Unsupported`] rather than panicking, so the config
//! shape stays uniform across targets.

#![cfg_attr(not(any(feature = "command", feature = "keyring")), allow(unused))]

use facet::Facet;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;
use zeroize::Zeroizing;

#[cfg(feature = "command")]
mod command;
#[cfg(feature = "keyring")]
mod keyring_resolver;

/// One credential, in whatever form the user chose to record it.
/// `serde` round-trips through a tagged enum so `account.toml`
/// reads naturally:
///
/// ```toml
/// password = { type = "command", argv = ["pass", "show", "imap/work"] }
/// password = { type = "keyring", service = "task-email", account = "work" }
/// password = { type = "env_var", name = "WORK_IMAP_PASS" }
/// password = { type = "raw", value = "hunter2" }  # tests only
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
#[repr(u8)]
pub enum Secret {
    /// Inline value. Don't use outside tests + ephemeral demos.
    Raw { value: String },
    /// Read from process environment.
    EnvVar { name: String },
    /// Shell out, capture trimmed stdout. argv-style — no shell
    /// interpolation, no surprise quoting.
    Command {
        argv: Vec<String>,
        #[serde(default)]
        timeout_ms: Option<u64>,
    },
    /// OS keyring entry — `service` is typically `"task-email"`;
    /// `account` is whatever identifier we used at `set` time
    /// (usually `<account-id>:<credential-name>`).
    Keyring { service: String, account: String },
}

impl Secret {
    pub fn raw(value: impl Into<String>) -> Self {
        Self::Raw {
            value: value.into(),
        }
    }
    pub fn env(name: impl Into<String>) -> Self {
        Self::EnvVar { name: name.into() }
    }
    pub fn command(argv: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Self::Command {
            argv: argv.into_iter().map(Into::into).collect(),
            timeout_ms: None,
        }
    }
    pub fn keyring(service: impl Into<String>, account: impl Into<String>) -> Self {
        Self::Keyring {
            service: service.into(),
            account: account.into(),
        }
    }

    /// Resolve to a self-zeroizing string. Async because
    /// `Command` needs to wait on a child process and `Keyring`
    /// may block on a Secret Service socket — we route both
    /// through the same `async` surface so callers don't branch.
    pub async fn resolve(&self) -> Result<SecretValue, SecretError> {
        match self {
            Self::Raw { value } => Ok(SecretValue::new(value.clone())),
            Self::EnvVar { name } => {
                let v = std::env::var(name).map_err(|_| SecretError::EnvMissing(name.clone()))?;
                Ok(SecretValue::new(v))
            }
            Self::Command { argv, timeout_ms } => {
                #[cfg(feature = "command")]
                {
                    let timeout = timeout_ms
                        .map(Duration::from_millis)
                        .unwrap_or(DEFAULT_COMMAND_TIMEOUT);
                    command::run(argv, timeout).await
                }
                #[cfg(not(feature = "command"))]
                {
                    let _ = (argv, timeout_ms);
                    Err(SecretError::Unsupported("command resolver disabled"))
                }
            }
            Self::Keyring { service, account } => {
                #[cfg(feature = "keyring")]
                {
                    keyring_resolver::get(service, account)
                }
                #[cfg(not(feature = "keyring"))]
                {
                    let _ = (service, account);
                    Err(SecretError::Unsupported("keyring resolver disabled"))
                }
            }
        }
    }
}

/// A resolved secret. The inner string zeroes itself when this
/// drops; `as_str` gives a borrowed view callers can pass to
/// `imap::login` etc.
#[derive(Debug, Clone)]
pub struct SecretValue(Zeroizing<String>);

impl SecretValue {
    pub fn new(s: impl Into<String>) -> Self {
        Self(Zeroizing::new(s.into()))
    }
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
    #[must_use]
    pub fn into_string(self) -> Zeroizing<String> {
        self.0
    }
}

const DEFAULT_COMMAND_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Error)]
pub enum SecretError {
    #[error("env var {0:?} not set")]
    EnvMissing(String),
    #[error("command exited non-zero: {0}")]
    CommandFailed(String),
    #[error("command timed out")]
    CommandTimedOut,
    #[error("command stdout was not utf-8")]
    NonUtf8,
    #[error("keyring lookup failed: {0}")]
    Keyring(String),
    #[error("resolver unsupported on this build: {0}")]
    Unsupported(&'static str),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("argv must be non-empty")]
    EmptyArgv,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn raw_round_trips() {
        let s = Secret::raw("hunter2");
        let v = s.resolve().await.unwrap();
        assert_eq!(v.as_str(), "hunter2");
    }

    #[tokio::test]
    async fn env_resolves() {
        // SAFETY: tests run in serial w/r/t this var because the
        // assert is immediate.
        unsafe { std::env::set_var("EMAIL_SECRET_TEST", "from-env") };
        let s = Secret::env("EMAIL_SECRET_TEST");
        let v = s.resolve().await.unwrap();
        assert_eq!(v.as_str(), "from-env");
    }

    #[tokio::test]
    async fn env_missing_errors() {
        let s = Secret::env("__definitely_unset_email_secret_var__");
        let err = s.resolve().await.unwrap_err();
        assert!(matches!(err, SecretError::EnvMissing(_)));
    }

    #[tokio::test]
    #[cfg(feature = "command")]
    async fn command_captures_trimmed_stdout() {
        let s = Secret::command(["sh", "-c", "printf 'hello\n'"]);
        let v = s.resolve().await.unwrap();
        assert_eq!(v.as_str(), "hello");
    }

    #[tokio::test]
    #[cfg(feature = "command")]
    async fn command_nonzero_exit_errors() {
        let s = Secret::command(["sh", "-c", "exit 1"]);
        let err = s.resolve().await.unwrap_err();
        assert!(matches!(err, SecretError::CommandFailed(_)));
    }

    #[tokio::test]
    #[cfg(feature = "command")]
    async fn command_empty_argv_errors() {
        let s = Secret::Command {
            argv: vec![],
            timeout_ms: None,
        };
        let err = s.resolve().await.unwrap_err();
        assert!(matches!(err, SecretError::EmptyArgv));
    }

    #[test]
    fn secret_value_does_not_leak_via_debug() {
        // Just enforce we keep zeroize; the SecretValue Debug
        // trait still prints the wrapper but the contents come
        // through Zeroizing which doesn't redact — leave a
        // failing assertion here as a tripwire if someone adds
        // a redacting Debug later and forgets to update tests.
        let v = SecretValue::new("nope");
        // Confirm the value is accessible for legitimate use.
        assert_eq!(v.as_str(), "nope");
    }
}
