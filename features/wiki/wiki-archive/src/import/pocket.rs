//! Pocket export importer. The service is DEAD (shut down
//! 2025) — only the takeout zip exists, so this is a pure
//! file parser: `part_000000.csv` (+ siblings) for the saves,
//! `annotations/*.json` for highlights.
//!
//! CSV is parsed by HEADER NAME, not position — Pocket
//! shipped at least two layouts (the final
//! `title,url,time_added,tags,status` and a legacy 6-column
//! variant with a cursor column) and the tags column is
//! pipe-separated.

use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use chrono::{DateTime, Utc};
use serde_json::Value;

use super::{Highlight, ImportedItem};
use crate::ArchiveError;

/// Parse a whole export zip from disk.
pub fn import_zip(path: &Path) -> Result<Vec<ImportedItem>, ArchiveError> {
    let file = std::fs::File::open(path)?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| ArchiveError::ImportParse(format!("pocket zip: {e}")))?;
    let mut csv_bodies: Vec<String> = Vec::new();
    let mut annotation_bodies: Vec<String> = Vec::new();
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| ArchiveError::ImportParse(format!("pocket zip entry: {e}")))?;
        let name = entry.name().to_ascii_lowercase();
        let ext = Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_string();
        let mut body = String::new();
        if ext == "csv" {
            entry.read_to_string(&mut body)?;
            csv_bodies.push(body);
        } else if ext == "json" && name.contains("annotations") {
            entry.read_to_string(&mut body)?;
            annotation_bodies.push(body);
        }
    }
    if csv_bodies.is_empty() {
        return Err(ArchiveError::ImportParse(
            "pocket zip: no part_*.csv inside — is this a Pocket export?".into(),
        ));
    }
    let mut items = Vec::new();
    for body in &csv_bodies {
        items.extend(parse_csv(body)?);
    }
    let mut highlights: HashMap<String, Vec<Highlight>> = HashMap::new();
    for body in &annotation_bodies {
        for (url, hs) in parse_annotations(body)? {
            highlights.entry(url).or_default().extend(hs);
        }
    }
    for item in &mut items {
        if let Some(hs) = highlights.remove(&item.url) {
            item.highlights = hs;
        }
    }
    Ok(items)
}

/// Parse one Pocket CSV part. Header-name driven; tolerates
/// extra/reordered columns (the legacy 6-col layout).
pub fn parse_csv(body: &str) -> Result<Vec<ImportedItem>, ArchiveError> {
    let mut reader = csv::ReaderBuilder::new()
        .flexible(true)
        .from_reader(body.as_bytes());
    let headers = reader
        .headers()
        .map_err(|e| ArchiveError::ImportParse(format!("pocket csv headers: {e}")))?
        .clone();
    let col = |name: &str| {
        headers
            .iter()
            .position(|h| h.trim().eq_ignore_ascii_case(name))
    };
    let (Some(url_col), Some(title_col)) = (col("url"), col("title")) else {
        return Err(ArchiveError::ImportParse(format!(
            "pocket csv: need `url` + `title` headers, got: {}",
            headers.iter().collect::<Vec<_>>().join(",")
        )));
    };
    let time_col = col("time_added");
    let tags_col = col("tags");
    let status_col = col("status");

    let mut items = Vec::new();
    for record in reader.records() {
        let record =
            record.map_err(|e| ArchiveError::ImportParse(format!("pocket csv row: {e}")))?;
        let Some(url) = record
            .get(url_col)
            .map(str::trim)
            .filter(|u| u.starts_with("http"))
        else {
            continue;
        };
        let tags: Vec<String> = tags_col
            .and_then(|c| record.get(c))
            .map(|t| {
                t.split('|')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        let mut item = ImportedItem {
            url: url.to_string(),
            title: record.get(title_col).unwrap_or_default().trim().to_string(),
            saved_at: time_col
                .and_then(|c| record.get(c))
                .and_then(|s| s.trim().parse::<i64>().ok())
                .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0)),
            tags,
            html: None,
            highlights: Vec::new(),
            note: None,
            origin: "pocket-export".into(),
        };
        // `status` (unread/archive) rides as a tag so the
        // wiki keeps the read-state signal without a schema
        // field for a dead service.
        if let Some(status) = status_col
            .and_then(|c| record.get(c))
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            item.tags.push(format!("pocket/{status}"));
        }
        items.push(item);
    }
    Ok(items)
}

