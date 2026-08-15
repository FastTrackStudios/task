//! Adapter from a [`Vault`] + [`BlockIndex`] to the editor's
//! `VaultLookup` trait. Lets `editor_view::Editor` resolve
//! `((uuid))`, `[[Page]]`, `![[Page#Heading]]`, and
//! `![[Page#^short-id]]` against the full vault while staying
//! ignorant of vault internals.

use editor_state::markdown::{VaultBlockHit, VaultLookup, VaultPageHit};

use crate::{BlockIndex, Vault};

/// Borrowed view over a vault + block index. Cheap to
/// construct; live for the duration of one `live_preview_with`
/// pass.
pub struct VaultLookupView<'v> {
    vault: &'v Vault,
    blocks: &'v BlockIndex,
}

impl<'v> VaultLookupView<'v> {
    #[must_use]
    pub fn new(vault: &'v Vault, blocks: &'v BlockIndex) -> Self {
        Self { vault, blocks }
    }
}

impl VaultLookup for VaultLookupView<'_> {
    fn lookup_block(&self, uuid: &str) -> Option<VaultBlockHit> {
        let loc = self.blocks.lookup_str(uuid)?;
        let page = self.vault.pages.get(loc.page_idx)?;
        let preview = first_line(&page.raw, loc.anchor);
        Some(VaultBlockHit {
            page: page.basename.clone(),
            preview,
        })
    }

    fn lookup_page(&self, name: &str) -> Option<VaultPageHit> {
        let page = self.vault.page_by_basename(name)?;
        Some(VaultPageHit {
            preview: preview_of_page(&page.raw, 200),
        })
    }

    fn lookup_section(&self, page: &str, heading: &str) -> Option<String> {
        let p = self.vault.page_by_basename(page)?;
        section_body(&p.raw, heading)
    }

    fn lookup_block_short(&self, page: &str, short_id: &str) -> Option<String> {
        let p = self.vault.page_by_basename(page)?;
        let needle = format!("^{short_id}");
        let pos = p.raw.find(&needle)?;
        let line_start = p.raw[..pos].rfind('\n').map_or(0, |n| n + 1);
        let line_end = p.raw[pos..].find('\n').map_or(p.raw.len(), |n| pos + n);
        let line = &p.raw[line_start..line_end];
        Some(line[..line.len() - needle.len()].trim_end().to_string())
    }
}

/// First line of the block at `anchor` in `raw`, capped at 80
/// chars for chip display.
fn first_line(raw: &str, anchor: usize) -> String {
    let end = raw[anchor..].find('\n').map_or(raw.len(), |n| anchor + n);
    let line = &raw[anchor..end];
    if line.chars().count() > 80 {
        let truncated: String = line.chars().take(80).collect();
        format!("{truncated}…")
    } else {
        line.to_string()
    }
}

/// Page preview: first non-empty paragraph, stripped of
/// frontmatter, capped at `max` chars.
fn preview_of_page(raw: &str, max: usize) -> String {
    // Skip frontmatter if present.
    let body_start = if let Some(rest) = raw.strip_prefix("---\n") {
        rest.find("\n---\n").map_or(0, |i| 4 + i + 5)
    } else {
        0
    };
    let body = &raw[body_start..];
    let trimmed = body.trim_start();
    let truncated: String = trimmed.chars().take(max).collect();
    if trimmed.chars().count() > max {
        format!("{truncated}…")
    } else {
        truncated
    }
}

/// Find a heading whose text matches `name` (case-insensitive
/// trim) and return its body content up to the next same-or-
/// higher heading.
fn section_body(raw: &str, name: &str) -> Option<String> {
    let needle = name.trim().to_lowercase();
    let lines: Vec<(usize, usize)> = line_ranges(raw);
    let mut start_idx: Option<(usize, usize)> = None;
    for (i, (lf, lt)) in lines.iter().enumerate() {
        let line = &raw[*lf..*lt];
        if let Some((level, marker_end)) = parse_heading(line) {
            let title = line[marker_end..].trim();
            if title.to_lowercase() == needle {
                start_idx = Some((i, level));
                break;
            }
        }
    }
    let (start_i, start_level) = start_idx?;
    let mut body = String::new();
    for (lf, lt) in lines.iter().skip(start_i + 1) {
        let line = &raw[*lf..*lt];
        if let Some((level, _)) = parse_heading(line) {
            if level <= start_level {
                break;
            }
        }
        body.push_str(line);
        body.push('\n');
    }
    Some(body.trim().to_string())
}

fn line_ranges(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut start = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b == b'\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    if start <= bytes.len() {
        out.push((start, bytes.len()));
    }
    out
}

fn parse_heading(line: &str) -> Option<(usize, usize)> {
    let b = line.as_bytes();
    let mut level = 0;
    while level < 6 && b.get(level) == Some(&b'#') {
        level += 1;
    }
    if level == 0 {
        return None;
    }
    if b.get(level) != Some(&b' ') {
        return None;
    }
    Some((level, level + 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vault::VaultPage;
    use std::time::SystemTime;

    fn page(rel: &str, raw: &str) -> VaultPage {
        VaultPage {
            rel_path: rel.into(),
            basename: std::path::Path::new(rel)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(rel)
                .into(),
            folder: String::new(),
            raw: raw.into(),
            mtime: SystemTime::UNIX_EPOCH,
        }
    }

    fn build(pages: Vec<VaultPage>) -> (Vault, BlockIndex) {
        let v = Vault {
            bases: Vec::new(),
            property_types: Default::default(),
            root: ".".into(),
            pages,
        };
        let idx = BlockIndex::build(&v);
        (v, idx)
    }

    #[test]
    fn lookup_block_finds_cross_page_uuid() {
        let uuid = "11111111-1111-4111-8111-111111111111";
        let (v, idx) = build(vec![
            page("p1.md", "unrelated"),
            page("Home.md", &format!("Block content here\nid:: {uuid}\n")),
        ]);
        let lookup = VaultLookupView::new(&v, &idx);
        let hit = lookup.lookup_block(uuid).unwrap();
        assert_eq!(hit.page, "Home");
        assert_eq!(hit.preview, "Block content here");
    }

    #[test]
    fn lookup_page_returns_preview() {
        let (v, idx) = build(vec![page(
            "Howdy.md",
            "---\ntitle: x\n---\nFirst para body",
        )]);
        let lookup = VaultLookupView::new(&v, &idx);
        let hit = lookup.lookup_page("Howdy").unwrap();
        assert!(hit.preview.contains("First para body"));
    }

    #[test]
    fn lookup_section_extracts_heading_body() {
        let raw = "# Top\n\nintro\n\n## Section\nbody line\nmore body\n\n## Next\nelse";
        let (v, idx) = build(vec![page("p.md", raw)]);
        let lookup = VaultLookupView::new(&v, &idx);
        let body = lookup.lookup_section("p", "Section").unwrap();
        assert!(body.contains("body line"));
        assert!(!body.contains("else"));
    }

    #[test]
    fn lookup_block_short_handles_obsidian_anchor() {
        let raw = "Inline block ^anchor-here\nrest";
        let (v, idx) = build(vec![page("p.md", raw)]);
        let lookup = VaultLookupView::new(&v, &idx);
        let body = lookup.lookup_block_short("p", "anchor-here").unwrap();
        assert_eq!(body, "Inline block");
    }
}
