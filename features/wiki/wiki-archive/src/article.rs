//! Article + Google Docs extraction.
//!
//! Articles: fetch → `dom_smoothie` readability in
//! `TextMode::Markdown` (the maintained readability port —
//! mozilla/readability test parity). When readability finds
//! a content node but markdown conversion comes back thin,
//! fall back to running `htmd` over the cleaned content HTML.
//!
//! Google Docs: the JS app shell is unscrapable — use the
//! `/export?format=md` endpoint instead (anonymous works for
//! link-shared docs; reqwest follows the 307 redirect chain).

use dom_smoothie::{Config, Readability, TextMode};
use url::Url;

use crate::ArchiveError;

/// Browser-ish UA: a surprising number of sites 403 the
/// default `reqwest/x.y` UA at the CDN layer. We still
/// identify ourselves in the suffix.
pub const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/124.0.0.0 Safari/537.36 task-wiki-archive/0.1";

/// Shared HTTP client for all extractors + importers.
pub fn http_client() -> Result<reqwest::Client, ArchiveError> {
    reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(45))
        .build()
        .map_err(|e| ArchiveError::Fetch {
            url: String::new(),
            message: format!("client build: {e}"),
        })
}

/// Extraction result: what the provenance + body composer
/// needs.
#[derive(Debug, Clone)]
pub struct ExtractedArticle {
    pub title: String,
    /// Author line when readability finds one.
    pub byline: Option<String>,
    /// Markdown body.
    pub markdown: String,
}

/// Fetch a URL and run readability extraction.
pub async fn fetch_article(
    client: &reqwest::Client,
    url: &Url,
) -> Result<ExtractedArticle, ArchiveError> {
    let html = fetch_text(client, url.as_str(), "text/html").await?;
    extract_article(&html, url.as_str())
}

/// Pure extraction — unit-testable on fixture HTML.
pub fn extract_article(html: &str, base_url: &str) -> Result<ExtractedArticle, ArchiveError> {
    let cfg = Config {
        text_mode: TextMode::Markdown,
        ..Config::default()
    };
    let mut readability =
        Readability::new(html, Some(base_url), Some(cfg)).map_err(|e| ArchiveError::Extract {
            url: base_url.to_string(),
            message: format!("readability init: {e}"),
        })?;
    let article = readability.parse().map_err(|e| ArchiveError::Extract {
        url: base_url.to_string(),
        message: format!("readability parse: {e}"),
    })?;

    let mut markdown = article.text_content.trim().to_string();
    if markdown.len() < 200 {
        // Thin markdown but a real content node: htmd over the
        // cleaned HTML usually recovers tables/code blocks the
        // text serializer dropped. Only replace when it's a
        // strict improvement.
        if let Ok(alt) = htmd::convert(&article.content) {
            let alt = alt.trim().to_string();
            if alt.len() > markdown.len() {
                markdown = alt;
            }
        }
    }
    if markdown.is_empty() {
        return Err(ArchiveError::Extract {
            url: base_url.to_string(),
            message: "readability produced an empty body".into(),
        });
    }

    let byline = article
        .byline
        .as_deref()
        .map(str::trim)
        .filter(|b| !b.is_empty())
        .map(ToString::to_string);
    Ok(ExtractedArticle {
        title: article.title.trim().to_string(),
        byline,
        markdown,
    })
}

/// Convert reader-clean HTML (Readwise Reader / Karakeep
/// payloads) straight to markdown — no readability pass.
pub fn clean_html_to_markdown(html: &str) -> Result<String, ArchiveError> {
    htmd::convert(html)
        .map(|s| s.trim().to_string())
        .map_err(|e| ArchiveError::Extract {
            url: String::new(),
            message: format!("htmd: {e}"),
        })
}

/// Google Docs markdown export. Returns `(title, markdown)`
/// — title from the `Content-Disposition` filename, falling
/// back to the doc id.
pub async fn fetch_google_doc(
    client: &reqwest::Client,
    doc_id: &str,
) -> Result<(String, String), ArchiveError> {
    let export = format!("https://docs.google.com/document/d/{doc_id}/export?format=md");
    let resp = client
        .get(&export)
        .send()
        .await
        .map_err(|e| ArchiveError::Fetch {
            url: export.clone(),
            message: e.to_string(),
        })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ArchiveError::BadResponse {
            url: export,
            message: format!(
                "HTTP {status} — is the doc link-shared? Private docs need a signed-in export"
            ),
        });
    }
    let title = resp
        .headers()
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .and_then(content_disposition_filename)
        .map_or_else(
            || format!("google-doc-{doc_id}"),
            |f| f.trim_end_matches(".md").to_string(),
        );
    let body = resp.text().await.map_err(|e| ArchiveError::Fetch {
        url: export.clone(),
        message: e.to_string(),
    })?;
    if body.trim().is_empty() {
        return Err(ArchiveError::BadResponse {
            url: export,
            message: "empty export body".into(),
        });
    }
    Ok((title, body))
}