/// Parse one `annotations/*.json` body → highlights keyed by
/// item URL. Tolerant of the two recorded spellings
/// (`item_url`/`url`, `quote`/`text`) — the export format was
/// never versioned and the service can no longer be asked.
pub fn parse_annotations(body: &str) -> Result<Vec<(String, Vec<Highlight>)>, ArchiveError> {
    let v: Value = serde_json::from_str(body)
        .map_err(|e| ArchiveError::ImportParse(format!("pocket annotations: {e}")))?;
    // Either a bare array or `{"annotations": [...]}`.
    let arr = v
        .as_array()
        .cloned()
        .or_else(|| v.get("annotations").and_then(Value::as_array).cloned())
        .unwrap_or_default();
    let mut by_url: HashMap<String, Vec<Highlight>> = HashMap::new();
    for a in &arr {
        let url = a
            .get("item_url")
            .or_else(|| a.get("url"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let quote = a
            .get("quote")
            .or_else(|| a.get("text"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .trim()
            .to_string();
        if url.is_empty() || quote.is_empty() {
            continue;
        }
        by_url.entry(url.to_string()).or_default().push(Highlight {
            text: quote,
            note: a
                .get("note")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(ToString::to_string),
        });
    }
    Ok(by_url.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CSV_FIXTURE: &str = "\
title,url,time_added,tags,status
\"A Post, With Comma\",https://example.com/a,1700000000,rust|wiki,unread
Plain Post,https://example.com/b,1700000100,,archive
not-a-url-row,about:blank,0,,unread
";

    /// Legacy export layout: extra `cursor` column, shuffled.
    const CSV_LEGACY_FIXTURE: &str = "\
url,title,time_added,cursor,tags,status
https://example.com/old,Old Post,1500000000,abc123,history,archive
";

    const ANNOTATIONS_FIXTURE: &str = r#"[
      {"item_url": "https://example.com/a", "quote": "A great quote.",
       "created_at": "2023-11-14T00:00:00Z"},
      {"url": "https://example.com/a", "text": "Second form.", "note": "check"},
      {"quote": "orphan without url"}
    ]"#;

    #[test]
    fn csv_parses_by_header_name() {
        let items = parse_csv(CSV_FIXTURE).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "A Post, With Comma");
        assert_eq!(items[0].tags, vec!["rust", "wiki", "pocket/unread"]);
        assert_eq!(items[0].saved_at.unwrap().timestamp(), 1_700_000_000);
        assert_eq!(items[1].tags, vec!["pocket/archive"]);
    }

    #[test]
    fn legacy_six_column_layout_parses() {
        let items = parse_csv(CSV_LEGACY_FIXTURE).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Old Post");
        assert_eq!(items[0].tags, vec!["history", "pocket/archive"]);
    }

    #[test]
    fn missing_headers_error_clearly() {
        let err = parse_csv("a,b\n1,2\n").unwrap_err();
        assert!(err.to_string().contains("need `url` + `title`"), "{err}");
    }

    #[test]
    fn annotations_parse_both_spellings() {
        let by_url = parse_annotations(ANNOTATIONS_FIXTURE).unwrap();
        assert_eq!(by_url.len(), 1);
        let (url, hs) = &by_url[0];
        assert_eq!(url, "https://example.com/a");
        assert_eq!(hs.len(), 2);
        assert_eq!(hs[1].note.as_deref(), Some("check"));
    }

    #[test]
    fn zip_roundtrip() {
        use std::io::Write;
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
            let opts: zip::write::SimpleFileOptions = Default::default();
            w.start_file("part_000000.csv", opts).unwrap();
            w.write_all(CSV_FIXTURE.as_bytes()).unwrap();
            w.start_file("annotations/part_000000.json", opts).unwrap();
            w.write_all(ANNOTATIONS_FIXTURE.as_bytes()).unwrap();
            w.finish().unwrap();
        }
        let dir = std::env::temp_dir().join("wiki-archive-pocket-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("export.zip");
        std::fs::write(&path, &buf).unwrap();
        let items = import_zip(&path).unwrap();
        assert_eq!(items.len(), 2);
        // Highlights joined onto the matching item by URL.
        assert_eq!(items[0].highlights.len(), 2);
        assert!(items[1].highlights.is_empty());
        std::fs::remove_file(&path).ok();
    }
}
