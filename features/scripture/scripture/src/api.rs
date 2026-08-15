//! API-backed editions — the copyright-restricted translations (ESV,
//! NIV) we can't bundle, fetched per-chapter over HTTP with the user's
//! key.
//!
//! Two providers:
//! - **ESV** (`api.esv.org`, Crossway) — free non-commercial key. The
//!   text endpoint returns the passage with `[N]` verse markers.
//! - **API.Bible** (`scripture.api.bible`, American Bible Society) — a
//!   generic provider keyed by a per-edition `bible_id`. This is the
//!   realistic route for **NIV**, *if* the user's key has NIV access —
//!   it's tightly licensed and may not be on a given key/tier.
//!
//! Nothing is persisted: each read fetches live (keeping us inside the
//! ESV / API.Bible caching terms). Verse-number parsing is pure and
//! unit-tested; the HTTP plumbing is thin.

use std::sync::LazyLock;

use regex::Regex;
use scripture_proto::{Book, ScriptureError, VerseId, VerseLine};
use serde::Deserialize;

/// Which upstream service serves an edition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    /// Crossway ESV API (`api.esv.org`).
    Esv,
    /// American Bible Society API.Bible, keyed by the edition's id.
    ApiBible { bible_id: String },
}

/// A copyright-restricted edition fetched over HTTP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiTranslation {
    pub id: String,
    pub name: String,
    pub license: String,
    pub provider: Provider,
    pub key: String,
}

impl ApiTranslation {
    /// The ESV via Crossway's API.
    #[must_use]
    pub fn esv(key: impl Into<String>) -> Self {
        Self {
            id: "ESV".into(),
            name: "English Standard Version".into(),
            license: "© Crossway — ESV API (non-commercial)".into(),
            provider: Provider::Esv,
            key: key.into(),
        }
    }

    /// An API.Bible edition (e.g. NIV) by its `bible_id`.
    #[must_use]
    pub fn api_bible(
        id: impl Into<String>,
        name: impl Into<String>,
        bible_id: impl Into<String>,
        key: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            license: "© publisher — fetched via API.Bible".into(),
            provider: Provider::ApiBible {
                bible_id: bible_id.into(),
            },
            key: key.into(),
        }
    }
}

fn upstream(e: impl std::fmt::Display) -> ScriptureError {
    ScriptureError::Upstream(e.to_string())
}

/// Fetch one chapter of an API edition as clean verse lines.
pub async fn fetch_chapter(
    http: &reqwest::Client,
    tx: &ApiTranslation,
    book: Book,
    chapter: u16,
) -> Result<Vec<VerseLine>, ScriptureError> {
    let raw = match &tx.provider {
        Provider::Esv => esv_passage(http, &tx.key, &format!("{} {chapter}", book.name())).await?,
        Provider::ApiBible { bible_id } => {
            api_bible_chapter(
                http,
                &tx.key,
                bible_id,
                &format!("{}.{chapter}", book.usfm()),
            )
            .await?
        }
    };
    Ok(parse_bracketed_verses(&raw, book, chapter))
}

// ── ESV ──────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct EsvResp {
    #[serde(default)]
    passages: Vec<String>,
}

async fn esv_passage(
    http: &reqwest::Client,
    key: &str,
    query: &str,
) -> Result<String, ScriptureError> {
    let resp = http
        .get("https://api.esv.org/v3/passage/text/")
        .header("Authorization", format!("Token {key}"))
        .query(&[
            ("q", query),
            ("include-verse-numbers", "true"),
            ("include-headings", "false"),
            ("include-footnotes", "false"),
            ("include-passage-references", "false"),
            ("include-short-copyright", "false"),
            ("include-passage-horizontal-lines", "false"),
            ("include-heading-horizontal-lines", "false"),
        ])
        .send()
        .await
        .map_err(upstream)?;
    if !resp.status().is_success() {
        return Err(ScriptureError::Upstream(format!(
            "ESV API status {}",
            resp.status()
        )));
    }
    let body: EsvResp = resp.json().await.map_err(upstream)?;
    body.passages
        .into_iter()
        .next()
        .ok_or_else(|| ScriptureError::NotFound("ESV passage empty".into()))
}

