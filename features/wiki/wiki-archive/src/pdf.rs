//! PDF text extraction with per-page `^p<page>` anchors.
//!
//! Two engines, tried in order:
//!
//! 1. **pdfium** (feature `pdf`, same flag-and-dep shape as
//!    `wiki-extract`): pdfium-render binds a `libpdfium`
//!    DYNAMICALLY at runtime — nothing is bundled. The
//!    library is looked up in `TASK_PDFIUM` (a directory)
//!    first, then the system library path. Grab a prebuilt
//!    from <https://github.com/bblanchon/pdfium-binaries>.
//! 2. **`pdftotext -layout`** (poppler) as a subprocess
//!    fallback when pdfium isn't compiled in or can't bind.
//!    Reads the PDF from stdin, emits form-feed-separated
//!    pages on stdout.
//!
//! OCR for scanned/image-only pages is explicitly deferred —
//! a page with no extractable text renders an honest
//! `(no extractable text on this page)` block instead of
//! silently vanishing, so the gap is visible in the archive.
//!
//! ## The `^p<page>` convention
//!
//! Mirrors the `^t<seconds>` transcript convention: the FIRST
//! paragraph of each page ends with a `^p<page>` block anchor
//! (1-based, matching `ExtractedImage.page + 1` in
//! wiki-extract and what humans call "page N"). Legal under
//! the existing obsidian block-id grammar, so
//! `[[Sources/x#^p12]]` deep links work with zero parser
//! changes.

use std::time::Duration;

use crate::ArchiveError;

/// How long one `pdftotext` run may take before we kill it.
const PDFTOTEXT_TIMEOUT: Duration = Duration::from_secs(60);

/// Per-page extracted text, in page order. Pages with no
/// text stay as empty strings (the renderer marks them).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfText {
    /// Document-metadata title, when the PDF carries one.
    pub title: Option<String>,
    pub pages: Vec<String>,
}

/// Extract per-page text, preferring pdfium, falling back to
/// `pdftotext`. Returns the text and which engine produced it
/// (`pdfium` / `pdftotext`) for the provenance `extractor`
/// field.
pub async fn extract_pdf(
    bytes: &[u8],
    pdftotext_binary: &str,
) -> Result<(PdfText, &'static str), ArchiveError> {
    #[cfg(feature = "pdf")]
    {
        match extract_with_pdfium(bytes) {
            Ok(text) => return Ok((text, "pdfium")),
            Err(e) => {
                tracing::warn!(error = %e, "pdfium unavailable/failed; falling back to pdftotext");
            }
        }
    }
    let text = extract_with_pdftotext(bytes, pdftotext_binary).await?;
    Ok((text, "pdftotext"))
}

/// pdfium-render extraction. Binding order: `TASK_PDFIUM`
/// directory, then the system library. Runtime-soft: a
/// missing library is an `Err`, not a panic — the caller
/// falls back to pdftotext.
#[cfg(feature = "pdf")]
pub fn extract_with_pdfium(bytes: &[u8]) -> Result<PdfText, ArchiveError> {
    use pdfium_render::prelude::*;

    let bindings = match std::env::var("TASK_PDFIUM") {
        Ok(dir) if !dir.is_empty() => {
            Pdfium::bind_to_library(Pdfium::pdfium_platform_library_name_at_path(&dir))
        }
        _ => Pdfium::bind_to_system_library(),
    }
    .map_err(|e| ArchiveError::Extract {
        url: String::new(),
        message: format!(
            "pdfium bind: {e} — set TASK_PDFIUM to a directory containing libpdfium \
             (prebuilts: github.com/bblanchon/pdfium-binaries)"
        ),
    })?;
    let pdfium = Pdfium::new(bindings);
    let document =
        pdfium
            .load_pdf_from_byte_slice(bytes, None)
            .map_err(|e| ArchiveError::Extract {
                url: String::new(),
                message: format!("pdfium load: {e}"),
            })?;

    let title = document
        .metadata()
        .get(PdfDocumentMetadataTagType::Title)
        .map(|tag| tag.value().trim().to_string())
        .filter(|t| !t.is_empty());

    let mut pages = Vec::new();
    for page in document.pages().iter() {
        let text = page.text().map(|t| t.all()).unwrap_or_default();
        pages.push(text);
    }
    if pages.is_empty() {
        return Err(ArchiveError::Extract {
            url: String::new(),
            message: "pdfium: zero pages".into(),
        });
    }
    Ok(PdfText { title, pages })
}

