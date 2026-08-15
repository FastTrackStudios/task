//! Readwise importers — classic highlights (v2 export API)
//! and Reader documents (v3 list API).
//!
//! Auth is `Authorization: Token <key>` for both. Rate limit
//! is 20 req/min on the export endpoint — we sleep ~3.2 s
//! between pages and honor `Retry-After` on 429.
//!
//! - v2 export: `GET /api/v2/export/?updatedAfter=…&pageCursor=…`
//!   → books with nested highlight arrays. `updatedAfter` is
//!   the incremental cursor (pass the previous run's end time).
//! - Reader v3: `GET /api/v3/list/?withHtmlContent=true&pageCursor=…`
//!   → documents with `html_content` (full reader-clean HTML).
//!
//! Parsers are pure (`parse_export_page`, `parse_reader_page`)
//! and unit-tested against recorded response shapes; the
//! fetchers are thin pagination loops over them.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Highlight, ImportedItem};
use crate::ArchiveError;

const EXPORT_URL: &str = "https://readwise.io/api/v2/export/";
const READER_LIST_URL: &str = "https://readwise.io/api/v3/list/";
/// 20/min on v2 export ⇒ 3 s floor; a hair over for clock slop.
const PAGE_DELAY: std::time::Duration = std::time::Duration::from_millis(3200);
const MAX_429_RETRIES: u32 = 3;

/// One parsed page: items + the next cursor (None = done).
#[derive(Debug, Clone)]
pub struct Page {
    pub items: Vec<ImportedItem>,
    pub next_cursor: Option<String>,
}

/// Parse one v2 `/export/` response page.
pub fn parse_export_page(json: &str) -> Result<Page, ArchiveError> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| ArchiveError::ImportParse(format!("readwise export: {e}")))?;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| ArchiveError::ImportParse("readwise export: no results".into()))?;
    let mut items = Vec::new();
    for book in results {
        // `source_url` is the original article; books from
        // Kindle etc. may have none — skip those (no URL, no
        // archive target).
        let Some(url) = book
            .get("source_url")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("http"))
        else {
            continue;
        };
        let title = book
            .get("readable_title")
            .or_else(|| book.get("title"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let tags = book
            .get("book_tags")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str))
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let highlights: Vec<Highlight> = book
            .get("highlights")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|h| {
                        let text = h.get("text")?.as_str()?.trim().to_string();
                        if text.is_empty() {
                            return None;
                        }
                        Some(Highlight {
                            text,
                            note: h
                                .get("note")
                                .and_then(Value::as_str)
                                .map(str::trim)
                                .filter(|n| !n.is_empty())
                                .map(ToString::to_string),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let saved_at = book
            .get("highlights")
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .and_then(|h| h.get("highlighted_at"))
            .and_then(Value::as_str)
            .and_then(parse_ts);
        items.push(ImportedItem {
            url: url.to_string(),
            title,
            saved_at,
            tags,
            html: None,
            highlights,
            note: None,
            origin: "readwise-v2".into(),
        });
    }
    Ok(Page {
        items,
        next_cursor: cursor_of(&v, "nextPageCursor"),
    })
}

/// Parse one Reader v3 `/list/` response page.
pub fn parse_reader_page(json: &str) -> Result<Page, ArchiveError> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| ArchiveError::ImportParse(format!("readwise reader: {e}")))?;
    let results = v
        .get("results")
        .and_then(Value::as_array)
        .ok_or_else(|| ArchiveError::ImportParse("readwise reader: no results".into()))?;
    let mut items = Vec::new();
    for doc in results {
        // Reader wraps everything in its own reader URL; the
        // original lives in `source_url`. Skip non-http
        // (email-forwarded docs report `mailto:`-ish values).
        let url = doc
            .get("source_url")
            .and_then(Value::as_str)
            .filter(|u| u.starts_with("http"))
            .or_else(|| doc.get("url").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        if url.is_empty() {
            continue;
        }
        // Notes/highlights ride as child documents in v3;
        // whole-doc import keeps `parent_id == null` rows.
        if doc.get("parent_id").is_some_and(|p| !p.is_null()) {
            continue;
        }
        let tags = doc
            .get("tags")
            .and_then(Value::as_object)
            .map(|m| m.keys().cloned().collect())
            .unwrap_or_default();
        items.push(ImportedItem {
            url,
            title: doc
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            saved_at: doc
                .get("created_at")
                .and_then(Value::as_str)
                .and_then(parse_ts),
            tags,
            html: doc
                .get("html_content")
                .and_then(Value::as_str)
                .filter(|h| !h.trim().is_empty())
                .map(ToString::to_string),
            highlights: Vec::new(),
            note: doc
                .get("notes")
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
                .map(ToString::to_string),
            origin: "readwise-reader-v3".into(),
        });
    }
    Ok(Page {
        items,
        next_cursor: cursor_of(&v, "nextPageCursor"),
    })
}

/// Fetch every v2 export page. `updated_after` = RFC3339
/// incremental cursor from the previous run.
pub async fn fetch_export(
    client: &reqwest::Client,
    token: &str,
    updated_after: Option<&str>,
) -> Result<Vec<ImportedItem>, ArchiveError> {
    fetch_paged(
        client,
        token,
        EXPORT_URL,
        "pageCursor",
        updated_after,
        parse_export_page,
    )
    .await
}

/// Fetch every Reader v3 page (with stored HTML).
pub async fn fetch_reader(
    client: &reqwest::Client,
    token: &str,
    updated_after: Option<&str>,
) -> Result<Vec<ImportedItem>, ArchiveError> {
    fetch_paged(
        client,
        token,
        &format!("{READER_LIST_URL}?withHtmlContent=true"),
        "pageCursor",
        updated_after,
        parse_reader_page,
    )
    .await
}

