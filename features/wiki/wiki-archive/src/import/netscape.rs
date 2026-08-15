//! Netscape bookmarks-HTML importer — the `bookmarks.html`
//! every browser exports.
//!
//! The format is famously malformed (a 1990s DTD with
//! unclosed `<DT>`/`<DD>` elements); `scraper` rides
//! html5ever, which error-recovers exactly the way browsers
//! do, so a plain `a[href]` selection sees every bookmark.
//! Firefox/Chrome attributes: `ADD_DATE` (unix secs),
//! `TAGS` (comma-separated, Firefox only), `LAST_MODIFIED`.

use chrono::{DateTime, Utc};
use scraper::{Html, Selector};

use super::ImportedItem;
use crate::ArchiveError;

/// Parse a bookmarks HTML export. Pure — fixture-tested.
pub fn parse_bookmarks_html(html: &str) -> Result<Vec<ImportedItem>, ArchiveError> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("a[href]").expect("static selector");
    let mut items = Vec::new();
    for a in doc.select(&sel) {
        let href = a.value().attr("href").unwrap_or_default().trim();
        if !href.starts_with("http") {
            continue; // place:, javascript:, file: rows
        }
        let title = a.text().collect::<String>().trim().to_string();
        let tags = a
            .value()
            .attr("tags")
            .map(|t| {
                t.split(',')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(ToString::to_string)
                    .collect()
            })
            .unwrap_or_default();
        items.push(ImportedItem {
            url: href.to_string(),
            title,
            saved_at: a
                .value()
                .attr("add_date")
                .and_then(|s| s.trim().parse::<i64>().ok())
                .and_then(|secs| DateTime::<Utc>::from_timestamp(secs, 0)),
            tags,
            html: None,
            highlights: Vec::new(),
            note: None,
            origin: "netscape-html".into(),
        });
    }
    if items.is_empty() {
        return Err(ArchiveError::ImportParse(
            "no http(s) bookmarks found — is this a Netscape bookmarks export?".into(),
        ));
    }
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-world shape: DOCTYPE, unclosed DT/DD, nested
    /// folders, a `place:` smart folder to skip.
    const FIXTURE: &str = r#"<!DOCTYPE NETSCAPE-Bookmark-file-1>
<META HTTP-EQUIV="Content-Type" CONTENT="text/html; charset=UTF-8">
<TITLE>Bookmarks</TITLE>
<H1>Bookmarks Menu</H1>
<DL><p>
    <DT><H3 ADD_DATE="1690000000">Dev</H3>
    <DL><p>
        <DT><A HREF="https://example.com/rust-post" ADD_DATE="1700000000" TAGS="rust,async">Rust Post</A>
        <DD>A description line that never gets a closing tag
        <DT><A HREF="https://example.com/other" ADD_DATE="1700000100">Other & Co.</A>
    </DL><p>
    <DT><A HREF="place:type=6&sort=14">Recent Tags</A>
</DL>"#;

    #[test]
    fn parses_bookmarks_with_unclosed_dts() {
        let items = parse_bookmarks_html(FIXTURE).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Rust Post");
        assert_eq!(items[0].tags, vec!["rust", "async"]);
        assert_eq!(items[0].saved_at.unwrap().timestamp(), 1_700_000_000);
        assert_eq!(items[1].title, "Other & Co.");
        assert_eq!(items[1].origin, "netscape-html");
    }

    #[test]
    fn empty_or_foreign_html_errors() {
        assert!(parse_bookmarks_html("<html><body><p>hi</p></body></html>").is_err());
    }
}