/// GET a URL as text with an `Accept` hint, mapping HTTP
/// errors to [`ArchiveError`].
pub async fn fetch_text(
    client: &reqwest::Client,
    url: &str,
    accept: &str,
) -> Result<String, ArchiveError> {
    let resp = client
        .get(url)
        .header("accept", accept)
        .send()
        .await
        .map_err(|e| ArchiveError::Fetch {
            url: url.to_string(),
            message: e.to_string(),
        })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ArchiveError::BadResponse {
            url: url.to_string(),
            message: format!("HTTP {status}"),
        });
    }
    resp.text().await.map_err(|e| ArchiveError::Fetch {
        url: url.to_string(),
        message: e.to_string(),
    })
}

/// GET a URL as raw bytes, returning `(content_type, bytes)`.
/// The archive front door uses the content type (plus magic
/// bytes) to divert e.g. extensionless PDFs away from the
/// readability path.
pub async fn fetch_bytes(
    client: &reqwest::Client,
    url: &str,
    accept: &str,
) -> Result<(String, Vec<u8>), ArchiveError> {
    let resp = client
        .get(url)
        .header("accept", accept)
        .send()
        .await
        .map_err(|e| ArchiveError::Fetch {
            url: url.to_string(),
            message: e.to_string(),
        })?;
    let status = resp.status();
    if !status.is_success() {
        return Err(ArchiveError::BadResponse {
            url: url.to_string(),
            message: format!("HTTP {status}"),
        });
    }
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| ArchiveError::Fetch {
            url: url.to_string(),
            message: e.to_string(),
        })?
        .to_vec();
    Ok((content_type, bytes))
}

/// `attachment; filename="My Doc.md"` → `My Doc.md`. Handles
/// the unquoted variant too; skips RFC 5987 `filename*=`
/// (Google doesn't emit it for doc exports).
fn content_disposition_filename(header: &str) -> Option<String> {
    let idx = header.find("filename=")?;
    let rest = &header[idx + "filename=".len()..];
    let rest = rest.trim();
    let name = if let Some(stripped) = rest.strip_prefix('"') {
        stripped.split('"').next()?
    } else {
        rest.split(';').next()?.trim()
    };
    if name.is_empty() {
        None
    } else {
        Some(name.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = r#"<!doctype html>
<html><head><title>Test Post — Example Blog</title>
<meta name="author" content="Jane Doe"></head>
<body>
<nav><a href="/">home</a><a href="/about">about</a></nav>
<article>
  <h1>Test Post</h1>
  <p class="byline">By Jane Doe</p>
  <p>This is the first paragraph of the article body. It needs to be long
  enough that readability's scorer treats it as real content rather than
  boilerplate, so here is some additional filler prose about Rust,
  archiving, and provenance frontmatter conventions.</p>
  <p>Second paragraph with a <a href="/relative/link">relative link</a> and
  some <strong>bold text</strong> to convert. More filler so the content
  scorer keeps this node: extraction quality matters more than brevity in
  this fixture, which is why it rambles on.</p>
  <pre><code>fn main() { println!("hi"); }</code></pre>
</article>
<footer>© 2026 — subscribe to the newsletter!</footer>
</body></html>"#;

    #[test]
    fn extracts_title_and_body_markdown() {
        let a = extract_article(FIXTURE, "https://example.com/blog/test-post").unwrap();
        assert!(a.title.contains("Test Post"), "{}", a.title);
        assert!(a.markdown.contains("first paragraph"), "{}", a.markdown);
        // Boilerplate stripped (nav + footer).
        assert!(!a.markdown.contains("newsletter"));
        assert!(!a.markdown.contains("[home]"));
    }

    #[test]
    fn empty_page_is_an_error() {
        let err = extract_article("<html><body></body></html>", "https://example.com/x");
        assert!(err.is_err());
    }

    #[test]
    fn clean_html_converts() {
        let md = clean_html_to_markdown("<h2>Hi</h2><p>Para with <em>em</em>.</p>").unwrap();
        assert!(md.contains("## Hi"));
        assert!(md.contains("*em*"));
    }

    #[test]
    fn content_disposition_parses() {
        assert_eq!(
            content_disposition_filename(r#"attachment; filename="My Doc.md""#),
            Some("My Doc.md".to_string())
        );
        assert_eq!(
            content_disposition_filename("attachment; filename=plain.md; size=3"),
            Some("plain.md".to_string())
        );
        assert_eq!(content_disposition_filename("attachment"), None);
    }
}