/// `pdftotext -layout - -` subprocess fallback: PDF in via
/// stdin, form-feed-separated pages out via stdout.
pub async fn extract_with_pdftotext(bytes: &[u8], binary: &str) -> Result<PdfText, ArchiveError> {
    use tokio::io::AsyncWriteExt;

    let mut child = tokio::process::Command::new(binary)
        .args(["-layout", "-", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| ArchiveError::Subprocess {
            program: binary.to_string(),
            message: if e.kind() == std::io::ErrorKind::NotFound {
                "binary not found — install poppler-utils, or enable the pdfium path \
                 (build with `--features pdf` + set TASK_PDFIUM)"
                    .to_string()
            } else {
                format!("spawn: {e}")
            },
        })?;
    let mut stdin = child.stdin.take().expect("piped stdin");
    let bytes_owned = bytes.to_vec();
    let writer = tokio::spawn(async move {
        let _ = stdin.write_all(&bytes_owned).await;
        // Drop closes the pipe so pdftotext sees EOF.
    });
    let out = tokio::time::timeout(PDFTOTEXT_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| ArchiveError::Subprocess {
            program: binary.to_string(),
            message: format!("timed out after {}s", PDFTOTEXT_TIMEOUT.as_secs()),
        })?
        .map_err(|e| ArchiveError::Subprocess {
            program: binary.to_string(),
            message: format!("wait: {e}"),
        })?;
    writer.abort();
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(ArchiveError::Subprocess {
            program: binary.to_string(),
            message: format!(
                "exit {}: {}",
                out.status,
                stderr.lines().last().unwrap_or("(no stderr)")
            ),
        });
    }
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    let pages = split_form_feed_pages(&text);
    if pages.is_empty() {
        return Err(ArchiveError::Extract {
            url: String::new(),
            message: "pdftotext produced no pages".into(),
        });
    }
    Ok(PdfText { title: None, pages })
}

/// pdftotext separates pages with `\x0c`; the final page has
/// a trailing one too. Pure — fixture-tested.
#[must_use]
pub fn split_form_feed_pages(text: &str) -> Vec<String> {
    let mut pages: Vec<String> = text.split('\x0c').map(ToString::to_string).collect();
    // Trailing form feed leaves one empty tail entry — drop
    // it (but keep interior empty pages: those are real blank
    // or image-only pages and the renderer marks them).
    if pages.last().is_some_and(|p| p.trim().is_empty()) {
        pages.pop();
    }
    pages
}

/// Render the raw-source body: each page's paragraphs, the
/// first paragraph of every page carrying the `^p<n>` anchor
/// (1-based). Image-only/blank pages render an honest
/// placeholder so the OCR gap is visible.
#[must_use]
pub fn render_pdf_markdown(pages: &[String]) -> String {
    let mut out = String::new();
    for (i, page) in pages.iter().enumerate() {
        let n = i + 1;
        let paragraphs = page_paragraphs(page);
        match paragraphs.split_first() {
            None => {
                out.push_str(&format!(
                    "[p. {n}] (no extractable text on this page — OCR not yet supported) ^p{n}\n\n"
                ));
            }
            Some((first, rest)) => {
                out.push_str(&format!("[p. {n}] {first} ^p{n}\n\n"));
                for para in rest {
                    out.push_str(para);
                    out.push_str("\n\n");
                }
            }
        }
    }
    out.trim_end().to_string()
}

/// Split one page's text into paragraphs: blank-line
/// separated, hard-wrapped lines re-joined with single
/// spaces (pdftotext `-layout` pads columns — collapse runs).
fn page_paragraphs(page: &str) -> Vec<String> {
    let mut paras: Vec<String> = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for line in page.lines() {
        let collapsed = line.split_whitespace().collect::<Vec<_>>().join(" ");
        if collapsed.is_empty() {
            if !current.is_empty() {
                paras.push(current.join(" "));
                current.clear();
            }
        } else {
            current.push(collapsed);
        }
    }
    if !current.is_empty() {
        paras.push(current.join(" "));
    }
    paras
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn form_feed_split_keeps_interior_blank_pages() {
        let text = "page one text\x0c\x0cpage three\x0c";
        let pages = split_form_feed_pages(text);
        assert_eq!(pages.len(), 3);
        assert!(pages[1].trim().is_empty());
        assert_eq!(pages[2], "page three");
    }

    #[test]
    fn render_anchors_first_paragraph_of_each_page() {
        let pages = vec![
            "Title line\nwrapped  with   layout    padding\n\nSecond para.".to_string(),
            String::new(),
            "Third page text.".to_string(),
        ];
        let md = render_pdf_markdown(&pages);
        assert!(
            md.contains("[p. 1] Title line wrapped with layout padding ^p1\n"),
            "{md}"
        );
        assert!(md.contains("\nSecond para.\n"), "{md}");
        // Blank page is honestly marked, not skipped — and
        // still anchored so [[…#^p2]] resolves.
        assert!(md.contains("[p. 2] (no extractable text"), "{md}");
        assert!(md.contains("^p2\n"), "{md}");
        assert!(md.contains("[p. 3] Third page text. ^p3"), "{md}");
    }

    #[test]
    fn anchors_are_block_id_legal() {
        let pages = vec!["A.".to_string(), "B.".to_string()];
        let md = render_pdf_markdown(&pages);
        for line in md.lines().filter(|l| l.contains("^p")) {
            let (_, id) = line.rsplit_once(" ^p").expect("anchor at line end");
            assert!(
                !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()),
                "{line}"
            );
        }
    }
}