async fn fetch_paged(
    client: &reqwest::Client,
    token: &str,
    base_url: &str,
    cursor_param: &str,
    updated_after: Option<&str>,
    parse: fn(&str) -> Result<Page, ArchiveError>,
) -> Result<Vec<ImportedItem>, ArchiveError> {
    let mut items = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let mut url = reqwest::Url::parse(base_url)
            .map_err(|e| ArchiveError::ImportParse(format!("bad base url: {e}")))?;
        if let Some(ua) = updated_after {
            url.query_pairs_mut().append_pair("updatedAfter", ua);
        }
        if let Some(c) = &cursor {
            url.query_pairs_mut().append_pair(cursor_param, c);
        }
        let body = get_with_retry(client, url.as_str(), token).await?;
        let page = parse(&body)?;
        tracing::info!(items = page.items.len(), "readwise page fetched");
        items.extend(page.items);
        match page.next_cursor {
            Some(c) => {
                cursor = Some(c);
                tokio::time::sleep(PAGE_DELAY).await;
            }
            None => return Ok(items),
        }
    }
}

async fn get_with_retry(
    client: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<String, ArchiveError> {
    for _attempt in 0..=MAX_429_RETRIES {
        let resp = client
            .get(url)
            .header("authorization", format!("Token {token}"))
            .send()
            .await
            .map_err(|e| ArchiveError::Fetch {
                url: url.to_string(),
                message: e.to_string(),
            })?;
        if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
            let wait = resp
                .headers()
                .get("retry-after")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(10)
                .min(120);
            tracing::warn!(wait, "readwise 429 — backing off");
            tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
            continue;
        }
        if !resp.status().is_success() {
            return Err(ArchiveError::BadResponse {
                url: url.to_string(),
                message: format!("HTTP {} (token valid?)", resp.status()),
            });
        }
        return resp.text().await.map_err(|e| ArchiveError::Fetch {
            url: url.to_string(),
            message: e.to_string(),
        });
    }
    Err(ArchiveError::BadResponse {
        url: url.to_string(),
        message: format!("still 429 after {MAX_429_RETRIES} retries"),
    })
}

fn cursor_of(v: &Value, key: &str) -> Option<String> {
    match v.get(key) {
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        // v2 has been seen returning numeric cursors.
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded v2 /export/ shape (PRD research).
    const EXPORT_FIXTURE: &str = r#"{
      "count": 2,
      "nextPageCursor": "01gx0cursor",
      "results": [
        {
          "user_book_id": 123,
          "title": "Some Article",
          "readable_title": "Some Article, Readable",
          "author": "Jane Doe",
          "source": "reader",
          "category": "articles",
          "source_url": "https://example.com/some-article?utm_source=newsletter",
          "book_tags": [{"id": 1, "name": "rust"}],
          "highlights": [
            {"id": 11, "text": "First highlight.", "note": "ponder this",
             "location": 100, "highlighted_at": "2026-01-02T03:04:05Z", "tags": []},
            {"id": 12, "text": "Second highlight.", "note": "", "location": 200,
             "highlighted_at": "2026-01-03T03:04:05Z", "tags": []}
          ]
        },
        {
          "user_book_id": 124,
          "title": "A Kindle Book",
          "source": "kindle",
          "category": "books",
          "source_url": null,
          "highlights": [{"id": 13, "text": "Book highlight."}]
        }
      ]
    }"#;

    /// Recorded Reader v3 /list/ shape.
    const READER_FIXTURE: &str = r#"{
      "count": 2,
      "nextPageCursor": null,
      "results": [
        {
          "id": "01gwdoc",
          "url": "https://read.readwise.io/read/01gwdoc",
          "source_url": "https://example.com/reader-article",
          "title": "Reader Article",
          "author": "Jane",
          "category": "article",
          "parent_id": null,
          "created_at": "2026-02-03T04:05:06.789Z",
          "tags": {"deep-dive": {"name": "deep-dive"}},
          "html_content": "<h2>Stored</h2><p>Full reader HTML.</p>",
          "notes": "good one"
        },
        {
          "id": "01gwchild",
          "parent_id": "01gwdoc",
          "source_url": "https://example.com/reader-article",
          "title": "a highlight child row"
        }
      ]
    }"#;

    #[test]
    fn export_page_parses_books_with_urls() {
        let page = parse_export_page(EXPORT_FIXTURE).unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("01gx0cursor"));
        // Kindle book without source_url is skipped.
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.title, "Some Article, Readable");
        assert_eq!(item.origin, "readwise-v2");
        assert_eq!(item.tags, vec!["rust"]);
        assert_eq!(item.highlights.len(), 2);
        assert_eq!(item.highlights[0].note.as_deref(), Some("ponder this"));
        assert_eq!(item.highlights[1].note, None);
        assert_eq!(
            item.saved_at.unwrap().to_rfc3339(),
            "2026-01-02T03:04:05+00:00"
        );
    }

    #[test]
    fn reader_page_parses_docs_and_skips_children() {
        let page = parse_reader_page(READER_FIXTURE).unwrap();
        assert_eq!(page.next_cursor, None);
        assert_eq!(page.items.len(), 1);
        let item = &page.items[0];
        assert_eq!(item.url, "https://example.com/reader-article");
        assert_eq!(item.origin, "readwise-reader-v3");
        assert!(item.html.as_deref().unwrap().contains("Stored"));
        assert_eq!(item.note.as_deref(), Some("good one"));
        assert_eq!(item.tags, vec!["deep-dive"]);
    }
}
