//! Karakeep importer — `GET /api/v1/bookmarks` with
//! `includeContent=true`.
//!
//! Self-hosted, so the endpoint comes from the user
//! (`--endpoint https://keep.example.com`). Auth is
//! `Authorization: Bearer ak2_…` (API keys are `ak2_`-
//! prefixed). We use the REST API instead of Karakeep's JSON
//! export because the export is lossy — it drops the crawled
//! `htmlContent` we want to archive.

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::ImportedItem;
use crate::ArchiveError;

/// One parsed page: items + cursor (None = done). Karakeep
/// also returns non-link bookmarks (text notes, assets);
/// those have no URL to archive and are counted as skipped.
#[derive(Debug, Clone)]
pub struct Page {
    pub items: Vec<ImportedItem>,
    pub skipped_non_links: usize,
    pub next_cursor: Option<String>,
}

/// Parse one `/api/v1/bookmarks` response page.
pub fn parse_bookmarks_page(json: &str) -> Result<Page, ArchiveError> {
    let v: Value = serde_json::from_str(json)
        .map_err(|e| ArchiveError::ImportParse(format!("karakeep: {e}")))?;
    let bookmarks = v
        .get("bookmarks")
        .and_then(Value::as_array)
        .ok_or_else(|| ArchiveError::ImportParse("karakeep: no bookmarks array".into()))?;
    let mut items = Vec::new();
    let mut skipped = 0usize;
    for b in bookmarks {
        let content = b.get("content").unwrap_or(&Value::Null);
        if content.get("type").and_then(Value::as_str) != Some("link") {
            skipped += 1;
            continue;
        }
        let Some(url) = content.get("url").and_then(Value::as_str) else {
            skipped += 1;
            continue;
        };
        let title = b
            .get("title")
            .and_then(Value::as_str)
            .or_else(|| content.get("title").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        let tags = b
            .get("tags")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.get("name").and_then(Value::as_str))
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        items.push(ImportedItem {
            url: url.to_string(),
            title,
            saved_at: b
                .get("createdAt")
                .and_then(Value::as_str)
                .and_then(parse_ts),
            tags,
            html: content
                .get("htmlContent")
                .and_then(Value::as_str)
                .filter(|h| !h.trim().is_empty())
                .map(ToString::to_string),
            highlights: Vec::new(),
            note: b
                .get("note")
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
                .map(ToString::to_string),
            origin: "karakeep-api".into(),
        });
    }
    Ok(Page {
        items,
        skipped_non_links: skipped,
        next_cursor: v
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|c| !c.is_empty())
            .map(ToString::to_string),
    })
}

/// Fetch every bookmark page from a Karakeep instance.
/// Returns `(items, skipped_non_link_count)`.
pub async fn fetch_bookmarks(
    client: &reqwest::Client,
    endpoint: &str,
    token: &str,
) -> Result<(Vec<ImportedItem>, usize), ArchiveError> {
    let base = endpoint.trim_end_matches('/');
    let mut items = Vec::new();
    let mut skipped = 0usize;
    let mut cursor: Option<String> = None;
    loop {
        let mut url = format!("{base}/api/v1/bookmarks?includeContent=true&limit=50");
        if let Some(c) = &cursor {
            url.push_str("&cursor=");
            url.push_str(&urlencode(c));
        }
        let resp = client
            .get(&url)
            .header("authorization", format!("Bearer {token}"))
            .send()
            .await
            .map_err(|e| ArchiveError::Fetch {
                url: url.clone(),
                message: e.to_string(),
            })?;
        if !resp.status().is_success() {
            return Err(ArchiveError::BadResponse {
                url,
                message: format!("HTTP {} (ak2_ API key valid?)", resp.status()),
            });
        }
        let body = resp.text().await.map_err(|e| ArchiveError::Fetch {
            url: url.clone(),
            message: e.to_string(),
        })?;
        let page = parse_bookmarks_page(&body)?;
        tracing::info!(items = page.items.len(), "karakeep page fetched");
        items.extend(page.items);
        skipped += page.skipped_non_links;
        match page.next_cursor {
            Some(c) => {
                cursor = Some(c);
                // Be polite to self-hosted instances.
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
            }
            None => return Ok((items, skipped)),
        }
    }
}

fn parse_ts(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|d| d.with_timezone(&Utc))
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded `/api/v1/bookmarks?includeContent=true` shape.
    const FIXTURE: &str = r#"{
      "bookmarks": [
        {
          "id": "abc123",
          "createdAt": "2026-03-04T05:06:07.000Z",
          "title": "Curator Title",
          "note": "worth keeping",
          "tags": [{"id": "t1", "name": "homelab"}],
          "content": {
            "type": "link",
            "url": "https://example.com/karakeep-article",
            "title": "Crawled Title",
            "htmlContent": "<p>Crawled body.</p>",
            "crawledAt": "2026-03-04T05:07:00.000Z"
          }
        },
        {
          "id": "def456",
          "createdAt": "2026-03-05T00:00:00.000Z",
          "title": null,
          "tags": [],
          "content": {"type": "text", "text": "a plain text note"}
        }
      ],
      "nextCursor": "v2:1709528827000_def456"
    }"#;

    #[test]
    fn parses_links_and_skips_text_notes() {
        let page = parse_bookmarks_page(FIXTURE).unwrap();
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.skipped_non_links, 1);
        assert_eq!(page.next_cursor.as_deref(), Some("v2:1709528827000_def456"));
        let item = &page.items[0];
        assert_eq!(item.title, "Curator Title");
        assert_eq!(item.origin, "karakeep-api");
        assert_eq!(item.tags, vec!["homelab"]);
        assert_eq!(item.note.as_deref(), Some("worth keeping"));
        assert!(item.html.as_deref().unwrap().contains("Crawled body"));
    }

    #[test]
    fn falls_back_to_crawled_title() {
        let page = parse_bookmarks_page(
            r#"{"bookmarks": [{"id": "x", "title": null,
                "content": {"type": "link", "url": "https://e.com/a", "title": "Crawled"}}]}"#,
        )
        .unwrap();
        assert_eq!(page.items[0].title, "Crawled");
        assert_eq!(page.next_cursor, None);
    }
}
