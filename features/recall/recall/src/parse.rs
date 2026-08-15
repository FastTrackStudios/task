//! Markdown → [`RecallCard`].
//!
//! The structured fields live in the leading YAML frontmatter (FSRS
//! state under `sr-*` keys); the body holds the front + back prompt,
//! split by a `<!-- back -->` marker. Tolerant by design — a missing
//! field falls back to a sensible default so one malformed file never
//! nukes the whole deck (the scanner logs + skips truly unreadable
//! files).

use recall_proto::{CardType, RecallCard};
use vault_entity::yaml;

pub use vault_entity::ParseError;

/// The line that separates the front from the back in the body.
pub const BACK_MARKER: &str = "<!-- back -->";

/// Split a markdown file's leading YAML frontmatter from the body.
/// Returns `(frontmatter, body)`. One splitter for the whole tree —
/// [`vault_entity::frontmatter::split`].
#[must_use]
pub fn frontmatter_split(src: &str) -> Option<(&str, &str)> {
    vault_entity::frontmatter::split(src)
}

/// Split a card body into `(front, back)` around [`BACK_MARKER`]. A
/// body without the marker is treated as a front-only card.
#[must_use]
pub fn split_body(body: &str) -> (String, String) {
    let needle = format!("\n{BACK_MARKER}\n");
    match body.split_once(&needle) {
        Some((front, back)) => (front.trim().to_string(), back.trim().to_string()),
        None => (body.trim().to_string(), String::new()),
    }
}

/// Parse a learning card. `path` supplies the fallback id (the file
/// stem) when frontmatter has none; `body` is the raw markdown after
/// the frontmatter fence.
pub fn parse_recall_card(
    path: &str,
    frontmatter_yaml: &str,
    body: &str,
) -> Result<RecallCard, ParseError> {
    let map: serde_yaml::Mapping = serde_yaml::from_str(frontmatter_yaml)
        .map_err(|e| ParseError::Yaml(e.to_string()))?;

    let id = yaml::str_at(&map, "id").unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string()
    });

    let (front, back) = split_body(body);

    Ok(RecallCard {
        id,
        project: yaml::str_at(&map, "project").unwrap_or_default(),
        card_type: yaml::str_at(&map, "card_type").unwrap_or_else(|| CardType::FREE.to_string()),
        front,
        back,
        source_note: yaml::str_at(&map, "source_note"),
        stability: yaml::f64_at(&map, "sr-stability").unwrap_or(0.0),
        difficulty: yaml::f64_at(&map, "sr-difficulty").unwrap_or(0.0),
        reps: yaml::i64_at(&map, "sr-reps").unwrap_or(0),
        lapses: yaml::i64_at(&map, "sr-lapses").unwrap_or(0),
        due: yaml::str_at(&map, "sr-due"),
        last_review: yaml::str_at(&map, "sr-last-review"),
        archived: yaml::bool_at(&map, "archived").unwrap_or(false),
        created: yaml::str_at(&map, "created").unwrap_or_default(),
    })
}
