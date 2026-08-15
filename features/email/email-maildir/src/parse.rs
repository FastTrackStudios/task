//! mail-parser bridge — read on-disk message bytes into the
//! `email-proto` payload shapes. We use `mail-parser`'s
//! headers-only mode for envelope listings and the full parser
//! for `fetch_message`, both producing `email-proto` types so
//! the wire surface is exactly what callers see.

use email_proto::{Addr, AttachmentMeta, EmailSyncError, Envelope, Message};
use mail_parser::{Address, MessageParser, MimeHeaders, PartType};
use std::path::Path;

/// Parse one on-disk maildir file into an envelope.
/// `folder` + `path_id` are echoed onto the result; the
/// parser supplies everything else from the headers.
pub fn envelope_from_bytes(
    bytes: &[u8],
    folder: &str,
    flags: Vec<String>,
) -> Result<Envelope, EmailSyncError> {
    let parsed = MessageParser::default()
        .parse_headers(bytes)
        .ok_or_else(|| EmailSyncError::Parse("mail-parser refused headers".into()))?;

    let message_id = parsed
        .message_id()
        .map_or_else(|| synth_message_id(bytes), std::string::ToString::to_string);

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
        size: bytes.len() as u64,
        snippet: None,
    })
}

/// Full parse — envelope + bodies + attachment metadata.
pub fn message_from_bytes(
    bytes: &[u8],
    folder: &str,
    flags: Vec<String>,
) -> Result<Message, EmailSyncError> {
    let parsed = MessageParser::default()
        .parse(bytes)
        .ok_or_else(|| EmailSyncError::Parse("mail-parser refused message".into()))?;

    let envelope = envelope_from_bytes(bytes, folder, flags)?;

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
            .map(|v| v.trim().trim_start_matches('<').trim_end_matches('>').to_owned())
            .filter(|v| !v.is_empty());
        attachments.push(AttachmentMeta {
            part: idx.to_string(),
            filename,
            mime,
            size,
            content_id,
        });
    }

    let references = collect_references(&parsed);

    let headers_raw = headers_text(bytes);

    Ok(Message {
        envelope,
        headers_raw,
        body_text,
        body_html,
        attachments,
        references,
    })
}

/// Fetch the raw bytes for one MIME part by index, matching
/// the `part` ids `message_from_bytes` hands out.
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

fn collect_references(parsed: &mail_parser::Message<'_>) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(s) = parsed.in_reply_to().as_text() {
        out.push(s.to_string());
    }
    if let Some(list) = parsed.references().as_text_list() {
        out.extend(list.iter().map(std::string::ToString::to_string));
    }
    out
}

/// Synthesize a stable id for messages that lack a `Message-ID`
/// header — content hash, prefixed so it's distinguishable from
/// real RFC2822 ids.
fn synth_message_id(bytes: &[u8]) -> String {
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    hasher.write(bytes);
    format!("<sha-{:016x}@local.maildir>", hasher.finish())
}

/// Carve out the header block (everything up to the first
/// blank line) for the `headers_raw` field. Best-effort — if no
/// blank line is found, return the first 64KB so we don't ship
/// the whole body.
fn headers_text(bytes: &[u8]) -> String {
    let cutoff = bytes
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 2)
        .or_else(|| bytes.windows(2).position(|w| w == b"\n\n").map(|i| i + 1))
        .unwrap_or_else(|| bytes.len().min(64 * 1024));
    String::from_utf8_lossy(&bytes[..cutoff]).into_owned()
}

#[allow(dead_code)]
fn _path_is_safe(path: &Path) -> bool {
    !path
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir))
}
