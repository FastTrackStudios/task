//! Thunderbird autoconfig XML parser. Same schema is served by
//! ISPDB and by providers' own `autoconfig.<domain>` endpoints.
//!
//! We hand-roll a pull-parser pass instead of leaning on
//! `quick_xml::de` — the schema is small + we want to be
//! permissive about unknown fields (the spec is loose), and
//! `serde-xml` derivation gets tangled on the
//! `<incomingServer type="imap">` attribute discrimination.

use crate::{AuthMethod, AutoconfigResult, Error, Protocol, Server};
use email_config::TlsMode;
use quick_xml::Reader;
use quick_xml::events::Event;

pub fn parse(xml: &str) -> Result<AutoconfigResult, Error> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    let mut buf = Vec::new();
    let mut result = AutoconfigResult::default();
    let mut current: Option<PartialServer> = None;
    let mut current_tag: Option<String> = None;

    loop {
        match reader.read_event_into(&mut buf) {
            Err(e) => {
                return Err(Error::Xml(format!(
                    "at pos {}: {e}",
                    reader.buffer_position()
                )));
            }
            Ok(Event::Eof) => break,
            Ok(Event::Start(e)) => {
                let tag = std::str::from_utf8(e.name().as_ref())
                    .map_err(|err| Error::Xml(err.to_string()))?
                    .to_string();
                match tag.as_str() {
                    "incomingServer" | "outgoingServer" => {
                        let mut server_type = String::new();
                        for attr in e.attributes().flatten() {
                            if attr.key.as_ref() == b"type" {
                                server_type = String::from_utf8_lossy(&attr.value).into_owned();
                            }
                        }
                        current = Some(PartialServer::new(tag.clone(), server_type));
                    }
                    _ => {}
                }
                current_tag = Some(tag);
            }
            Ok(Event::Text(e)) => {
                let Some(server) = current.as_mut() else {
                    continue;
                };
                let Some(tag) = current_tag.as_deref() else {
                    continue;
                };
                // BytesText in quick-xml 0.40 doesn't expose
                // `unescape()` directly — convert via the
                // `&[u8]` view. Autoconfig payloads don't use
                // entity-encoded characters in practice (host
                // names + ports + auth keywords).
                let text = String::from_utf8_lossy(e.as_ref()).into_owned();
                server.field(tag, text);
            }
            Ok(Event::End(e)) => {
                let tag = std::str::from_utf8(e.name().as_ref())
                    .map_err(|err| Error::Xml(err.to_string()))?
                    .to_string();
                if matches!(tag.as_str(), "incomingServer" | "outgoingServer") {
                    if let Some(server) = current.take() {
                        if let Some(built) = server.build() {
                            match server.outer_tag.as_str() {
                                "incomingServer" => result.incoming.push(built),
                                "outgoingServer" => result.outgoing.push(built),
                                _ => {}
                            }
                        }
                    }
                }
                current_tag = None;
            }
            _ => {}
        }
        buf.clear();
    }

    Ok(result)
}

/// Mid-parse accumulator. Mozilla's schema has `<authentication>`
/// repeated for multi-method servers, so we collect into a `Vec`.
struct PartialServer {
    outer_tag: String,
    server_type: String,
    hostname: Option<String>,
    port: Option<u16>,
    socket_type: Option<String>,
    username: Option<String>,
    auth: Vec<AuthMethod>,
}

impl PartialServer {
    fn new(outer_tag: String, server_type: String) -> Self {
        Self {
            outer_tag,
            server_type,
            hostname: None,
            port: None,
            socket_type: None,
            username: None,
            auth: Vec::new(),
        }
    }

    fn field(&mut self, tag: &str, value: String) {
        match tag {
            "hostname" => self.hostname = Some(value),
            "port" => self.port = value.trim().parse().ok(),
            "socketType" => self.socket_type = Some(value),
            "username" => self.username = Some(value),
            "authentication" => {
                if let Some(m) = parse_auth(&value) {
                    self.auth.push(m);
                }
            }
            _ => {}
        }
    }

