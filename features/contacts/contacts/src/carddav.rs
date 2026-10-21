//! CardDAV import — the one-way pull (server → vault).
//!
//! [`import`] connects to a [`CardDavAccount`], discovers its
//! addressbook, fetches the vCards, and maps each to a
//! [`contacts_proto::Contact`] with `uid` / `etag` / `source` /
//! `account` filled in. [`crate::VaultContacts::sync_account`] then
//! reconciles the returned contacts against the vault (add / update /
//! skip by UID + ETag).
//!
//! The pull is a plain generic-CardDAV walk over HTTPS with Basic auth:
//!
//! - Nextcloud/generic: `/.well-known/carddav` redirect → PROPFIND
//!   `current-user-principal` → PROPFIND `addressbook-home-set` →
//!   PROPFIND Depth:1 the home for `resourcetype` = `addressbook`
//!   collections.
//! - iCloud: fixed root `https://contacts.icloud.com`, same walk, needs
//!   an app-specific password (Basic auth).
//! - If `account.addressbook_url` is already set, it's used directly and
//!   discovery is skipped.
//!
//! Each collection is drained with an `addressbook-query` REPORT
//! (Depth:1) requesting `getetag` + `address-data`; the multistatus is
//! parsed for (href, etag, vcard) tuples and each vCard is mapped by the
//! pure [`vcard_to_contact`] (unit-tested without network). Cards that
//! fail to parse are logged and skipped.
//!
//! # Where the credential may go
//!
//! The password is sent to exactly one place: the account's
//! **credential scope** ([`CredentialScope`]) — the configured origin,
//! over https. For an iCloud account that is Apple's own `icloud.com`
//! hosts on 443, because iCloud answers discovery on
//! `contacts.icloud.com` and serves the addressbook from a numbered
//! `pNN-contacts.icloud.com` partition. Everything else is refused
//! *before* a request is made:
//!
//! - an `http:` origin, at save ([`validate_account`]) and at sync;
//! - a redirect whose target is outside the scope — it is not followed
//!   at all, with or without the header;
//! - an href in a DAV response that points outside the scope.
//!
//! Redirects are followed by hand (the agent's own following is off),
//! so no library default can carry the `Authorization` header across.

use std::io::BufReader;
use std::sync::Arc;
use std::time::Duration;

use contacts_proto::{CardDavAccount, CardDavProvider, Contact, ContactSource, ContactsError};
use url::Url;

/// iCloud's fixed discovery root.
pub const ICLOUD_ROOT: &str = "https://contacts.icloud.com";

/// Pull + map every vCard in the account's addressbook.
///
/// One-way: this only *reads* the server. The caller reconciles the
/// returned contacts against the vault by `uid` / `etag`. `password`
/// is the resolved credential — the account itself never carries it.
pub fn import(
    account: &CardDavAccount,
    password: &str,
    transport: &Transport,
) -> Result<Vec<Contact>, ContactsError> {
    let dav = Dav::new(account, password, transport)?;
    let source = source_for_provider(&account.provider);

    // If the addressbook URL is already resolved, use it directly and
    // skip discovery. Otherwise walk principal → home → collections.
    let addressbooks = match account.addressbook_url.as_deref() {
        Some(url) if !url.trim().is_empty() => vec![dav.absolute(url.trim())?],
        _ => dav.discover_addressbooks()?,
    };

    if addressbooks.is_empty() {
        return Err(ContactsError::Sync {
            message: format!(
                "no addressbook collections found for '{}' ({})",
                account.label, account.provider
            ),
        });
    }

    let mut out = Vec::new();
    for book in &addressbooks {
        for card in dav.fetch_cards(book)? {
            match vcard_to_contact(&card.vcard, &card.etag, source, &account.label) {
                Some(mut c) => {
                    // UID is required for reconciliation; if the card has
                    // none, synthesize a stable one from its href.
                    if c.uid.as_deref().is_none_or(str::is_empty) {
                        c.uid = Some(format!("href:{}", card.href));
                    }
                    out.push(c);
                }
                None => {
                    tracing::warn!(href = %card.href, "skipping unparseable vCard");
                }
            }
        }
    }
    Ok(out)
}

