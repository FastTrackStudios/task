//! YAML frontmatter parser tailored to the link layer's needs.
//! Recognizes the `---` delimited block at the top of an
//! Obsidian-style markdown file and pulls out the two fields
//! we care about: `type` / `kind` (the entity kind) and
//! `emails:` (the link list).
//!
//! We don't try to round-trip the entire frontmatter — for that
//! job `knowledge-proto` already exists. This is a minimal
//! reader so the link walker can stay decoupled.

use serde::Deserialize;

/// Parse-result for the link walker.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ParsedFrontmatter {
    /// `type:` or `kind:` field — case-insensitive lookup,
    /// lower-cased on output. `None` when neither is present.
    pub kind: Option<String>,
    /// `id:` or `uuid:` field — whichever the file uses to
    /// identify itself. `None` when neither is present; the
    /// caller typically falls back to the file path.
    pub id: Option<String>,
    /// Every entry under `emails:`. Each item may be either a
    /// string (bare Message-ID) or a `{message_id: ..., ...}`
    /// map (matches the existing `EmailRef` shape). We only
    /// keep the Message-ID; the rest is the email store's job.
    pub emails: Vec<String>,
}

/// Split a markdown file into `(frontmatter_str, body)`.
/// Frontmatter is the content between two `---\n` lines at the
/// very top of the file. Returns `(None, full_body)` when no
/// frontmatter is present.
#[must_use]
pub fn split(content: &str) -> (Option<&str>, &str) {
    if !content.starts_with("---\n") && !content.starts_with("---\r\n") {
        return (None, content);
    }
    let after_open = match content.find('\n') {
        Some(i) => &content[i + 1..],
        None => return (None, content),
    };
    if let Some(end_rel) = find_closing(after_open) {
        let fm = &after_open[..end_rel.start];
        let body_start = end_rel.body_start;
        (Some(fm), &after_open[body_start..])
    } else {
        (None, content)
    }
}

struct CloseMarker {
    start: usize,
    body_start: usize,
}

/// Find the closing `---` line. Tolerates either `\n` or `\r\n`
/// endings and skips lines beginning with `---` that aren't the
/// terminator (e.g. inside a YAML block).
fn find_closing(s: &str) -> Option<CloseMarker> {
    let mut i = 0;
    while i < s.len() {
        let rest = &s[i..];
        if rest.starts_with("---\n") {
            return Some(CloseMarker {
                start: i,
                body_start: i + 4,
            });
        }
        if rest.starts_with("---\r\n") {
            return Some(CloseMarker {
                start: i,
                body_start: i + 5,
            });
        }
        if rest.starts_with("---") && rest.len() == 3 {
            return Some(CloseMarker {
                start: i,
                body_start: i + 3,
            });
        }
        // Advance to next line.
        match rest.find('\n') {
            Some(off) => i += off + 1,
            None => return None,
        }
    }
    None
}

/// Parse `fm` (the YAML text between the `---` markers) for
/// the fields the link layer cares about. Unknown keys are
/// silently ignored.
pub fn parse_frontmatter(fm: &str) -> ParsedFrontmatter {
    #[derive(Deserialize, Default)]
    struct Raw {
        #[serde(default)]
        r#type: Option<String>,
        #[serde(default)]
        kind: Option<String>,
        #[serde(default)]
        id: Option<String>,
        #[serde(default)]
        uuid: Option<String>,
        #[serde(default)]
        emails: Option<serde_yaml::Value>,
    }

    let raw: Raw = match serde_yaml::from_str(fm) {
        Ok(r) => r,
        Err(err) => {
            tracing::debug!(%err, "frontmatter parse failed");
            return ParsedFrontmatter::default();
        }
    };

    let kind = raw
        .r#type
        .or(raw.kind)
        .map(|s| s.trim().to_lowercase())
        .filter(|s| !s.is_empty());
    let id = raw.id.or(raw.uuid).filter(|s| !s.is_empty());

    let emails = match raw.emails {
        Some(v) => extract_message_ids(&v),
        None => Vec::new(),
    };

    ParsedFrontmatter { kind, id, emails }
}

/// `emails:` may be a list of strings OR a list of maps. We
/// accept both, falling back gracefully on shapes we don't
/// recognize.
fn extract_message_ids(v: &serde_yaml::Value) -> Vec<String> {
    let mut out = Vec::new();
    let Some(seq) = v.as_sequence() else {
        // Single scalar — treat as one id.
        if let Some(s) = v.as_str() {
            out.push(s.trim().to_string());
        }
        return out;
    };
    for item in seq {
        if let Some(s) = item.as_str() {
            out.push(s.trim().to_string());
            continue;
        }
        if let Some(map) = item.as_mapping() {
            // Look for `message_id:` (preferred) or `id:` /
            // `mid:` keys. Matches the `EmailRef` shape.
            for key in ["message_id", "messageId", "id", "mid", "Message-ID"] {
                if let Some(val) = map.get(serde_yaml::Value::String(key.into())) {
                    if let Some(s) = val.as_str() {
                        out.push(s.trim().to_string());
                        break;
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_no_frontmatter() {
        let (fm, body) = split("hello world");
        assert!(fm.is_none());
        assert_eq!(body, "hello world");
    }

    #[test]
    fn split_with_frontmatter() {
        let (fm, body) = split("---\nfoo: bar\n---\nbody here\n");
        assert_eq!(fm, Some("foo: bar\n"));
        assert_eq!(body, "body here\n");
    }

    #[test]
    fn parses_kind_from_type_or_kind() {
        let p = parse_frontmatter("type: project\n");
        assert_eq!(p.kind.as_deref(), Some("project"));
        let p = parse_frontmatter("kind: task\n");
        assert_eq!(p.kind.as_deref(), Some("task"));
    }

    #[test]
    fn parses_id_from_id_or_uuid() {
        let p = parse_frontmatter("id: abc-123\n");
        assert_eq!(p.id.as_deref(), Some("abc-123"));
        let p = parse_frontmatter("uuid: 00000000-0000-0000-0000-000000000000\n");
        assert!(p.id.as_ref().unwrap().starts_with("00000000"));
    }

    #[test]
    fn parses_emails_as_string_list() {
        let p = parse_frontmatter("emails:\n  - <a@example.com>\n  - <b@example.com>\n");
        assert_eq!(p.emails, vec!["<a@example.com>", "<b@example.com>"]);
    }

    #[test]
    fn parses_emails_as_map_list() {
        // The EmailRef shape from `task-core/src/email/model.rs`.
        // YAML requires keys under a list item to align — both
        // `message_id` and `subject` start in the same column.
        let p = parse_frontmatter(
            "emails:\n\
             - message_id: <a@example.com>\n  subject: Hello\n\
             - message_id: <b@example.com>\n  subject: World\n",
        );
        assert_eq!(p.emails, vec!["<a@example.com>", "<b@example.com>"]);
    }

    #[test]
    fn parses_emails_mixed_shapes() {
        let p = parse_frontmatter(
            "emails:\n\
             - <a@example.com>\n\
             - message_id: <b@example.com>\n  subject: Hello\n",
        );
        assert_eq!(p.emails.len(), 2);
    }

    #[test]
    fn invalid_yaml_returns_default() {
        let p = parse_frontmatter("not: : valid: yaml: [");
        assert!(p.emails.is_empty());
        assert!(p.kind.is_none());
    }

    #[test]
    fn empty_frontmatter_returns_default() {
        let p = parse_frontmatter("");
        assert_eq!(p, ParsedFrontmatter::default());
    }
}
