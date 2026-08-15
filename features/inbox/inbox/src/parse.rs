//! Markdown → [`InboxItem`].
//!
//! The structured fields live in the leading YAML frontmatter; the
//! markdown body *is* the verbatim capture. Tolerant by design — a
//! missing field falls back to a sensible default so one malformed
//! file never nukes the whole inbox list (the scanner logs + skips
//! truly unreadable files).

use inbox_proto::InboxItem;
use vault_entity::yaml;

pub use vault_entity::ParseError;

/// Split a markdown file's leading YAML frontmatter from the body.
/// Returns `(frontmatter, body)`. One splitter for the whole tree —
/// [`vault_entity::frontmatter::split`].
#[must_use]
pub fn frontmatter_split(src: &str) -> Option<(&str, &str)> {
    vault_entity::frontmatter::split(src)
}

/// Parse a captured inbox item. `path` supplies the fallback id
/// (the file stem) when frontmatter has none; `body` is the raw
/// markdown after the frontmatter fence.
pub fn parse_inbox_item(
    path: &str,
    frontmatter_yaml: &str,
    body: &str,
) -> Result<InboxItem, ParseError> {
    let map: serde_yaml::Mapping =
        serde_yaml::from_str(frontmatter_yaml).map_err(|e| ParseError::Yaml(e.to_string()))?;

    let id = yaml::str_at(&map, "id").unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string()
    });

    Ok(InboxItem {
        id,
        body: body.trim().to_string(),
        kind: yaml::str_at(&map, "kind").unwrap_or_else(|| InboxItem::KIND_FLEETING.to_string()),
        status: yaml::str_at(&map, "status").unwrap_or_else(|| InboxItem::STATUS_OPEN.to_string()),
        source: yaml::str_at(&map, "source").unwrap_or_default(),
        created: yaml::str_at(&map, "created").unwrap_or_default(),
        resurface_on: yaml::str_at(&map, "resurface_on"),
        processed_into: yaml::str_at(&map, "processed_into"),
        // Spaced-repetition state (obsidian-spaced-repetition SM-2).
        // Absent on notes captured before SR landed → sensible defaults:
        // BASE_EASE, never-reviewed interval 0, zero reviews.
        ease: yaml::i64_at(&map, "sr-ease").unwrap_or(inbox_proto::schedule::BASE_EASE),
        interval: yaml::i64_at(&map, "sr-interval").unwrap_or(0),
        reviews: yaml::i64_at(&map, "sr-reviews").unwrap_or(0),
    })
}