/// The [`ContactSource`] constant that matches a provider.
fn source_for_provider(provider: &str) -> &'static str {
    match provider {
        CardDavProvider::NEXTCLOUD => ContactSource::NEXTCLOUD,
        CardDavProvider::ICLOUD => ContactSource::ICLOUD,
        _ => ContactSource::CARDDAV,
    }
}

// ── Transport policy ────────────────────────────────────────────────

/// How the CardDAV client reaches a server: which certificate
/// authorities it trusts beyond the public web roots.
///
/// The default trusts the public roots only. A self-hosted Nextcloud
/// behind a private CA adds that CA — on a server through
/// `TASK_CARDDAV_EXTRA_CA` (a PEM file), see [`Transport::from_env`].
/// The scheme is never negotiable: there is no plain-http transport.
#[derive(Clone, Default)]
pub struct Transport {
    extra_roots: Vec<rustls_pki_types::CertificateDer<'static>>,
}

impl std::fmt::Debug for Transport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transport")
            .field("extra_roots", &self.extra_roots.len())
            .finish()
    }
}

impl Transport {
    /// Trust the certificates in `pem` as additional roots.
    ///
    /// # Errors
    ///
    /// The PEM holds no certificate, or one that does not parse.
    pub fn with_extra_ca_pem(pem: &[u8]) -> Result<Self, ContactsError> {
        let roots: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(pem))
            .collect::<Result<_, _>>()
            .map_err(|e| ContactsError::Backend {
                message: format!("reading the extra CardDAV CA: {e}"),
            })?;
        if roots.is_empty() {
            return Err(ContactsError::Backend {
                message: "the extra CardDAV CA file holds no certificate".into(),
            });
        }
        Ok(Self { extra_roots: roots })
    }

    /// `TASK_CARDDAV_EXTRA_CA` (a PEM file path), or the public roots
    /// only. A file that cannot be read is logged and ignored — the
    /// public roots still apply, so the failure is a refused handshake
    /// against the private server, never a weaker check.
    #[must_use]
    pub fn from_env() -> Self {
        let Some(path) = std::env::var_os("TASK_CARDDAV_EXTRA_CA") else {
            return Self::default();
        };
        match std::fs::read(&path)
            .map_err(|e| ContactsError::Backend {
                message: e.to_string(),
            })
            .and_then(|pem| Self::with_extra_ca_pem(&pem))
        {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(?path, %e, "contacts: TASK_CARDDAV_EXTRA_CA ignored");
                Self::default()
            }
        }
    }

    fn agent(&self) -> Result<ureq::Agent, ContactsError> {
        let mut builder = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            // Never let the agent follow a redirect: `Dav::send` decides
            // each hop against the credential scope.
            .redirects(0);
        if !self.extra_roots.is_empty() {
            let mut roots = rustls::RootCertStore {
                roots: webpki_roots::TLS_SERVER_ROOTS.to_vec(),
            };
            for cert in &self.extra_roots {
                roots
                    .add(cert.clone())
                    .map_err(|e| ContactsError::Backend {
                        message: format!("the extra CardDAV CA is not a usable root: {e}"),
                    })?;
            }
            let config = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls::crypto::ring::default_provider(),
            ))
            .with_safe_default_protocol_versions()
            .map_err(|e| ContactsError::Backend {
                message: format!("tls config: {e}"),
            })?
            .with_root_certificates(roots)
            .with_no_client_auth();
            builder = builder.tls_config(Arc::new(config));
        }
        Ok(builder.build())
    }
}

/// Where an account's credential may be sent. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CredentialScope {
    host: String,
    port: u16,
    /// iCloud: any `icloud.com` host on 443 too.
    icloud: bool,
}

impl CredentialScope {
    /// Does `url` fall inside the scope — https, no userinfo, and the
    /// configured origin (or, for iCloud, an `icloud.com` host)?
    #[must_use]
    pub fn allows(&self, url: &Url) -> bool {
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            return false;
        }
        let Some(host) = url.host_str() else {
            return false;
        };
        let port = url.port_or_known_default().unwrap_or(0);
        if host.eq_ignore_ascii_case(&self.host) && port == self.port {
            return true;
        }
        self.icloud && port == 443 && is_icloud_host(host)
    }
}

