//! ISP autoconfiguration. Resolve an email address to its
//! IMAP / SMTP / JMAP endpoints so the add-account wizard can
//! say "enter your email" and have the rest fall out.
//!
//! Three sources, tried in this order (matches Thunderbird's
//! own behavior):
//!
//! 1. **Mozilla ISPDB.** A curated database of provider configs
//!    served from `https://autoconfig.thunderbird.net/v1.1/<domain>`.
//!    Covers Gmail, Outlook, Fastmail, Yandex, plus hundreds of
//!    smaller providers.
//! 2. **Provider-served autoconfig.** Some providers publish
//!    their own XML at `https://autoconfig.<domain>/mail/config-v1.1.xml`
//!    or `https://<domain>/.well-known/mail-config`. Same
//!    schema as ISPDB.
//! 3. **DNS SRV (RFC 6186).** `_imaps._tcp.<domain>`,
//!    `_imap._tcp.<domain>`, `_submission._tcp.<domain>`. No
//!    auth method info — only host/port; the caller must guess
//!    `STARTTLS` vs `Implicit` from the port.
//!
//! Returns [`AutoconfigResult`] — a list of candidate
//! [`Server`] entries. The wizard picks the highest-priority
//! incoming + outgoing pair and feeds them into
//! [`email_config::BackendKind::Imap`].

mod ispdb;
mod srv;
mod xml;

pub use ispdb::ProviderSource;

use email_config::TlsMode;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Concrete endpoint candidate. One incoming server + one
/// outgoing server is enough to configure an IMAP account.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Server {
    pub protocol: Protocol,
    pub host: String,
    pub port: u16,
    pub tls: TlsMode,
    pub auth: Vec<AuthMethod>,
    /// Username placeholder as published by the provider —
    /// `%EMAILADDRESS%` for the full email, `%EMAILLOCALPART%`
    /// for the local part. The wizard substitutes before save.
    pub username: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    Imap,
    Smtp,
    Jmap,
    Pop3,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMethod {
    PasswordCleartext,
    PasswordEncrypted,
    OAuth2,
    Ntlm,
    GssApi,
    ClientIpAddress,
    TlsClientCert,
}

/// Aggregated result. Empty `incoming` + `outgoing` means none
/// of the sources had anything; the wizard falls back to
/// manual entry.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AutoconfigResult {
    pub incoming: Vec<Server>,
    pub outgoing: Vec<Server>,
    /// Which source produced this result. Useful for telemetry
    /// + a "we found this at $source" line in the UI.
    pub source: Option<String>,
}

impl AutoconfigResult {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.incoming.is_empty() && self.outgoing.is_empty()
    }
}

/// Resolve `email` to a configuration. Tries each source in
/// order; the first non-empty result wins.
///
/// `email` is split at `@`; only the domain portion is used
/// for lookups. The local part is preserved on the returned
/// `Server::username` so callers can substitute placeholders.
pub async fn lookup(email: &str) -> Result<AutoconfigResult, Error> {
    let (_local, domain) = split_email(email)?;
    tracing::debug!(domain, "autoconfig lookup");

    // 1. Mozilla ISPDB
    match ispdb::lookup(domain, ProviderSource::Mozilla).await {
        Ok(Some(r)) => return Ok(r),
        Ok(None) => tracing::debug!("ispdb: no match"),
        Err(e) => tracing::debug!(error = %e, "ispdb failed"),
    }

    // 2. Provider-served autoconfig
    match ispdb::lookup(domain, ProviderSource::Provider).await {
        Ok(Some(r)) => return Ok(r),
        Ok(None) => tracing::debug!("provider autoconfig: no match"),
        Err(e) => tracing::debug!(error = %e, "provider autoconfig failed"),
    }

    // 3. DNS SRV
    match srv::lookup(domain).await {
        Ok(r) if !r.is_empty() => return Ok(r),
        Ok(_) => tracing::debug!("srv: no records"),
        Err(e) => tracing::debug!(error = %e, "srv failed"),
    }

    Ok(AutoconfigResult::default())
}

fn split_email(email: &str) -> Result<(&str, &str), Error> {
    let (local, domain) = email.split_once('@').ok_or(Error::BadEmail)?;
    if local.is_empty() || domain.is_empty() || domain.contains('@') {
        return Err(Error::BadEmail);
    }
    Ok((local, domain))
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("invalid email address")]
    BadEmail,
    #[error("http error: {0}")]
    Http(String),
    #[error("xml parse error: {0}")]
    Xml(String),
    #[error("dns error: {0}")]
    Dns(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_email_handles_normal_address() {
        assert_eq!(
            split_email("alice@example.com").unwrap(),
            ("alice", "example.com")
        );
    }

    #[test]
    fn split_email_rejects_garbage() {
        assert!(split_email("not-an-email").is_err());
        assert!(split_email("@example.com").is_err());
        assert!(split_email("alice@").is_err());
        assert!(split_email("alice@host@evil").is_err());
    }
}
