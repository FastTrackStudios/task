//! Markdown → [`Contact`].
//!
//! The structured fields live in the leading YAML frontmatter (emails /
//! phones / groups as sequences or a single scalar); the body holds the
//! free-form notes. Tolerant by design — a missing field falls back to
//! a sensible default so one malformed file never nukes the whole
//! directory (the scanner logs + skips truly unreadable files).

use contacts_proto::{Contact, ContactSource};
use vault_entity::yaml;

pub use vault_entity::ParseError;

/// Split a markdown file's leading YAML frontmatter from the body.
/// Returns `(frontmatter, body)`. One splitter for the whole tree —
/// [`vault_entity::frontmatter::split`].
#[must_use]
pub fn frontmatter_split(src: &str) -> Option<(&str, &str)> {
    vault_entity::frontmatter::split(src)
}

/// Read a multi-value field that may be a YAML sequence *or* a single
/// scalar (or newline-joined scalar), returning it newline-joined —
/// the shape the [`Contact`] entity stores.
///
/// No shared equivalent: `vault_entity::yaml::string_list_at` yields a
/// `Vec`, and contacts keep these fields newline-joined on the entity.
fn take_multi(map: &serde_yaml::Mapping, key: &str) -> String {
    match map.get(serde_yaml::Value::from(key)) {
        Some(serde_yaml::Value::Sequence(items)) => items
            .iter()
            .filter_map(serde_yaml::Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        Some(serde_yaml::Value::String(s)) => s
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

/// Parse a contact. `path` supplies the fallback id (the file stem)
/// when frontmatter has none; `body` is the raw markdown after the
/// frontmatter fence (the free-form notes).
pub fn parse_contact(
    path: &str,
    frontmatter_yaml: &str,
    body: &str,
) -> Result<Contact, ParseError> {
    let map: serde_yaml::Mapping = serde_yaml::from_str(frontmatter_yaml)
        .map_err(|e| ParseError::Yaml(e.to_string()))?;

    let id = yaml::str_at(&map, "id").unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string()
    });

    let notes = {
        let trimmed = body.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    };

    Ok(Contact {
        id,
        uid: yaml::str_at(&map, "uid"),
        full_name: yaml::str_at(&map, "full_name").unwrap_or_default(),
        given_name: yaml::str_at(&map, "given_name"),
        family_name: yaml::str_at(&map, "family_name"),
        organization: yaml::str_at(&map, "organization"),
        title: yaml::str_at(&map, "title"),
        emails: take_multi(&map, "emails"),
        phones: take_multi(&map, "phones"),
        address: yaml::str_at(&map, "address"),
        birthday: yaml::str_at(&map, "birthday"),
        photo_url: yaml::str_at(&map, "photo_url"),
        groups: take_multi(&map, "groups"),
        notes,
        source: yaml::str_at(&map, "source").unwrap_or_else(|| ContactSource::MANUAL.to_string()),
        account: yaml::str_at(&map, "account"),
        etag: yaml::str_at(&map, "etag"),
        linked_party_id: yaml::str_at(&map, "linked_party_id"),
        linked_user_id: yaml::str_at(&map, "linked_user_id"),
        archived: yaml::bool_at(&map, "archived").unwrap_or(false),
        created: yaml::str_at(&map, "created").unwrap_or_default(),
        updated: yaml::str_at(&map, "updated"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_scalar_email_as_single_value() {
        let fm = "type: contact\nid: x\nfull_name: Ada\nemails: ada@example.com\n";
        let c = parse_contact("x.md", fm, "").unwrap();
        assert_eq!(c.email_list(), vec!["ada@example.com"]);
    }

    #[test]
    fn falls_back_to_file_stem_for_id() {
        let fm = "full_name: Ada\n";
        let c = parse_contact("Records/contacts/from-stem.md", fm, "").unwrap();
        assert_eq!(c.id, "from-stem");
    }

    #[test]
    fn body_becomes_notes() {
        let fm = "id: x\nfull_name: Ada\n";
        let c = parse_contact("x.md", fm, "\nSome notes here.\n").unwrap();
        assert_eq!(c.notes.as_deref(), Some("Some notes here."));
    }
}