fn is_icloud_host(host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    host == "icloud.com" || host.ends_with(".icloud.com")
}

/// Parse `raw` as an https URL with a host and no userinfo.
fn https_url(field: &str, raw: &str) -> Result<Url, ContactsError> {
    let invalid = |reason: String| ContactsError::Invalid {
        field: field.into(),
        reason,
    };
    let url = Url::parse(raw.trim()).map_err(|e| invalid(format!("not a URL ({e})")))?;
    if url.scheme() != "https" {
        return Err(invalid(format!(
            "only https is allowed; refusing the '{}' scheme, which would send the password \
             in the clear",
            url.scheme()
        )));
    }
    if url.host_str().is_none_or(str::is_empty) {
        return Err(invalid("the URL has no host".into()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(invalid(
            "credentials do not belong in the URL; use the username and password fields".into(),
        ));
    }
    Ok(url)
}

/// The base URL an account syncs from: its server URL, or iCloud's
/// fixed root when an iCloud account leaves it blank.
fn base_url(account: &CardDavAccount) -> Result<Url, ContactsError> {
    let configured = account.server_url.trim();
    if configured.is_empty() {
        if account.provider == CardDavProvider::ICLOUD {
            return https_url("account.server_url", ICLOUD_ROOT);
        }
        return Err(ContactsError::Invalid {
            field: "account.server_url".into(),
            reason: format!("account '{}' has no server URL", account.label),
        });
    }
    https_url("account.server_url", configured)
}

/// The credential scope of an account (after [`validate_account`]).
///
/// # Errors
///
/// As [`validate_account`].
pub fn credential_scope(account: &CardDavAccount) -> Result<CredentialScope, ContactsError> {
    let base = base_url(account)?;
    Ok(CredentialScope {
        host: base.host_str().unwrap_or_default().to_ascii_lowercase(),
        port: base.port_or_known_default().unwrap_or(443),
        icloud: account.provider == CardDavProvider::ICLOUD,
    })
}

// t[impl contacts.transport.https-only]
/// Check an account's URLs before it is saved — and again before every
/// sync, so a file edited by hand gets the same answer.
///
/// # Errors
///
/// [`ContactsError::Invalid`] naming the field: a non-https or
/// malformed server URL, a missing one (iCloud excepted), or an
/// addressbook URL outside the account's credential scope.
pub fn validate_account(account: &CardDavAccount) -> Result<(), ContactsError> {
    let scope = credential_scope(account)?;
    if let Some(book) = account
        .addressbook_url
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let url = https_url("account.addressbook_url", book)?;
        if !scope.allows(&url) {
            return Err(ContactsError::Invalid {
                field: "account.addressbook_url".into(),
                reason: "the addressbook must be on the account's own server".into(),
            });
        }
    }
    Ok(())
}

// ── DAV client ──────────────────────────────────────────────────────

/// A raw (href, etag, vcard-text) tuple from a multistatus response.
struct Card {
    href: String,
    etag: String,
    vcard: String,
}

/// A minimal blocking WebDAV/CardDAV client scoped to one account.
struct Dav {
    agent: ureq::Agent,
    /// The base URL discovery starts from; relative hrefs resolve
    /// against it.
    base: Url,
    /// Where the credential may be sent.
    scope: CredentialScope,
    /// Pre-computed `Basic …` authorization header value.
    auth: zeroize::Zeroizing<String>,
}

impl Dav {
    fn new(
        account: &CardDavAccount,
        password: &str,
        transport: &Transport,
    ) -> Result<Self, ContactsError> {
        validate_account(account).map_err(|e| ContactsError::Sync {
            message: e.to_string(),
        })?;
        let base = base_url(account)?;
        let scope = credential_scope(account)?;
        let auth = zeroize::Zeroizing::new(format!(
            "Basic {}",
            base64_encode(format!("{}:{}", account.username, password).as_bytes())
        ));
        Ok(Self {
            agent: transport.agent()?,
            base,
            scope,
            auth,
        })
    }

    /// Resolve a (possibly relative) href against the base URL, and
    /// refuse it if it leaves the credential scope.
    fn absolute(&self, href: &str) -> Result<Url, ContactsError> {
        let url = self
            .base
            .join(href.trim())
            .map_err(|e| ContactsError::Sync {
                message: format!("bad href {href:?} from the server: {e}"),
            })?;
        if !self.scope.allows(&url) {
            return Err(ContactsError::Sync {
                message: format!(
                    "refusing {}: outside this account's server, so its password is not sent there",
                    redact(&url)
                ),
            });
        }
        Ok(url)
    }

    // t[impl contacts.transport.same-origin-credentials]
    /// Issue a request with a body, returning the response body text.
    /// Redirects are followed only while they stay inside the
    /// credential scope; one that leaves it is an error and is never
    /// requested. Non-2xx → `Sync` error.
    fn send(
        &self,
        method: &str,
        url: &Url,
        depth: &str,
        body: &str,
    ) -> Result<String, ContactsError> {
        let mut target = url.clone();
        for _ in 0..5 {
            // The one gate every request passes, redirect hops included.
            if !self.scope.allows(&target) {
                return Err(ContactsError::Sync {
                    message: format!(
                        "refusing {method} {}: outside this account's server",
                        redact(&target)
                    ),
                });
            }
            let resp = self
                .agent
                .request(method, target.as_str())
                .set("Authorization", &self.auth)
                .set("Depth", depth)
                .set("Content-Type", "application/xml; charset=utf-8")
                .send_string(body);
            let (code, location, ok_body) = match resp {
                Ok(r) if (300..400).contains(&r.status()) => {
                    (r.status(), r.header("Location").map(str::to_owned), None)
                }
                Ok(r) => (r.status(), None, Some(r)),
                Err(ureq::Error::Status(code, r)) => {
                    (code, r.header("Location").map(str::to_owned), None)
                }
                Err(e) => {
                    return Err(ContactsError::Sync {
                        message: format!("{method} {}: {e}", redact(&target)),
                    });
                }
            };
            if let Some(r) = ok_body {
                return r.into_string().map_err(|e| ContactsError::Sync {
                    message: format!("reading {method} {}: {e}", redact(&target)),
                });
            }
            if matches!(code, 301 | 302 | 303 | 307 | 308)
                && let Some(loc) = location
            {
                let next = target.join(loc.trim()).map_err(|e| ContactsError::Sync {
                    message: format!("bad redirect {loc:?}: {e}"),
                })?;
                if !self.scope.allows(&next) {
                    return Err(ContactsError::Sync {
                        message: format!(
                            "refusing a redirect from {} to {}: outside this account's server",
                            redact(&target),
                            redact(&next)
                        ),
                    });
                }
                target = next;
                continue;
            }
            return Err(ContactsError::Sync {
                message: format!("{method} {} → HTTP {code}", redact(&target)),
            });
        }
        Err(ContactsError::Sync {
            message: format!("too many redirects for {method} {}", redact(url)),
        })
    }

    /// Walk principal → addressbook-home-set → addressbook collections.
    fn discover_addressbooks(&self) -> Result<Vec<Url>, ContactsError> {
        // current-user-principal
        let principal_body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;
        let well_known = self.absolute("/.well-known/carddav")?;
        let xml = self
            .send("PROPFIND", &well_known, "0", principal_body)
            .or_else(|_| self.send("PROPFIND", &self.base, "0", principal_body))?;
        let principal =
            first_href_for(&xml, "current-user-principal").ok_or_else(|| ContactsError::Sync {
                message: "no current-user-principal in server response".into(),
            })?;

        // addressbook-home-set
        let home_body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav"><d:prop><c:addressbook-home-set/></d:prop></d:propfind>"#;
        let xml = self.send("PROPFIND", &self.absolute(&principal)?, "0", home_body)?;
        let home =
            first_href_for(&xml, "addressbook-home-set").ok_or_else(|| ContactsError::Sync {
                message: "no addressbook-home-set in server response".into(),
            })?;

        // list collections; keep the ones whose resourcetype is addressbook
        let list_body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:displayname/></d:prop></d:propfind>"#;
        let home = self.absolute(&home)?;
        let xml = self.send("PROPFIND", &home, "1", list_body)?;
        addressbook_hrefs(&xml)
            .into_iter()
            .map(|h| {
                home.join(&h).map_err(|e| ContactsError::Sync {
                    message: format!("bad addressbook href {h:?}: {e}"),
                })
            })
            .collect::<Result<Vec<_>, _>>()
    }

    /// Drain one addressbook collection via `addressbook-query` REPORT.
    fn fetch_cards(&self, addressbook_url: &Url) -> Result<Vec<Card>, ContactsError> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<c:addressbook-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav">
  <d:prop><d:getetag/><c:address-data/></d:prop>
</c:addressbook-query>"#;
        let xml = self.send("REPORT", addressbook_url, "1", body)?;
        Ok(parse_multistatus(&xml))
    }
}