// ── API.Bible ────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct ApiBibleResp {
    data: ApiBibleData,
}
#[derive(Deserialize)]
struct ApiBibleData {
    #[serde(default)]
    content: String,
}

async fn api_bible_chapter(
    http: &reqwest::Client,
    key: &str,
    bible_id: &str,
    chapter_id: &str,
) -> Result<String, ScriptureError> {
    let url = format!("https://api.scripture.api.bible/v1/bibles/{bible_id}/chapters/{chapter_id}");
    let resp = http
        .get(url)
        .header("api-key", key)
        .query(&[
            ("content-type", "text"),
            ("include-verse-numbers", "true"),
            ("include-notes", "false"),
            ("include-titles", "false"),
            ("include-chapter-numbers", "false"),
            ("include-verse-spans", "false"),
        ])
        .send()
        .await
        .map_err(upstream)?;
    if !resp.status().is_success() {
        return Err(ScriptureError::Upstream(format!(
            "API.Bible status {}",
            resp.status()
        )));
    }
    let body: ApiBibleResp = resp.json().await.map_err(upstream)?;
    Ok(body.data.content)
}

// ── Parsing ──────────────────────────────────────────────────────────

/// Split passage text with `[N]` verse markers into clean verse lines.
/// Both ESV and API.Bible (text mode, verse numbers on) emit this shape.
fn parse_bracketed_verses(text: &str, book: Book, chapter: u16) -> Vec<VerseLine> {
    static MARK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(\d+)\]").unwrap());
    static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

    let marks: Vec<(usize, usize, u16)> = MARK
        .captures_iter(text)
        .filter_map(|c| {
            let m = c.get(0)?;
            let n = c.get(1)?.as_str().parse().ok()?;
            Some((m.start(), m.end(), n))
        })
        .collect();

    let mut out = Vec::with_capacity(marks.len());
    for (i, &(_, end, verse)) in marks.iter().enumerate() {
        let stop = marks.get(i + 1).map_or(text.len(), |next| next.0);
        let body = WS
            .replace_all(text[end..stop].trim(), " ")
            .trim()
            .to_string();
        if !body.is_empty() {
            out.push(VerseLine {
                verse,
                osis: VerseId::new(book, chapter, verse).osis(),
                text: body,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn john() -> Book {
        Book::lookup("John").unwrap()
    }

    #[test]
    fn parses_esv_style_passage() {
        // Shape of ESV `passage/text` output with verse numbers.
        let passage = "\n\n  [16] For God so loved the world, that he gave his only Son,\n  \
            that whoever believes in him should not perish but have eternal life.\n\n  \
            [17] For God did not send his Son into the world to condemn the world.\n\n";
        let verses = parse_bracketed_verses(passage, john(), 3);
        assert_eq!(verses.len(), 2);
        assert_eq!(verses[0].verse, 16);
        assert_eq!(verses[0].osis, "John.3.16");
        assert!(verses[0].text.starts_with("For God so loved the world"));
        assert!(verses[0].text.ends_with("eternal life."));
        // Whitespace/newlines collapsed to single spaces.
        assert!(!verses[0].text.contains('\n'));
        assert_eq!(verses[1].verse, 17);
    }

    #[test]
    fn empty_passage_yields_no_verses() {
        assert!(parse_bracketed_verses("   \n  ", john(), 3).is_empty());
    }

    #[test]
    fn esv_and_api_bible_constructors() {
        assert_eq!(ApiTranslation::esv("k").provider, Provider::Esv);
        let niv = ApiTranslation::api_bible("NIV", "New International Version", "abc123", "k");
        assert!(matches!(niv.provider, Provider::ApiBible { .. }));
        assert_eq!(niv.id, "NIV");
    }
}