    fn build(&self) -> Option<Server> {
        let host = self.hostname.clone()?;
        let port = self.port?;
        let protocol = match self.server_type.as_str() {
            "imap" => Protocol::Imap,
            "smtp" => Protocol::Smtp,
            "jmap" => Protocol::Jmap,
            "pop3" => Protocol::Pop3,
            _ => return None,
        };
        let tls = match self.socket_type.as_deref() {
            Some("SSL") => TlsMode::Implicit,
            Some("STARTTLS") => TlsMode::Starttls,
            Some("plain") => TlsMode::None,
            _ => {
                // Fall back to port-based heuristics.
                match port {
                    993 | 465 => TlsMode::Implicit,
                    143 | 587 => TlsMode::Starttls,
                    _ => TlsMode::None,
                }
            }
        };
        Some(Server {
            protocol,
            host,
            port,
            tls,
            auth: self.auth.clone(),
            username: self.username.clone().unwrap_or_default(),
        })
    }
}

fn parse_auth(value: &str) -> Option<AuthMethod> {
    match value {
        "password-cleartext" | "plain" => Some(AuthMethod::PasswordCleartext),
        "password-encrypted" | "secure" => Some(AuthMethod::PasswordEncrypted),
        "OAuth2" => Some(AuthMethod::OAuth2),
        "NTLM" => Some(AuthMethod::Ntlm),
        "GSSAPI" => Some(AuthMethod::GssApi),
        "client-IP-address" => Some(AuthMethod::ClientIpAddress),
        "TLS-client-cert" => Some(AuthMethod::TlsClientCert),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FASTMAIL_FIXTURE: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<clientConfig version="1.1">
  <emailProvider id="fastmail.com">
    <domain>fastmail.com</domain>
    <displayName>Fastmail</displayName>
    <incomingServer type="imap">
      <hostname>imap.fastmail.com</hostname>
      <port>993</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
      <authentication>OAuth2</authentication>
    </incomingServer>
    <outgoingServer type="smtp">
      <hostname>smtp.fastmail.com</hostname>
      <port>465</port>
      <socketType>SSL</socketType>
      <username>%EMAILADDRESS%</username>
      <authentication>password-cleartext</authentication>
    </outgoingServer>
  </emailProvider>
</clientConfig>"#;

    #[test]
    fn parses_fastmail_fixture() {
        let r = parse(FASTMAIL_FIXTURE).unwrap();
        assert_eq!(r.incoming.len(), 1);
        assert_eq!(r.outgoing.len(), 1);

        let in_ = &r.incoming[0];
        assert_eq!(in_.protocol, Protocol::Imap);
        assert_eq!(in_.host, "imap.fastmail.com");
        assert_eq!(in_.port, 993);
        assert_eq!(in_.tls, TlsMode::Implicit);
        assert_eq!(in_.username, "%EMAILADDRESS%");
        assert!(in_.auth.contains(&AuthMethod::PasswordCleartext));
        assert!(in_.auth.contains(&AuthMethod::OAuth2));

        let out = &r.outgoing[0];
        assert_eq!(out.protocol, Protocol::Smtp);
        assert_eq!(out.host, "smtp.fastmail.com");
        assert_eq!(out.port, 465);
        assert_eq!(out.tls, TlsMode::Implicit);
    }

    #[test]
    fn falls_back_to_port_heuristic_when_socket_type_missing() {
        let xml = r#"<clientConfig><emailProvider id="x">
            <incomingServer type="imap">
              <hostname>h</hostname><port>143</port>
              <username>u</username>
              <authentication>password-cleartext</authentication>
            </incomingServer>
        </emailProvider></clientConfig>"#;
        let r = parse(xml).unwrap();
        assert_eq!(r.incoming[0].tls, TlsMode::Starttls);
    }

    #[test]
    fn ignores_unknown_server_types() {
        let xml = r#"<clientConfig><emailProvider id="x">
            <incomingServer type="exchange">
              <hostname>h</hostname><port>443</port>
            </incomingServer>
        </emailProvider></clientConfig>"#;
        let r = parse(xml).unwrap();
        assert!(r.incoming.is_empty());
    }
}