/// A URL fit for an error message or a log line: scheme, host, port and
/// path — never userinfo or a query.
fn redact(url: &Url) -> String {
    format!(
        "{}://{}{}{}",
        url.scheme(),
        url.host_str().unwrap_or(""),
        url.port().map(|p| format!(":{p}")).unwrap_or_default(),
        url.path()
    )
}

// ── XML helpers (roxmltree; matched by local name, namespace-lenient) ─

/// Find the first `<href>` nested under the first element whose local
/// name is `container` (e.g. `current-user-principal`).
fn first_href_for(xml: &str, container: &str) -> Option<String> {
    let doc = roxmltree::Document::parse(xml).ok()?;
    doc.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == container)
        .and_then(|n| {
            n.descendants()
                .find(|c| c.is_element() && c.tag_name().name() == "href")
                .and_then(|h| h.text())
                .map(|s| s.trim().to_string())
        })
}

/// Hrefs of every `response` whose `resourcetype` contains `addressbook`.
fn addressbook_hrefs(xml: &str) -> Vec<String> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for resp in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "response")
    {
        let is_book = resp
            .descendants()
            .any(|n| n.is_element() && n.tag_name().name() == "addressbook");
        if !is_book {
            continue;
        }
        if let Some(href) = resp
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "href")
            .and_then(|h| h.text())
        {
            out.push(href.trim().to_string());
        }
    }
    out
}

