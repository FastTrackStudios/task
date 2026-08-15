//! mail-parser bridge — same shape as `email-maildir::parse`.
//! Lives here separately for now; once we add a third backend
//! we'll factor it into a shared `email-mime` crate.

use email_proto::{Addr, AttachmentMeta, EmailSyncError, Envelope, Message};
use mail_parser::{Address, MessageParser, MimeHeaders, PartType};

pub fn envelope_from_bytes(
    bytes: &[u8],
    folder: &str,
    flags: Vec<String>,
    message_id_override: Option<String>,
    size: u64,
) -> Result<Envelope, EmailSyncError> {
    let parsed = MessageParser::default()
        .parse_headers(bytes)
        .ok_or_else(|| EmailSyncError::Parse("mail-parser refused headers".into()))?;

    let message_id = message_id_override
        .or_else(|| parsed.message_id().map(std::string::ToString::to_string))
        .unwrap_or_else(|| synth_message_id(bytes));

    let subject = parsed.subject().unwrap_or("").to_string();
    let from = collect_addrs(parsed.from());
    let to = collect_addrs(parsed.to());
    let cc = collect_addrs(parsed.cc());

    let date_ms = parsed
        .date()
        .map(mail_parser::DateTime::to_timestamp)
        .map_or(0, |secs| secs.saturating_mul(1000));

    let has_attachments = parsed
        .headers()
        .iter()
        .any(|h| h.name().eq_ignore_ascii_case("Content-Disposition"));

    let thread_id = parsed
        .in_reply_to()
        .as_text()
        .map(std::string::ToString::to_string)
        .or_else(|| {
            parsed
                .references()
                .as_text_list()
                .and_then(|list| list.first().map(std::string::ToString::to_string))
        });

    Ok(Envelope {
        message_id,
        folder: folder.to_string(),
        thread_id,
        subject,
        from,
        to,
        cc,
        date_ms,
        flags,
        has_attachments,
        size,
        snippet: None,
    })
}

pub fn message_from_bytes(
    bytes: &[u8],
    folder: &str,
    flags: Vec<String>,
    size: u64,
) -> Result<Message, EmailSyncError> {
    let parsed = MessageParser::default()
        .parse(bytes)
        .ok_or_else(|| EmailSyncError::Parse("mail-parser refused message".into()))?;

    let envelope = envelope_from_bytes(bytes, folder, flags, None, size)?;

    let body_text = parsed.body_text(0).map(std::borrow::Cow::into_owned);
    let body_html = parsed.body_html(0).map(std::borrow::Cow::into_owned);

    let mut attachments = Vec::new();
    for (idx, part) in parsed.parts.iter().enumerate() {
        let is_attachment = matches!(part.body, PartType::Binary(_) | PartType::InlineBinary(_))
            || part.attachment_name().is_some();
        if !is_attachment {
            continue;
        }
        let filename = part.attachment_name().map(std::string::ToString::to_string);
        let mime = part.content_type().map_or_else(
            || "application/octet-stream".into(),
            |ct| {
                let main = ct.ctype();
                let sub = ct.subtype().unwrap_or("octet-stream");
                format!("{main}/{sub}")
            },
        );
        let size = match &part.body {
            PartType::Binary(b) | PartType::InlineBinary(b) => b.len() as u64,
            PartType::Text(t) | PartType::Html(t) => t.len() as u64,
            _ => 0,
        };
        // `Content-ID` marks a part the HTML body references as
        // `cid:…` rather than one the client lists at the bottom.
        // Stored bare; the header's angle brackets are not part of the
        // identifier.
        let content_id = part
            .headers
            .iter()
            .find(|h| h.name().eq_ignore_ascii_case("content-id"))
            .and_then(|h| h.value().as_text())
            .map(|v| {
                v.trim()
                    .trim_start_matches('<')
                    .trim_end_matches('>')
                    .to_owned()
            })
            .filter(|v| !v.is_empty());
        attachments.push(AttachmentMeta {
            part: idx.to_string(),
            filename,
            mime,
            size,
            content_id,
        });
    }

    let mut references = Vec::new();
    if let Some(s) = parsed.in_reply_to().as_text() {
        references.push(s.to_string());
    }
    if let Some(list) = parsed.references().as_text_list() {
        references.extend(list.iter().map(std::string::ToString::to_string));
    }

    let cutoff = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 2)
        .or_else(|| bytes.windows(2).position(|w| w == b"\n\n").map(|i| i + 1))
        .unwrap_or_else(|| bytes.len().min(64 * 1024));
    let headers_raw = String::from_utf8_lossy(&bytes[..cutoff]).into_owned();

    Ok(Message {
        envelope,
        headers_raw,
        body_text,
        body_html,
        attachments,
        references,
    })
}

/// Reserved for the future attachment-fetch RPC; callers
/// will land alongside the inbox UI's attachment download
/// flow. Kept here so the parser side stays paired with
/// the rest of the parsing surface.
#[allow(dead_code)]
pub fn attachment_bytes(bytes: &[u8], part: &str) -> Result<Vec<u8>, EmailSyncError> {
    let idx: usize = part.parse().map_err(|_| EmailSyncError::NotFound)?;
    let parsed = MessageParser::default()
        .parse(bytes)
        .ok_or_else(|| EmailSyncError::Parse("mail-parser refused message".into()))?;
    let part = parsed.parts.get(idx).ok_or(EmailSyncError::NotFound)?;
    Ok(match &part.body {
        PartType::Binary(b) | PartType::InlineBinary(b) => b.to_vec(),
        PartType::Text(t) | PartType::Html(t) => t.as_bytes().to_vec(),
        _ => return Err(EmailSyncError::NotFound),
    })
}

fn collect_addrs(value: Option<&Address>) -> Vec<Addr> {
    let Some(addr) = value else {
        return Vec::new();
    };
    addr.iter()
        .map(|a| Addr {
            name: a.name().map(std::string::ToString::to_string),
            email: a.address().unwrap_or("").to_string(),
        })
        .collect()
}

fn synth_message_id(bytes: &[u8]) -> String {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    format!("<sha-{:016x}@local.imap>", hasher.finish())
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADERS: &[u8] = b"\
Message-ID: <a@example.com>\r\n\
From: Alice <alice@example.com>\r\n\
To: you@example.com\r\n\
Subject: Hello\r\n\
Date: Mon, 14 Nov 2023 12:00:00 +0000\r\n\
\r\n";

    #[test]
    fn envelope_parses_header_only_bytes() {
        let env = envelope_from_bytes(HEADERS, "INBOX", vec!["\\Seen".into()], None, 1024).unwrap();
        assert_eq!(env.subject, "Hello");
        assert_eq!(env.from[0].email, "alice@example.com");
        assert!(env.message_id.contains("a@example.com"));
        assert_eq!(env.folder, "INBOX");
        assert_eq!(env.flags, vec!["\\Seen".to_string()]);
        assert_eq!(env.size, 1024);
    }

    #[test]
    fn override_message_id_wins() {
        let env = envelope_from_bytes(HEADERS, "INBOX", Vec::new(), Some("<override>".into()), 0)
            .unwrap();
        assert_eq!(env.message_id, "<override>");
    }
}
