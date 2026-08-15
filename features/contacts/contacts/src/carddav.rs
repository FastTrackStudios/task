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

use std::io::BufReader;
use std::time::Duration;

use contacts_proto::{CardDavAccount, CardDavProvider, Contact, ContactSource, ContactsError};

/// Pull + map every vCard in the account's addressbook.
///
/// One-way: this only *reads* the server. The caller reconciles the
/// returned contacts against the vault by `uid` / `etag`.
pub fn import(account: &CardDavAccount) -> Result<Vec<Contact>, ContactsError> {
    let dav = Dav::new(account)?;
    let source = source_for_provider(&account.provider);

    // 2. If the addressbook URL is already resolved, use it directly and
    //    skip discovery. Otherwise walk principal → home → collections.
    let addressbooks = match account.addressbook_url.as_deref() {
        Some(url) if !url.trim().is_empty() => vec![dav.absolute(url.trim())],
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
                    if c.uid.as_deref().map_or(true, str::is_empty) {
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
    /// `scheme://host[:port]` — hrefs in DAV responses are usually paths
    /// and are resolved against this.
    origin: String,
    /// The base URL to start discovery from.
    base: String,
    /// Pre-computed `Basic …` authorization header value.
    auth: String,
}

impl Dav {
    fn new(account: &CardDavAccount) -> Result<Self, ContactsError> {
        // iCloud has a fixed discovery root; everything else uses the
        // configured server URL.
        let base = {
            let configured = account.server_url.trim();
            if account.provider == CardDavProvider::ICLOUD && configured.is_empty() {
                "https://contacts.icloud.com".to_string()
            } else if configured.is_empty() {
                return Err(ContactsError::Sync {
                    message: format!("account '{}' has no server URL", account.label),
                });
            } else {
                configured.trim_end_matches('/').to_string()
            }
        };
        let origin = origin_of(&base).ok_or_else(|| ContactsError::Sync {
            message: format!("malformed server URL: {base}"),
        })?;
        let agent = ureq::AgentBuilder::new()
            .timeout(Duration::from_secs(30))
            .redirects(0) // we follow the well-known redirect by hand
            .build();
        let auth = format!(
            "Basic {}",
            base64_encode(format!("{}:{}", account.username, account.password).as_bytes())
        );
        Ok(Self {
            agent,
            origin,
            base,
            auth,
        })
    }

    /// Resolve a (possibly relative) href against this account's origin.
    fn absolute(&self, href: &str) -> String {
        let h = href.trim();
        if h.starts_with("http://") || h.starts_with("https://") {
            h.to_string()
        } else if h.starts_with('/') {
            format!("{}{}", self.origin, h)
        } else {
            format!("{}/{}", self.origin, h)
        }
    }

    /// Issue a request with a body, returning the response body text. A
    /// single redirect (well-known) is followed. Non-2xx → `Sync` error.
    fn send(
        &self,
        method: &str,
        url: &str,
        depth: &str,
        body: &str,
    ) -> Result<String, ContactsError> {
        let mut target = url.to_string();
        for _ in 0..4 {
            let resp = self
                .agent
                .request(method, &target)
                .set("Authorization", &self.auth)
                .set("Depth", depth)
                .set("Content-Type", "application/xml; charset=utf-8")
                .send_string(body);
            match resp {
                Ok(r) => {
                    return r.into_string().map_err(|e| ContactsError::Sync {
                        message: format!("reading {method} {target}: {e}"),
                    });
                }
                Err(ureq::Error::Status(code, r)) => {
                    // Follow one hop for the well-known redirect.
                    if code == 301 || code == 302 || code == 307 || code == 308 {
                        if let Some(loc) = r.header("Location") {
                            target = self.absolute(loc);
                            continue;
                        }
                    }
                    return Err(ContactsError::Sync {
                        message: format!("{method} {target} → HTTP {code}"),
                    });
                }
                Err(e) => {
                    return Err(ContactsError::Sync {
                        message: format!("{method} {target}: {e}"),
                    });
                }
            }
        }
        Err(ContactsError::Sync {
            message: format!("too many redirects for {method} {url}"),
        })
    }

    /// Walk principal → addressbook-home-set → addressbook collections.
    fn discover_addressbooks(&self) -> Result<Vec<String>, ContactsError> {
        // current-user-principal
        let principal_body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;
        let well_known = self.absolute("/.well-known/carddav");
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
        let xml = self.send("PROPFIND", &self.absolute(&principal), "0", home_body)?;
        let home =
            first_href_for(&xml, "addressbook-home-set").ok_or_else(|| ContactsError::Sync {
                message: "no addressbook-home-set in server response".into(),
            })?;

        // list collections; keep the ones whose resourcetype is addressbook
        let list_body = r#"<?xml version="1.0" encoding="utf-8"?>
<d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/><d:displayname/></d:prop></d:propfind>"#;
        let xml = self.send("PROPFIND", &self.absolute(&home), "1", list_body)?;
        Ok(addressbook_hrefs(&xml)
            .into_iter()
            .map(|h| self.absolute(&h))
            .collect())
    }

    /// Drain one addressbook collection via `addressbook-query` REPORT.
    fn fetch_cards(&self, addressbook_url: &str) -> Result<Vec<Card>, ContactsError> {
        let body = r#"<?xml version="1.0" encoding="utf-8"?>
<c:addressbook-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav">
  <d:prop><d:getetag/><c:address-data/></d:prop>
</c:addressbook-query>"#;
        let xml = self.send("REPORT", addressbook_url, "1", body)?;
        Ok(parse_multistatus(&xml))
    }
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

/// The scheme + host + optional port of a URL, or `None` if malformed.
fn origin_of(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme.is_empty() || rest.is_empty() {
        return None;
    }
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if authority.is_empty() {
        return None;
    }
    Some(format!("{scheme}://{authority}"))
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

    #[test]
    fn origin_extraction() {
        assert_eq!(
            origin_of("https://cloud.example.com/remote.php/dav").as_deref(),
            Some("https://cloud.example.com")
        );
        assert_eq!(
            origin_of("https://host:8443/x").as_deref(),
            Some("https://host:8443")
        );
        assert_eq!(origin_of("not a url"), None);
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