/// Parse a multistatus into (href, etag, vcard) tuples, keeping only
/// responses that actually carry `address-data`.
fn parse_multistatus(xml: &str) -> Vec<Card> {
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for resp in doc
        .descendants()
        .filter(|n| n.is_element() && n.tag_name().name() == "response")
    {
        let href = resp
            .children()
            .find(|c| c.is_element() && c.tag_name().name() == "href")
            .and_then(|h| h.text())
            .unwrap_or_default()
            .trim()
            .to_string();
        let etag = resp
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "getetag")
            .and_then(|n| n.text())
            .unwrap_or_default()
            .trim()
            .trim_matches('"')
            .to_string();
        let vcard = resp
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "address-data")
            .and_then(|n| n.text())
            .map(str::trim)
            .unwrap_or_default()
            .to_string();
        if !vcard.is_empty() {
            out.push(Card { href, etag, vcard });
        }
    }
    out
}

// ── vCard → Contact (pure; unit-tested without network) ─────────────

/// Map one vCard (3.0 or 4.0) to a [`Contact`], stamping `etag`,
/// `source`, and `account`. Returns `None` if the text isn't a parseable
/// vCard. `id` is a fresh uuid and `created` is an RFC-3339 now stamp —
/// the vault-file identity the caller reconciles against.
#[must_use]
pub fn vcard_to_contact(vcard: &str, etag: &str, source: &str, account: &str) -> Option<Contact> {
    let card = ical::VcardParser::new(BufReader::new(vcard.as_bytes()))
        .next()?
        .ok()?;
    let props = &card.properties;

    // First non-empty value for a property name (case-insensitive).
    let first = |name: &str| -> Option<String> {
        props
            .iter()
            .find(|p| p.name.eq_ignore_ascii_case(name))
            .and_then(|p| p.value.clone())
            .map(|v| unescape(&v))
            .filter(|v| !v.is_empty())
    };
    // All non-empty values for a property name.
    let all = |name: &str| -> Vec<String> {
        props
            .iter()
            .filter(|p| p.name.eq_ignore_ascii_case(name))
            .filter_map(|p| p.value.clone())
            .map(|v| unescape(&v))
            .filter(|v| !v.is_empty())
            .collect()
    };

    // FN is required by the spec; fall back to N or ORG so we never drop
    // a real card over a missing display name.
    let full_name = first("FN")
        .or_else(|| first("N").map(|n| n_to_display(&n)))
        .or_else(|| first("ORG").map(|o| o.split(';').next().unwrap_or("").trim().to_string()))
        .filter(|s| !s.is_empty())?;

    let id = uuid::Uuid::new_v4().to_string();
    let created = chrono::Utc::now().to_rfc3339();
    let mut c = Contact::create(id, full_name, created);

    c.uid = first("UID");
    c.etag = (!etag.trim().is_empty()).then(|| etag.trim().trim_matches('"').to_string());
    c.source = source.to_string();
    c.account = Some(account.to_string());

    // N = Family;Given;Additional;Prefixes;Suffixes
    if let Some(n) = first("N") {
        let parts: Vec<&str> = n.split(';').collect();
        c.family_name = parts
            .first()
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        c.given_name = parts
            .get(1)
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    // ORG = Company;Unit;… → first component.
    c.organization = first("ORG")
        .map(|o| o.split(';').next().unwrap_or("").trim().to_string())
        .filter(|s| !s.is_empty());
    c.title = first("TITLE");
    c.notes = first("NOTE");

    let emails = all("EMAIL");
    if !emails.is_empty() {
        c.emails = emails.join("\n");
    }
    let phones = all("TEL");
    if !phones.is_empty() {
        c.phones = phones.join("\n");
    }
    // CATEGORIES is comma-separated within one property.
    let groups: Vec<String> = all("CATEGORIES")
        .iter()
        .flat_map(|v| v.split(',').map(|s| s.trim().to_string()))
        .filter(|s| !s.is_empty())
        .collect();
    if !groups.is_empty() {
        c.groups = groups.join("\n");
    }

    // ADR = pobox;ext;street;locality;region;postcode;country → the
    // non-empty components, one per line.
    if let Some(adr) = first("ADR") {
        let lines: Vec<String> = adr
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();
        if !lines.is_empty() {
            c.address = Some(lines.join("\n"));
        }
    }

    c.birthday = first("BDAY").map(|b| normalize_date(&b));

    // PHOTO: keep only real URIs (http/https); skip embedded base64 and
    // data: URIs.
    if let Some(photo) = first("PHOTO") {
        if photo.starts_with("http://") || photo.starts_with("https://") {
            c.photo_url = Some(photo);
        }
    }

    Some(c)
}

/// Build a display name from an `N` value when `FN` is missing.
fn n_to_display(n: &str) -> String {
    let parts: Vec<&str> = n.split(';').map(str::trim).collect();
    let family = parts.first().copied().unwrap_or("");
    let given = parts.get(1).copied().unwrap_or("");
    format!("{given} {family}").trim().to_string()
}

/// Best-effort ISO `YYYY-MM-DD`. Accepts `19800501` → `1980-05-01`;
/// otherwise passes the value through (already ISO, or a partial date).
fn normalize_date(bday: &str) -> String {
    let s = bday.trim();
    let digits: String = s.chars().filter(char::is_ascii_digit).collect();
    if s.chars().all(|c| c.is_ascii_digit()) && digits.len() == 8 {
        format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8])
    } else {
        s.to_string()
    }
}

