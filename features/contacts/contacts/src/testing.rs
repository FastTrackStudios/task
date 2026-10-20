//! A fake CardDAV server for tests — real TLS, real HTTP, two hosts.
//!
//! The CardDAV client is https-only, so a test that wants to watch it
//! work has to give it a server that speaks TLS. [`FakeCardDav`] does:
//! it binds `127.0.0.1` on a free port, presents the test-only leaf in
//! `testdata/` (signed by `testdata/ca.pem`), and records every request
//! it receives — method, path, and whether it carried an
//! `Authorization` header — so a test can say exactly where the
//! password went.
//!
//! Two of them are two origins: one answers as `https://localhost:<p>`,
//! the other as `https://127.0.0.1:<q>` ([`Host`]). Pointing the first's
//! redirects at the second is how the suite proves a redirect never
//! carries the credential across.
//!
//! Test-only. Compiled under `cfg(test)` and the `test-support`
//! feature; the server never builds it.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::carddav::Transport;

/// The test CA, PEM.
pub const CA_PEM: &[u8] = include_bytes!("../testdata/ca.pem");
const SERVER_CERT_PEM: &[u8] = include_bytes!("../testdata/server.pem");
const SERVER_KEY_PEM: &[u8] = include_bytes!("../testdata/server.key");

/// Where [`CA_PEM`] sits on disk — what `TASK_CARDDAV_EXTRA_CA` points
/// at when a whole server has to trust the fake.
#[must_use]
pub fn ca_pem_path() -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/ca.pem"))
}

/// A transport that trusts the test CA.
///
/// # Panics
///
/// Never in practice: the CA is a committed fixture.
#[must_use]
pub fn trusting_transport() -> Transport {
    Transport::with_extra_ca_pem(CA_PEM).expect("the test CA parses")
}

/// Which name a fake answers as — two names, two origins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Host {
    Localhost,
    Loopback,
}

/// One request as the fake saw it.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// The `Authorization` header, verbatim, when there was one.
    pub authorization: Option<String>,
}

/// How a fake behaves.
#[derive(Debug, Clone, Default)]
pub struct Behaviour {
    /// `(username, password)` the fake accepts. `None` accepts nothing
    /// (every request is recorded and answered 401).
    pub credentials: Option<(String, String)>,
    /// Cards in the one addressbook: `(file name, etag, vCard)`.
    pub cards: Vec<(String, String, String)>,
    /// Answer `/.well-known/carddav` with a redirect here (absolute
    /// URL). `None` redirects to this server's own `/dav/`.
    pub well_known_to: Option<String>,
    /// Answer *every* request with a redirect to this base (absolute
    /// URL; the request path is appended).
    pub everything_to: Option<String>,
}

/// A running fake. Stops when dropped.
pub struct FakeCardDav {
    origin: String,
    port: u16,
    log: Arc<Mutex<Vec<Recorded>>>,
    behaviour: Arc<Mutex<Behaviour>>,
    stop: Arc<AtomicBool>,
}

impl FakeCardDav {
    /// Bind and serve.
    ///
    /// # Panics
    ///
    /// When it cannot bind a loopback port or load its own fixture.
    #[must_use]
    pub fn start(host: Host, behaviour: Behaviour) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind a loopback port");
        let port = listener.local_addr().expect("local addr").port();
        let origin = match host {
            Host::Localhost => format!("https://localhost:{port}"),
            Host::Loopback => format!("https://127.0.0.1:{port}"),
        };
        let config = Arc::new(server_config());
        let log = Arc::new(Mutex::new(Vec::new()));
        let behaviour = Arc::new(Mutex::new(behaviour));
        let stop = Arc::new(AtomicBool::new(false));
        {
            let (log, behaviour, stop, origin) =
                (log.clone(), behaviour.clone(), stop.clone(), origin.clone());
            std::thread::spawn(move || {
                for conn in listener.incoming() {
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    let Ok(tcp) = conn else { continue };
                    let (config, log, behaviour, origin) = (
                        config.clone(),
                        log.clone(),
                        behaviour.clone(),
                        origin.clone(),
                    );
                    std::thread::spawn(move || {
                        let _ = serve_one(tcp, config, &log, &behaviour, &origin);
                    });
                }
            });
        }
        Self {
            origin,
            port,
            log,
            behaviour,
            stop,
        }
    }

    /// `https://<host>:<port>` — the origin to configure an account with.
    #[must_use]
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// Every request received so far.
    #[must_use]
    pub fn requests(&self) -> Vec<Recorded> {
        self.log.lock().expect("log").clone()
    }

    /// How many of them carried an `Authorization` header.
    #[must_use]
    pub fn requests_with_authorization(&self) -> usize {
        self.requests()
            .iter()
            .filter(|r| r.authorization.is_some())
            .count()
    }

    /// Change behaviour mid-test.
    pub fn set_behaviour(&self, behaviour: Behaviour) {
        *self.behaviour.lock().expect("behaviour") = behaviour;
    }
}

