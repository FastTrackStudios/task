//! Bookmark-service importers — batch front-ends to the
//! archive router.
//!
//! Every importer normalizes its service's shape into
//! [`ImportedItem`]s; [`item_to_source`] then runs each item
//! through the SAME router/provenance path as a one-off
//! `task wiki archive <url>`, so canonical-URL dedup works
//! across services (the same article saved in Pocket and
//! Readwise lands on one `raw/sources/` file).
//!
//! | Module      | Service                | Transport                     |
//! |-------------|------------------------|-------------------------------|
//! | [`readwise`]| Readwise v2 + Reader v3| token API (rate-limited)      |
//! | [`karakeep`]| Karakeep               | bearer API (`includeContent`) |
//! | [`pocket`]  | Pocket (service is dead)| export zip: CSV + annotations|
//! | [`netscape`]| Any browser            | bookmarks HTML via scraper    |
//!
//! Idempotency comes from the canonical-URL filename scheme
//! (re-running an import skips everything already archived);
//! incremental cursors ride each service's native mechanism
//! (`updatedAfter`, page cursors) — the CLI prints the next
//! cursor value after each run.

pub mod karakeep;
pub mod netscape;
pub mod pocket;
pub mod readwise;

use chrono::{DateTime, Utc};

use crate::provenance::Provenance;
use crate::{ArchiveError, router};

/// One bookmark/document from any importer, normalized.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportedItem {
    /// Original URL as the service recorded it.
    pub url: String,
    /// Service title (may be empty — falls back to the URL).
    pub title: String,
    /// When the user saved it (service time, not import time).
    pub saved_at: Option<DateTime<Utc>>,
    pub tags: Vec<String>,
    /// Pre-fetched article HTML, when the service stores it
    /// (Readwise Reader `html_content`, Karakeep
    /// `htmlContent`). Converted with htmd — no readability
    /// re-pass on already-clean content.
    pub html: Option<String>,
    /// User highlights / annotations.
    pub highlights: Vec<Highlight>,
    /// Free-form user note attached to the whole item.
    pub note: Option<String>,
    /// Which importer produced it — lands in the provenance
    /// `extractor` field (`readwise-v2`, `pocket-export`, …).
    pub origin: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Highlight {
    pub text: String,
    pub note: Option<String>,
}

/// Convert one imported item into the `(Provenance, body)`
/// pair the archive pipeline writes. Pure — no network.
///
/// `content_type` honesty: items carrying stored content (or
/// at least highlights) keep their route's type; bare link
/// imports are `bookmark` — the page was never extracted, and
/// pretending otherwise would poison extractor-quality debug
/// later.
pub fn item_to_source(item: &ImportedItem) -> Result<(Provenance, String), ArchiveError> {
    let (url, route) = router::classify(&item.url)?;
    let canonical = router::canonicalize(&url, &route);

    let markdown = match &item.html {
        Some(html) => Some(crate::article::clean_html_to_markdown(html)?),
        None => None,
    };
    let content_type = if markdown.is_some() || !item.highlights.is_empty() {
        router::content_type_for(&route)
    } else {
        "bookmark"
    };

    let title = if item.title.trim().is_empty() {
        item.url.clone()
    } else {
        item.title.trim().to_string()
    };
    let prov = Provenance::new(title, &item.url, canonical, content_type, &item.origin);

    // ── Body ────────────────────────────────────────────
    let mut body = String::new();
    let mut lede: Vec<String> = Vec::new();
    if let Some(at) = item.saved_at {
        lede.push(format!("saved {}", at.format("%Y-%m-%d")));
    }
    if !item.tags.is_empty() {
        lede.push(format!("tags: {}", item.tags.join(", ")));
    }
    if !lede.is_empty() {
        body.push_str(&format!(
            "_{} import · {}_\n\n",
            item.origin,
            lede.join(" · ")
        ));
    }
    if let Some(note) = item
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty())
    {
        body.push_str(&format!("> [!note] Saver's note\n> {note}\n\n"));
    }
    if let Some(md) = &markdown {
        body.push_str(md);
        body.push_str("\n\n");
    }
    if !item.highlights.is_empty() {
        body.push_str("## Highlights\n\n");
        for h in &item.highlights {
            for line in h.text.trim().lines() {
                body.push_str(&format!("> {line}\n"));
            }
            if let Some(note) = h.note.as_deref().map(str::trim).filter(|n| !n.is_empty()) {
                body.push_str(&format!(">\n> — _note: {note}_\n"));
            }
            body.push('\n');
        }
    }
    if body.trim().is_empty() {
        body = format!(
            "_Bookmark imported from {} — content not fetched._",
            item.origin
        );
    }
    Ok((prov, body.trim_end().to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_bookmark_is_honest() {
        let item = ImportedItem {
            url: "https://example.com/post?utm_source=pocket".into(),
            title: "A Post".into(),
            origin: "pocket-export".into(),
            ..Default::default()
        };
        let (prov, body) = item_to_source(&item).unwrap();
        assert_eq!(prov.content_type, "bookmark");
        assert_eq!(prov.canonical_url, "https://example.com/post");
        assert_eq!(prov.extractor, "pocket-export");
        assert!(body.contains("content not fetched"));
    }

    #[test]
    fn stored_html_becomes_article() {
        let item = ImportedItem {
            url: "https://example.com/post".into(),
            title: "A Post".into(),
            html: Some("<h2>Section</h2><p>Stored body.</p>".into()),
            tags: vec!["rust".into()],
            origin: "readwise-reader-v3".into(),
            ..Default::default()
        };
        let (prov, body) = item_to_source(&item).unwrap();
        assert_eq!(prov.content_type, "article");
        assert!(body.contains("## Section"));
        assert!(body.contains("tags: rust"));
    }

    #[test]
    fn highlights_render_as_quotes() {
        let item = ImportedItem {
            url: "https://example.com/post".into(),
            title: "A Post".into(),
            highlights: vec![Highlight {
                text: "Key insight.".into(),
                note: Some("revisit".into()),
            }],
            origin: "readwise-v2".into(),
            ..Default::default()
        };
        let (prov, body) = item_to_source(&item).unwrap();
        assert_eq!(prov.content_type, "article");
        assert!(body.contains("> Key insight."));
        assert!(body.contains("_note: revisit_"));
    }

    #[test]
    fn cross_importer_dedup_key_matches() {
        let pocket = ImportedItem {
            url: "https://example.com/post/?utm_source=pocket".into(),
            title: "Post (Pocket)".into(),
            origin: "pocket-export".into(),
            ..Default::default()
        };
        let readwise = ImportedItem {
            url: "https://www.example.com/post".into(),
            title: "Post — Readwise Title".into(),
            origin: "readwise-v2".into(),
            ..Default::default()
        };
        let (p1, _) = item_to_source(&pocket).unwrap();
        let (p2, _) = item_to_source(&readwise).unwrap();
        assert_eq!(p1.canon8(), p2.canon8());
    }
}