/// Unescape the vCard text-value escapes (`\n` `\,` `\;` `\\`).
fn unescape(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    let mut chars = v.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(',') => out.push(','),
                Some(';') => out.push(';'),
                Some('\\') => out.push('\\'),
                Some(other) => out.push(other),
                None => out.push('\\'),
            }
        } else {
            out.push(ch);
        }
    }
    out.trim().to_string()
}

/// Minimal standard base64 encoder (avoids pulling in a crate just for
/// the Basic-auth header).
fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[(n >> 18 & 63) as usize] as char);
        out.push(TABLE[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            TABLE[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            TABLE[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const VCARD_3: &str = "BEGIN:VCARD\r\n\
VERSION:3.0\r\n\
UID:urn:uuid:aaaa-3\r\n\
FN:Ada Lovelace\r\n\
N:Lovelace;Ada;;;\r\n\
ORG:Analytical Engines;R&D\r\n\
TITLE:Mathematician\r\n\
EMAIL;TYPE=INTERNET:ada@example.com\r\n\
EMAIL;TYPE=WORK:ada.l@work.com\r\n\
TEL;TYPE=CELL:+1 555 0100\r\n\
ADR;TYPE=HOME:;;12 Analytical Way;London;;EC1;UK\r\n\
BDAY:18151210\r\n\
CATEGORIES:Engineering,Friends\r\n\
NOTE:First programmer.\\nLoved by all.\r\n\
PHOTO;VALUE=URI:https://example.com/ada.jpg\r\n\
END:VCARD\r\n";

    const VCARD_4: &str = "BEGIN:VCARD\r\n\
VERSION:4.0\r\n\
UID:grace-4\r\n\
FN:Grace Hopper\r\n\
N:Hopper;Grace;;;\r\n\
EMAIL:grace@navy.mil\r\n\
TEL:+1 555 0199\r\n\
BDAY:1906-12-09\r\n\
NOTE:Compiler pioneer.\r\n\
END:VCARD\r\n";

    #[test]
    fn maps_vcard_3() {
        let c = vcard_to_contact(
            VCARD_3,
            "\"etag-abc\"",
            ContactSource::NEXTCLOUD,
            "Personal",
        )
        .unwrap();
        assert_eq!(c.full_name, "Ada Lovelace");
        assert_eq!(c.uid.as_deref(), Some("urn:uuid:aaaa-3"));
        assert_eq!(c.etag.as_deref(), Some("etag-abc"));
        assert_eq!(c.source, ContactSource::NEXTCLOUD);
        assert_eq!(c.account.as_deref(), Some("Personal"));
        assert_eq!(c.family_name.as_deref(), Some("Lovelace"));
        assert_eq!(c.given_name.as_deref(), Some("Ada"));
        assert_eq!(c.organization.as_deref(), Some("Analytical Engines"));
        assert_eq!(c.title.as_deref(), Some("Mathematician"));
        assert_eq!(c.email_list(), vec!["ada@example.com", "ada.l@work.com"]);
        assert_eq!(c.phone_list(), vec!["+1 555 0100"]);
        assert_eq!(c.group_list(), vec!["Engineering", "Friends"]);
        assert_eq!(
            c.address.as_deref(),
            Some("12 Analytical Way\nLondon\nEC1\nUK")
        );
        assert_eq!(c.birthday.as_deref(), Some("1815-12-10"));
        assert_eq!(c.notes.as_deref(), Some("First programmer.\nLoved by all."));
        assert_eq!(c.photo_url.as_deref(), Some("https://example.com/ada.jpg"));
        // fresh uuid + rfc3339 timestamp
        assert_eq!(c.id.len(), 36);
        assert!(c.created.contains('T'));
    }

    #[test]
    fn maps_vcard_4() {
        let c = vcard_to_contact(VCARD_4, "w/\"xyz\"", ContactSource::ICLOUD, "iCloud").unwrap();
        assert_eq!(c.full_name, "Grace Hopper");
        assert_eq!(c.uid.as_deref(), Some("grace-4"));
        assert_eq!(c.source, ContactSource::ICLOUD);
        assert_eq!(c.given_name.as_deref(), Some("Grace"));
        assert_eq!(c.family_name.as_deref(), Some("Hopper"));
        assert_eq!(c.primary_email(), Some("grace@navy.mil"));
        assert_eq!(c.birthday.as_deref(), Some("1906-12-09"));
        assert!(c.photo_url.is_none());
    }

    #[test]
    fn rejects_non_vcard() {
        assert!(vcard_to_contact("not a vcard", "", ContactSource::CARDDAV, "x").is_none());
    }

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64_encode(b"user:pass"), "dXNlcjpwYXNz");
        assert_eq!(
            base64_encode(b"Aladdin:open sesame"),
            "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="
        );
        assert_eq!(base64_encode(b""), "");
    }

    fn account(provider: &str, server_url: &str) -> CardDavAccount {
        let mut a = CardDavAccount::create("a1", "Test", provider, "2026-10-20T00:00:00Z");
        a.server_url = server_url.into();
        a
    }

    // t[verify contacts.transport.https-only]
    #[test]
    fn only_https_origins_are_accepted() {
        assert!(validate_account(&account("nextcloud", "https://cloud.example.com/dav")).is_ok());
        for refused in [
            "http://cloud.example.com/dav",
            "ftp://cloud.example.com",
            "cloud.example.com",
            "https://user:pw@cloud.example.com",
            "",
        ] {
            let err = validate_account(&account("nextcloud", refused)).unwrap_err();
            assert!(
                matches!(&err, ContactsError::Invalid { field, .. } if field == "account.server_url"),
                "{refused:?} → {err:?}"
            );
        }
        // iCloud may leave the URL blank: its root is fixed, and https.
        assert!(validate_account(&account("icloud", "")).is_ok());
        // An addressbook on another host is refused too.
        let mut a = account("nextcloud", "https://cloud.example.com");
        a.addressbook_url = Some("https://elsewhere.example.net/books/1/".into());
        assert!(validate_account(&a).is_err());
        a.addressbook_url = Some("https://cloud.example.com/books/1/".into());
        assert!(validate_account(&a).is_ok());
    }

    // t[verify contacts.transport.same-origin-credentials]
    #[test]
    fn the_credential_scope_is_the_configured_origin() {
        let scope =
            credential_scope(&account("nextcloud", "https://cloud.example.com:8443/dav")).unwrap();
        let ok = |s: &str| scope.allows(&Url::parse(s).unwrap());
        assert!(ok("https://cloud.example.com:8443/remote.php/dav/"));
        assert!(ok("https://CLOUD.example.com:8443/x"));
        assert!(
            !ok("https://cloud.example.com/x"),
            "another port is another origin"
        );
        assert!(
            !ok("http://cloud.example.com:8443/x"),
            "http is never in scope"
        );
        assert!(!ok("https://evil.example.com:8443/x"));
        assert!(!ok("https://cloud.example.com.evil.net:8443/x"));
        assert!(!ok("https://u@cloud.example.com:8443/x"));

        // iCloud: Apple's partition hosts, on 443, and nothing else.
        let icloud = credential_scope(&account("icloud", "")).unwrap();
        let ok = |s: &str| icloud.allows(&Url::parse(s).unwrap());
        assert!(ok("https://contacts.icloud.com/"));
        assert!(ok("https://p42-contacts.icloud.com/123/carddavhome/"));
        assert!(!ok("https://icloud.com.evil.net/"));
        assert!(!ok("https://p42-contacts.icloud.com:8443/"));
        assert!(!ok("http://p42-contacts.icloud.com/"));
    }

    #[test]
    fn parses_multistatus_tuples() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:response>
    <d:href>/addressbooks/user/default/card1.vcf</d:href>
    <d:propstat><d:prop>
      <d:getetag>"etag-1"</d:getetag>
      <card:address-data>BEGIN:VCARD&#13;
VERSION:3.0&#13;
FN:Test&#13;
END:VCARD</card:address-data>
    </d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        let cards = parse_multistatus(xml);
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].href, "/addressbooks/user/default/card1.vcf");
        assert_eq!(cards[0].etag, "etag-1");
        assert!(cards[0].vcard.contains("FN:Test"));
    }

    #[test]
    fn finds_addressbook_collections() {
        let xml = r#"<?xml version="1.0"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">
  <d:response>
    <d:href>/dav/user/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat>
  </d:response>
  <d:response>
    <d:href>/dav/user/contacts/</d:href>
    <d:propstat><d:prop><d:resourcetype><d:collection/><card:addressbook/></d:resourcetype></d:prop></d:propstat>
  </d:response>
</d:multistatus>"#;
        assert_eq!(addressbook_hrefs(xml), vec!["/dav/user/contacts/"]);
    }
}
