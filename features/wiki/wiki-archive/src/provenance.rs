//! Provenance frontmatter on archived source pages +
//! slug/filename derivation.
//!
//! Every archived source under `Wiki/raw/sources/` opens with
//! a YAML block recording where the bytes came from and how:
//!
//! ```yaml
//! ---
//! title: "How to do X"
//! source_url: "https://example.com/blog/x?utm_source=pocket"
//! canonical_url: "https://example.com/blog/x"
//! content_type: article            # article|document|video|bookmark
//! archived_at: 2026-06-11T20:01:02Z
//! extractor: dom_smoothie          # which path produced the text
//! media: "https://www.youtube.com/watch?v=…"   # optional (videos)
//! duration: 3622                   # optional, seconds (videos)
//! ---
//! ```
//!
//! The filename embeds the canonical-URL hash —
//! `<title-slug>-<canon8>.md` — so dedup is a pure filename
//! scan (`find_canonical_match`): no frontmatter parsing, no
//! server round-trips per existing source. wiki-live's
//! byte-level sha dedup still backstops identical re-archives.

use chrono::{DateTime, SecondsFormat, Utc};
use sha2::{Digest, Sha256};

/// Where an archived source came from. Composed into the
/// frontmatter at the top of the `raw/sources/` markdown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provenance {
    /// Human title (page `<title>`, video title, bookmark
    /// label). Used for the slug + the ingest log.
    pub title: String,
    /// URL exactly as the user/importer supplied it.
    pub source_url: String,
    /// Router-normalized dedup key — see [`crate::router::canonicalize`].
    pub canonical_url: String,
    /// `article` | `document` | `video` | `bookmark`.
    pub content_type: String,
    pub archived_at: DateTime<Utc>,
    /// Which extractor produced the body (`dom_smoothie`,
    /// `gdocs-export`, `yt-dlp`, `readwise-v2`, …). Honest
    /// provenance for debugging extraction quality later.
    pub extractor: String,
    /// Playable original for media sources — what the
    /// SourceViewer embeds (canonical watch URL today;
    /// `media/archive/<sha>` paths once binary originals
    /// land in phase 2).
    pub media: Option<String>,
    /// Media duration in whole seconds.
    pub duration_secs: Option<u64>,
    /// `unarchived` when extraction failed and only a stub
    /// was stored (phase-3 accept-fragility routes). Stub
    /// FILENAMES also carry an `unarchived-` prefix so the
    /// retry sweep is a filename scan, like dedup.
    pub archive_status: Option<String>,
    /// Why the last extraction attempt failed — honest UX on
    /// the source page itself.
    pub archive_error: Option<String>,
}

impl Provenance {
    /// New provenance with the optional fields empty.
    #[must_use]
    pub fn new(
        title: impl Into<String>,
        source_url: impl Into<String>,
        canonical_url: impl Into<String>,
        content_type: impl Into<String>,
        extractor: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            source_url: source_url.into(),
            canonical_url: canonical_url.into(),
            content_type: content_type.into(),
            archived_at: Utc::now(),
            extractor: extractor.into(),
            media: None,
            duration_secs: None,
            archive_status: None,
            archive_error: None,
        }
    }

    /// First 8 hex chars of the canonical URL's sha256.
    #[must_use]
    pub fn canon8(&self) -> String {
        canon_hash8(&self.canonical_url)
    }

    /// `<title-slug>-<canon8>` — deterministic across
    /// importers because the hash half only depends on the
    /// canonical URL.
    #[must_use]
    pub fn slug(&self) -> String {
        let base = slugify(&self.title);
        let base = if base.is_empty() {
            slugify(&self.canonical_url)
        } else {
            base
        };
        format!("{base}-{}", self.canon8())
    }

    /// Filename under `Wiki/raw/sources/`.
    #[must_use]
    pub fn filename(&self) -> String {
        format!("{}.md", self.slug())
    }

    /// Render the YAML frontmatter block (including the
    /// `---` fences, with a trailing newline).
    #[must_use]
    pub fn frontmatter(&self) -> String {
        let mut out = String::from("---\n");
        out.push_str(&format!("title: {}\n", yaml_quote(&self.title)));
        out.push_str(&format!("source_url: {}\n", yaml_quote(&self.source_url)));
        out.push_str(&format!(
            "canonical_url: {}\n",
            yaml_quote(&self.canonical_url)
        ));
        out.push_str(&format!("content_type: {}\n", self.content_type));
        out.push_str(&format!(
            "archived_at: {}\n",
            self.archived_at.to_rfc3339_opts(SecondsFormat::Secs, true)
        ));
        out.push_str(&format!("extractor: {}\n", yaml_quote(&self.extractor)));
        if let Some(media) = &self.media {
            out.push_str(&format!("media: {}\n", yaml_quote(media)));
        }
        if let Some(d) = self.duration_secs {
            out.push_str(&format!("duration: {d}\n"));
        }
        if let Some(s) = &self.archive_status {
            out.push_str(&format!("archive_status: {s}\n"));
        }
        if let Some(e) = &self.archive_error {
            out.push_str(&format!("archive_error: {}\n", yaml_quote(e)));
        }
        out.push_str("---\n");
        out
    }
}

/// Frontmatter + `# title` heading + extracted body.
#[must_use]
pub fn compose_source_markdown(prov: &Provenance, body: &str) -> String {
    format!(
        "{}\n# {}\n\n{}\n",
        prov.frontmatter(),
        prov.title,
        body.trim()
    )
}

