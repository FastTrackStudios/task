//! `Draft` → RFC5322 bytes via `mail-builder`. Pure data, no
//! IO — testable end-to-end with `mail-parser` round-tripping.

use email_proto::{Addr, Draft};
use mail_builder::MessageBuilder;
use mail_builder::headers::address::Address;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum BuildError {
    #[error("draft missing required field: {0}")]
    Missing(&'static str),
    #[error("mail-builder: {0}")]
    Builder(String),
}

/// Build the RFC5322 byte representation of a draft. Returns
/// the bytes + the generated Message-ID (host part is derived
/// from the `from` address's domain; the random part is a
/// timestamp-shuffled token from `mail-builder`).
pub fn build_message(draft: &Draft) -> Result<(Vec<u8>, String), BuildError> {
    if draft.from.email.is_empty() {
        return Err(BuildError::Missing("from"));
    }
    if draft.to.is_empty() && draft.cc.is_empty() && draft.bcc.is_empty() {
        return Err(BuildError::Missing("to/cc/bcc"));
    }

    let domain = draft
        .from
        .email
        .split_once('@')
        .map_or("localhost", |(_, d)| d);
    let message_id = generate_message_id(domain);

    let mut builder = MessageBuilder::new()
        .from(addr_to_pair(&draft.from))
        .to(addrs_to_list(&draft.to))
        .subject(draft.subject.clone())
        .message_id(message_id.clone());

    if !draft.cc.is_empty() {
        builder = builder.cc(addrs_to_list(&draft.cc));
    }
    if !draft.bcc.is_empty() {
        builder = builder.bcc(addrs_to_list(&draft.bcc));
    }
    if let Some(in_reply_to) = &draft.in_reply_to {
        builder = builder.in_reply_to(in_reply_to.clone());
    }
    if !draft.references.is_empty() {
        builder = builder.references(draft.references.clone());
    }

    // Body — text + optional alternative HTML. mail-builder
    // builds multipart/alternative automatically when both are
    // supplied.
    if let Some(html) = &draft.body_html {
        builder = builder.html_body(html.clone());
    }
    builder = builder.text_body(draft.body_text.clone());

    // Attachments — `mail-builder` switches to multipart/mixed
    // as soon as one is present.
    for att in &draft.attachments {
        let filename = att.meta.filename.clone().unwrap_or_default();
        builder = builder.attachment(att.meta.mime.clone(), filename, att.data.clone());
    }

    let bytes = builder
        .write_to_vec()
        .map_err(|e| BuildError::Builder(e.to_string()))?;
    Ok((bytes, message_id))
}

fn addr_to_pair(a: &Addr) -> Address<'static> {
    Address::new_address(a.name.clone(), a.email.clone())
}

fn addrs_to_list(list: &[Addr]) -> Address<'static> {
    Address::new_list(list.iter().map(addr_to_pair).collect::<Vec<_>>())
}

fn generate_message_id(domain: &str) -> String {
    use std::hash::Hasher;
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // ns + a hash of the address-time pair = adequate uniqueness
    // for a client. Server may rewrite anyway.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(domain.as_bytes());
    h.write_u128(now);
    format!("<{:x}.{:x}@{domain}>", now, h.finish())
}

#[cfg(test)]
mod tests {
    use super::*;
    use email_proto::{Addr, Attachment, Draft};

    fn base_draft() -> Draft {
        Draft {
            from: Addr {
                name: Some("Alice".into()),
                email: "alice@example.com".into(),
            },
            to: vec![Addr {
                name: None,
                email: "bob@example.com".into(),
            }],
            cc: vec![],
            bcc: vec![],
            subject: "Hello there".into(),
            body_text: "First line.\nSecond line.\n".into(),
            body_html: None,
            in_reply_to: None,
            references: vec![],
            attachments: vec![],
        }
    }

    #[test]
    fn builds_minimal_rfc5322() {
        let d = base_draft();
        let (bytes, mid) = build_message(&d).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("From:"));
        assert!(s.contains("alice@example.com"));
        assert!(s.contains("To:"));
        assert!(s.contains("bob@example.com"));
        assert!(s.contains("Subject: Hello there"));
        // mail-builder may fold long Message-ID lines onto a
        // continuation; just assert the id token itself appears.
        let mid_trimmed = mid.trim_start_matches('<').trim_end_matches('>');
        assert!(
            s.contains(mid_trimmed),
            "expected {mid_trimmed:?} in output"
        );
        // Body present somewhere after the blank line.
        assert!(s.contains("First line"));
    }

    #[test]
    fn rejects_draft_without_from() {
        let mut d = base_draft();
        d.from.email.clear();
        assert!(matches!(
            build_message(&d).unwrap_err(),
            BuildError::Missing("from")
        ));
    }

    #[test]
    fn rejects_draft_without_recipient() {
        let mut d = base_draft();
        d.to.clear();
        assert!(matches!(
            build_message(&d).unwrap_err(),
            BuildError::Missing("to/cc/bcc")
        ));
    }

    #[test]
    fn includes_threading_headers_on_reply() {
        let mut d = base_draft();
        d.in_reply_to = Some("<parent@example.com>".into());
        d.references = vec![
            "<grandparent@example.com>".into(),
            "<parent@example.com>".into(),
        ];
        let (bytes, _) = build_message(&d).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("In-Reply-To:"));
        assert!(s.contains("parent@example.com"));
        assert!(s.contains("References:"));
        assert!(s.contains("grandparent@example.com"));
    }

    #[test]
    fn attachments_produce_multipart() {
        let mut d = base_draft();
        d.attachments
            .push(Attachment::new("hello.txt", b"hello".to_vec()).with_content_type("text/plain"));
        let (bytes, _) = build_message(&d).unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.to_lowercase().contains("multipart/mixed"));
        assert!(s.contains("hello.txt"));
    }

    #[test]
    fn message_ids_are_unique() {
        let d = base_draft();
        let (_, a) = build_message(&d).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let (_, b) = build_message(&d).unwrap();
        assert_ne!(a, b, "consecutive message ids should differ");
    }
}
