//! Wikilink form for inline references in markdown notes:
//!
//! ```text
//! [[email://<message-id>]]
//! [[email://<message-id>|Display text]]
//! [[email://abc@example.com|Re: status]]
//! ```
//!
//! The message-id can be bare (`abc@example.com`) or
//! angle-bracketed (`<abc@example.com>`); we strip brackets on
//! parse so callers always work with the canonical form.
//!
//! Shape mirrors `knowledge-proto::ENTITY_LINK_REGEX`
//! (`[[entity://kind/uuid|alias]]`); we use a dedicated
//! `email://` prefix instead of `entity://email/...` because
//! Message-IDs aren't UUIDs and the encoding bracketing is
//! annoying.

use crate::link::bare_message_id;
use regex::Regex;
use std::sync::OnceLock;

/// One match from [`parse_wikilinks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmailWikilink {
    /// Canonical (bare, no angle brackets) Message-ID.
    pub message_id: String,
    /// Optional display text. When `None`, the renderer
    /// typically falls back to the subject from the indexed
    /// envelope.
    pub label: Option<String>,
}

fn email_link_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        // Message-ID is allowed any RFC 5322 atom + `@` + domain
        // chars. We accept a permissive set: alphanumerics,
        // dot, hyphen, underscore, plus, `@`, optional angle
        // brackets stripped on capture.
        Regex::new(r"\[\[email://<?([A-Za-z0-9._+\-]+@[A-Za-z0-9._\-]+)>?(?:\|([^\]\r\n]+))?\]\]")
            .expect("email wikilink regex compiles")
    })
}

/// Find every `[[email://...]]` wikilink in `content`. Returns
/// them in source order; duplicate Message-IDs are reported
/// once each (with the first label seen).
#[must_use]
pub fn parse_wikilinks(content: &str) -> Vec<EmailWikilink> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for caps in email_link_re().captures_iter(content) {
        let mid = caps
            .get(1)
            .map(|m| m.as_str().to_string())
            .unwrap_or_default();
        if !seen.insert(mid.clone()) {
            continue;
        }
        let label = caps.get(2).map(|m| m.as_str().trim().to_string());
        out.push(EmailWikilink {
            message_id: mid,
            label,
        });
    }
    out
}

/// Inverse of [`parse_wikilinks`]: produce a wikilink string
/// for one message. Brackets are stripped from the id so the
/// rendered form is `[[email://abc@example.com|Subject]]`.
#[must_use]
pub fn format_wikilink(message_id: &str, label: Option<&str>) -> String {
    let bare = bare_message_id(message_id);
    match label {
        Some(l) => format!("[[email://{bare}|{l}]]"),
        None => format!("[[email://{bare}]]"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_bare_id() {
        let s = "see [[email://abc@example.com]] for context";
        let links = parse_wikilinks(s);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].message_id, "abc@example.com");
        assert_eq!(links[0].label, None);
    }

    #[test]
    fn parses_with_label() {
        let s = "[[email://abc@example.com|Re: status]]";
        let links = parse_wikilinks(s);
        assert_eq!(links[0].label.as_deref(), Some("Re: status"));
    }

    #[test]
    fn strips_angle_brackets() {
        let s = "[[email://<abc@example.com>]]";
        let links = parse_wikilinks(s);
        assert_eq!(links[0].message_id, "abc@example.com");
    }

    #[test]
    fn deduplicates_repeated_links() {
        let s = "first [[email://a@b.com]] then [[email://a@b.com|repeat]] again";
        let links = parse_wikilinks(s);
        assert_eq!(links.len(), 1);
        // First label wins.
        assert_eq!(links[0].label, None);
    }

    #[test]
    fn ignores_non_email_wikilinks() {
        let s = "[[Some Page]] and [[entity://task/abc]] and [[email://x@y.com]]";
        let links = parse_wikilinks(s);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].message_id, "x@y.com");
    }

    #[test]
    fn format_roundtrips_through_parse() {
        let formatted = format_wikilink("<test@example.com>", Some("Hello"));
        assert_eq!(formatted, "[[email://test@example.com|Hello]]");
        let links = parse_wikilinks(&formatted);
        assert_eq!(links.len(), 1);
        assert_eq!(links[0].message_id, "test@example.com");
        assert_eq!(links[0].label.as_deref(), Some("Hello"));
    }

    #[test]
    fn multiple_distinct_links() {
        let s = "[[email://a@x.com|Alpha]] and [[email://b@x.com|Beta]]";
        let links = parse_wikilinks(s);
        assert_eq!(links.len(), 2);
        assert_eq!(links[0].message_id, "a@x.com");
        assert_eq!(links[1].message_id, "b@x.com");
    }
}