impl Drop for FakeCardDav {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Unblock the accept loop.
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn server_config() -> rustls::ServerConfig {
    let certs = rustls_pemfile::certs(&mut BufReader::new(SERVER_CERT_PEM))
        .collect::<Result<Vec<_>, _>>()
        .expect("server cert");
    let key = rustls_pemfile::private_key(&mut BufReader::new(SERVER_KEY_PEM))
        .expect("server key")
        .expect("a key in the fixture");
    rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("protocol versions")
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .expect("server config")
}

fn serve_one(
    tcp: TcpStream,
    config: Arc<rustls::ServerConfig>,
    log: &Mutex<Vec<Recorded>>,
    behaviour: &Mutex<Behaviour>,
    origin: &str,
) -> std::io::Result<()> {
    tcp.set_read_timeout(Some(std::time::Duration::from_secs(10)))?;
    let conn = rustls::ServerConnection::new(config).map_err(std::io::Error::other)?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    let mut reader = BufReader::new(&mut tls);

    let mut request_line = String::new();
    reader.read_line(&mut request_line)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_string();
    let path = parts.next().unwrap_or_default().to_string();
    let mut authorization = None;
    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 || line == "\r\n" || line == "\n" {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("authorization") {
                authorization = Some(value);
            } else if name.eq_ignore_ascii_case("content-length") {
                content_length = value.parse().unwrap_or(0);
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body)?;
    drop(reader);

    log.lock().expect("log").push(Recorded {
        method: method.clone(),
        path: path.clone(),
        authorization: authorization.clone(),
    });
    let behaviour = behaviour.lock().expect("behaviour").clone();
    let (status, extra, body) = respond(&behaviour, origin, &method, &path, authorization);
    let reason = match status {
        200 => "OK",
        207 => "Multi-Status",
        301 => "Moved Permanently",
        401 => "Unauthorized",
        _ => "Not Found",
    };
    let head = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/xml; charset=utf-8\r\n\
         Content-Length: {}\r\nConnection: close\r\n{extra}\r\n",
        body.len()
    );
    tls.write_all(head.as_bytes())?;
    tls.write_all(body.as_bytes())?;
    tls.flush()?;
    tls.conn.send_close_notify();
    let _ = tls.flush();
    Ok(())
}

fn respond(
    b: &Behaviour,
    origin: &str,
    method: &str,
    path: &str,
    authorization: Option<String>,
) -> (u16, String, String) {
    if let Some(to) = &b.everything_to {
        return (301, format!("Location: {to}{path}\r\n"), String::new());
    }
    let authorised = match (&b.credentials, authorization) {
        (Some((user, pass)), Some(header)) => {
            header == format!("Basic {}", base64(format!("{user}:{pass}").as_bytes()))
        }
        _ => false,
    };
    if !authorised {
        return (
            401,
            "WWW-Authenticate: Basic realm=\"fake\"\r\n".into(),
            String::new(),
        );
    }
    let multistatus = |inner: String| {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<d:multistatus xmlns:d="DAV:" xmlns:card="urn:ietf:params:xml:ns:carddav">{inner}</d:multistatus>"#
        )
    };
    match (method, path) {
        ("PROPFIND", "/.well-known/carddav") => {
            let to = b
                .well_known_to
                .clone()
                .unwrap_or_else(|| format!("{origin}/dav/"));
            (301, format!("Location: {to}\r\n"), String::new())
        }
        ("PROPFIND", "/dav/" | "/dav") => (
            207,
            String::new(),
            multistatus(
                "<d:response><d:href>/dav/</d:href><d:propstat><d:prop>\
                 <d:current-user-principal><d:href>/dav/principals/user/</d:href></d:current-user-principal>\
                 </d:prop></d:propstat></d:response>"
                    .into(),
            ),
        ),
        ("PROPFIND", "/dav/principals/user/") => (
            207,
            String::new(),
            multistatus(
                "<d:response><d:href>/dav/principals/user/</d:href><d:propstat><d:prop>\
                 <card:addressbook-home-set><d:href>/dav/addressbooks/user/</d:href></card:addressbook-home-set>\
                 </d:prop></d:propstat></d:response>"
                    .into(),
            ),
        ),
        ("PROPFIND", "/dav/addressbooks/user/") => (
            207,
            String::new(),
            multistatus(
                "<d:response><d:href>/dav/addressbooks/user/</d:href><d:propstat><d:prop>\
                 <d:resourcetype><d:collection/></d:resourcetype></d:prop></d:propstat></d:response>\
                 <d:response><d:href>/dav/addressbooks/user/contacts/</d:href><d:propstat><d:prop>\
                 <d:resourcetype><d:collection/><card:addressbook/></d:resourcetype></d:prop></d:propstat></d:response>"
                    .into(),
            ),
        ),
        ("REPORT", "/dav/addressbooks/user/contacts/") => {
            let mut inner = String::new();
            for (name, etag, vcard) in &b.cards {
                inner.push_str(&format!(
                    "<d:response><d:href>/dav/addressbooks/user/contacts/{name}</d:href>\
                     <d:propstat><d:prop><d:getetag>\"{etag}\"</d:getetag>\
                     <card:address-data>{}</card:address-data></d:prop></d:propstat></d:response>",
                    xml_escape(vcard)
                ));
            }
            (207, String::new(), multistatus(inner))
        }
        _ => (404, String::new(), String::new()),
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('\r', "&#13;")
}

fn base64(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
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

/// A small vCard for the fake's addressbook.
#[must_use]
pub fn vcard(uid: &str, name: &str) -> String {
    format!("BEGIN:VCARD\r\nVERSION:3.0\r\nUID:{uid}\r\nFN:{name}\r\nEND:VCARD\r\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::VaultContacts;
    use crate::credentials::TestSecrets;
    use contacts_proto::{CardDavAccount, CardDavProvider, Contacts, ContactsError};

    fn behaviour(user: &str, pass: &str) -> Behaviour {
        Behaviour {
            credentials: Some((user.into(), pass.into())),
            cards: vec![
                (
                    "ada.vcf".into(),
                    "e1".into(),
                    vcard("uid-ada", "Ada Lovelace"),
                ),
                (
                    "grace.vcf".into(),
                    "e2".into(),
                    vcard("uid-grace", "Grace Hopper"),
                ),
            ],
            ..Behaviour::default()
        }
    }

    fn account(id: &str, origin: &str, pass: &str) -> CardDavAccount {
        let mut a = CardDavAccount::create(id, "Fake", CardDavProvider::NEXTCLOUD, "2026-10-20");
        a.server_url = format!("{origin}/dav/");
        a.username = "me".into();
        a.password = pass.into();
        a
    }

    fn open(root: &std::path::Path) -> VaultContacts {
        VaultContacts::with_secrets(root, TestSecrets::shared(), trusting_transport()).unwrap()
    }

    #[test]
    fn a_sync_pulls_over_tls_and_survives_a_reopen() {
        let fake = FakeCardDav::start(Host::Localhost, behaviour("me", "pw-sync-1"));
        let tmp = tempfile::tempdir().unwrap();
        let c = open(tmp.path());
        c.upsert_account(&account("s1", fake.origin(), "pw-sync-1"))
            .unwrap();
        let report = c.sync_account("s1".into()).unwrap();
        assert_eq!(report.added, 2, "{report:?}");

        // A restart: a new backend over the same disk, nothing in memory.
        drop(c);
        let c = open(tmp.path());
        let report = c.sync_account("s1".into()).unwrap();
        assert_eq!(report.skipped, 2, "{report:?}");
        // The same-origin well-known redirect was followed.
        assert!(
            fake.requests()
                .iter()
                .any(|r| r.path == "/.well-known/carddav")
        );
    }

    // t[verify contacts.transport.same-origin-credentials]
    #[test]
    fn a_redirect_to_another_host_never_carries_the_password() {
        let other = FakeCardDav::start(Host::Loopback, Behaviour::default());
        let fake = FakeCardDav::start(
            Host::Localhost,
            Behaviour {
                everything_to: Some(other.origin().to_string()),
                ..behaviour("me", "pw-redirect")
            },
        );
        let tmp = tempfile::tempdir().unwrap();
        let c = open(tmp.path());
        c.upsert_account(&account("r1", fake.origin(), "pw-redirect"))
            .unwrap();
        let err = c.sync_account("r1".into()).unwrap_err();
        assert!(matches!(err, ContactsError::Sync { .. }), "{err:?}");
        assert!(!fake.requests().is_empty(), "the sync never started");
        assert_eq!(other.requests_with_authorization(), 0);
        assert!(
            other.requests().is_empty(),
            "a cross-origin redirect was followed at all: {:?}",
            other.requests()
        );
    }

    #[test]
    fn a_well_known_redirect_elsewhere_falls_back_to_the_base() {
        let other = FakeCardDav::start(Host::Loopback, Behaviour::default());
        let fake = FakeCardDav::start(
            Host::Localhost,
            Behaviour {
                well_known_to: Some(format!("{}/dav/", other.origin())),
                ..behaviour("me", "pw-wk")
            },
        );
        let tmp = tempfile::tempdir().unwrap();
        let c = open(tmp.path());
        c.upsert_account(&account("w1", fake.origin(), "pw-wk"))
            .unwrap();
        let report = c.sync_account("w1".into()).unwrap();
        assert_eq!(report.added, 2);
        assert!(other.requests().is_empty());
    }

    #[test]
    fn an_untrusted_certificate_is_refused() {
        let fake = FakeCardDav::start(Host::Localhost, behaviour("me", "pw-tls"));
        let tmp = tempfile::tempdir().unwrap();
        let c =
            VaultContacts::with_secrets(tmp.path(), TestSecrets::shared(), Transport::default())
                .unwrap();
        c.upsert_account(&account("t1", fake.origin(), "pw-tls"))
            .unwrap();
        assert!(c.sync_account("t1".into()).is_err());
        assert_eq!(fake.requests_with_authorization(), 0);
    }

    // t[verify contacts.transport.https-only]
    #[test]
    fn an_http_origin_planted_by_hand_is_refused_at_sync() {
        let tmp = tempfile::tempdir().unwrap();
        let c = open(tmp.path());
        c.upsert_account(&account("h1", "https://localhost:1", "pw-h1"))
            .unwrap();
        // Somebody edits the file to http.
        let path = tmp
            .path()
            .join(crate::vault_contacts::ACCOUNTS_DIR)
            .join("h1.md");
        let raw = std::fs::read_to_string(&path).unwrap();
        std::fs::write(
            &path,
            raw.replace("https://localhost:1", "http://localhost:1"),
        )
        .unwrap();
        let err = c.sync_account("h1".into()).unwrap_err();
        assert!(
            matches!(&err, ContactsError::Sync { message } if message.contains("https")),
            "{err:?}"
        );
    }
}