/// First 8 hex chars of sha256(canonical_url).
#[must_use]
pub fn canon_hash8(canonical_url: &str) -> String {
    let mut h = Sha256::new();
    h.update(canonical_url.as_bytes());
    format!("{:x}", h.finalize())[..8].to_string()
}

/// Canonical-URL dedup: does any existing `raw/sources/`
/// filename carry this canonical hash? Filenames are
/// `<slug>-<canon8>.md`, so a suffix match is exact — title
/// drift between importers can't defeat it.
pub fn find_canonical_match<'a, I>(filenames: I, canon8: &str) -> Option<&'a str>
where
    I: IntoIterator<Item = &'a str>,
{
    let suffix = format!("-{canon8}.md");
    filenames.into_iter().find(|f| f.ends_with(&suffix))
}

/// Lowercase-ASCII slug, runs of non-alphanumerics collapsed
/// to single `-`, capped at 60 chars (filesystem hygiene —
/// the hash carries the uniqueness).
#[must_use]
pub fn slugify(s: &str) -> String {
    let mut out = String::new();
    let mut prev_dash = true; // suppress leading dash
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            prev_dash = false;
        } else if !prev_dash {
            out.push('-');
            prev_dash = true;
        }
        if out.len() >= 60 {
            break;
        }
    }
    out.trim_matches('-').to_string()
}

/// `[mm:ss]` (or `[h:mm:ss]` past the hour) display form for
/// a `^t<seconds>` anchor — shared by the transcript writer,
/// the notes convention, and the prompt examples.
#[must_use]
pub fn mmss(total_secs: u64) -> String {
    let h = total_secs / 3600;
    let m = (total_secs % 3600) / 60;
    let s = total_secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Double-quoted YAML scalar with `\` and `"` escaped.
fn yaml_quote(s: &str) -> String {
    let escaped = s.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{escaped}\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prov() -> Provenance {
        Provenance::new(
            "How to do \"X\" — properly",
            "https://example.com/blog/x?utm_source=pocket",
            "https://example.com/blog/x",
            "article",
            "dom_smoothie",
        )
    }

    #[test]
    fn slug_is_title_plus_canon_hash() {
        let p = prov();
        let slug = p.slug();
        assert!(slug.starts_with("how-to-do-x-properly-"), "{slug}");
        assert_eq!(slug.len(), "how-to-do-x-properly".len() + 9);
        // Same canonical URL + different title (Pocket vs
        // Readwise titles drift) ⇒ same hash suffix.
        let mut p2 = prov();
        p2.title = "How To Do X, Properly!".into();
        assert_eq!(p.canon8(), p2.canon8());
    }

    #[test]
    fn frontmatter_quotes_and_optionals() {
        let mut p = prov();
        p.media = Some("https://www.youtube.com/watch?v=abc123xyz".into());
        p.duration_secs = Some(3622);
        let fm = p.frontmatter();
        assert!(fm.starts_with("---\n"));
        assert!(
            fm.contains(r#"title: "How to do \"X\" — properly""#),
            "{fm}"
        );
        assert!(fm.contains("content_type: article"));
        assert!(fm.contains("duration: 3622"));
        assert!(fm.contains("media: \"https://www.youtube.com/watch?v=abc123xyz\""));
        assert!(fm.ends_with("---\n"));
    }

    #[test]
    fn optionals_omitted_when_absent() {
        let fm = prov().frontmatter();
        assert!(!fm.contains("media:"));
        assert!(!fm.contains("duration:"));
        assert!(!fm.contains("archive_status:"));
        assert!(!fm.contains("archive_error:"));
    }

    #[test]
    fn unarchived_stub_fields_render() {
        let mut p = prov();
        p.archive_status = Some("unarchived".into());
        p.archive_error = Some("HTTP 403 — blocked".into());
        let fm = p.frontmatter();
        assert!(fm.contains("archive_status: unarchived"), "{fm}");
        assert!(fm.contains("archive_error: \"HTTP 403 — blocked\""), "{fm}");
    }

    #[test]
    fn canonical_match_ignores_title_drift() {
        let p = prov();
        let existing = [
            "some-other-page-aaaaaaaa.md".to_string(),
            format!("totally-different-title-{}.md", p.canon8()),
        ];
        let hit = find_canonical_match(existing.iter().map(String::as_str), &p.canon8());
        assert_eq!(hit, Some(existing[1].as_str()));
        assert_eq!(
            find_canonical_match(existing.iter().map(String::as_str), "deadbeef"),
            None
        );
    }

    #[test]
    fn slugify_collapses_and_caps() {
        assert_eq!(slugify("Hello,  World! — 2026"), "hello-world-2026");
        assert_eq!(slugify("___"), "");
        let long = "a".repeat(100);
        assert!(slugify(&long).len() <= 60);
    }

    #[test]
    fn mmss_formats() {
        assert_eq!(mmss(0), "0:00");
        assert_eq!(mmss(75), "1:15");
        assert_eq!(mmss(3622), "1:00:22");
    }

    #[test]
    fn compose_has_fm_heading_body() {
        let md = compose_source_markdown(&prov(), "Body text.\n");
        assert!(md.contains("---\n\n# How to do \"X\" — properly\n\nBody text.\n"));
    }
}
