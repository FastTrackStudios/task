//! DNS SRV resolver (RFC 6186). Used as the last-resort fallback
//! when neither ISPDB nor provider XML answers.
//!
//! We probe four records:
//! - `_imaps._tcp.<domain>` → incoming, implicit TLS (port 993)
//! - `_imap._tcp.<domain>`  → incoming, STARTTLS (port 143)
//! - `_submission._tcp.<domain>` → outgoing, STARTTLS (port 587)
//! - `_submissions._tcp.<domain>` → outgoing, implicit TLS (port 465)
//!
//! SRV gives only host/port; auth info has to be guessed —
//! we default to `PasswordCleartext` on whatever TLS variant
//! the record implies. Callers should still let the user
//! confirm before saving.

use crate::{AuthMethod, AutoconfigResult, Error, Protocol, Server};
use email_config::TlsMode;
use hickory_resolver::TokioAsyncResolver;
use hickory_resolver::config::{ResolverConfig, ResolverOpts};

#[derive(Debug, Clone, Copy)]
struct Probe {
    record: &'static str,
    protocol: Protocol,
    tls: TlsMode,
    incoming: bool,
}

const PROBES: &[Probe] = &[
    Probe {
        record: "_imaps._tcp",
        protocol: Protocol::Imap,
        tls: TlsMode::Implicit,
        incoming: true,
    },
    Probe {
        record: "_imap._tcp",
        protocol: Protocol::Imap,
        tls: TlsMode::Starttls,
        incoming: true,
    },
    Probe {
        record: "_submissions._tcp",
        protocol: Protocol::Smtp,
        tls: TlsMode::Implicit,
        incoming: false,
    },
    Probe {
        record: "_submission._tcp",
        protocol: Protocol::Smtp,
        tls: TlsMode::Starttls,
        incoming: false,
    },
];

pub async fn lookup(domain: &str) -> Result<AutoconfigResult, Error> {
    // Best-effort — use system config when available, fall back
    // to Google's public DNS so we still work in containers
    // with no `/etc/resolv.conf`.
    let resolver = TokioAsyncResolver::tokio_from_system_conf().unwrap_or_else(|_| {
        TokioAsyncResolver::tokio(ResolverConfig::google(), ResolverOpts::default())
    });

    let out = AutoconfigResult {
        source: Some(format!("dns-srv:{domain}")),
        ..AutoconfigResult::default()
    };
    let mut out = out;

    for probe in PROBES {
        let name = format!("{}.{}.", probe.record, domain);
        let lookup = match resolver.srv_lookup(&name).await {
            Ok(l) => l,
            Err(e) => {
                tracing::trace!(record = %name, error = %e, "srv miss");
                continue;
            }
        };
        for rec in lookup.iter() {
            let host = rec.target().to_utf8();
            let host = host.trim_end_matches('.').to_string();
            // RFC 6186 §3.6: a target of `.` means "service not
            // offered." Skip — there's nothing to connect to.
            if host.is_empty() {
                continue;
            }
            let server = Server {
                protocol: probe.protocol,
                host,
                port: rec.port(),
                tls: probe.tls,
                auth: vec![AuthMethod::PasswordCleartext],
                username: "%EMAILADDRESS%".into(),
            };
            if probe.incoming {
                out.incoming.push(server);
            } else {
                out.outgoing.push(server);
            }
        }
    }

    Ok(out)
}
