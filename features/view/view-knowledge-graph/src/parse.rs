//! Markdown extraction helpers shared by [`crate::build`] and
//! [`crate::relevance`]: frontmatter fields, `[[wikilinks]]`, and link
//! target resolution. Pure string work — no regex crate, no FS.

/// A wiki markdown file handed to the builder. The caller is
/// responsible for walking the `wiki/` tree and reading contents; this
/// crate stays FS-free so it builds on wasm and is trivially testable.
#[derive(Debug, Clone, PartialEq)]
pub struct WikiFile {
    /// File name including the `.md` extension (e.g. `acme-corp.md`).
    pub name: String,
    /// Absolute path, surfaced on the node for click-to-open.
    pub path: String,
    /// Full file contents (frontmatter + body).
    pub content: String,
}

impl WikiFile {
    /// Node id — file name without the `.md` suffix.
    pub fn id(&self) -> String {
        self.name
            .strip_suffix(".md")
            .unwrap_or(&self.name)
            .to_string()
    }
}

/// Extract the `---\n…\n---` frontmatter block (without the fences),
/// or `""` if there is none at the very top of the file.
pub fn frontmatter(content: &str) -> &str {
    let rest = match content.strip_prefix("---\n") {
        Some(r) => r,
        None => return "",
    };
    match rest.find("\n---") {
        Some(end) => &rest[..end],
        None => "",
    }
}

/// Read a simple scalar `key: value` line out of a frontmatter block.
/// Strips surrounding quotes. Returns the first match.
fn fm_scalar(fm: &str, key: &str) -> Option<String> {
    for line in fm.lines() {
        let trimmed = line.trim_end();
        if let Some(rest) = trimmed.strip_prefix(key) {
            if let Some(val) = rest.strip_prefix(':') {
                let val = val.trim();
                let val = val
                    .trim_start_matches(['"', '\''])
                    .trim_end_matches(['"', '\'']);
                if !val.is_empty() {
                    return Some(val.to_string());
                }
            }
        }
    }
    None
}

/// Page title: frontmatter `title:`, else first `# heading`, else the
/// de-slugged file name (`acme-corp` → `acme corp`).
pub fn extract_title(content: &str, file_name: &str) -> String {
    let fm = frontmatter(content);
    if let Some(title) = fm_scalar(fm, "title") {
        return title;
    }
    for line in content.lines() {
        if let Some(h) = line.strip_prefix("# ") {
            let h = h.trim();
            if !h.is_empty() {
                return h.to_string();
            }
        }
    }
    file_name
        .strip_suffix(".md")
        .unwrap_or(file_name)
        .replace('-', " ")
}

/// Page kind: frontmatter `type:` lowercased, else `"other"`.
pub fn extract_kind(content: &str) -> String {
    fm_scalar(frontmatter(content), "type")
        .map_or_else(|| "other".to_string(), |t| t.to_ascii_lowercase())
}

/// Source citations from frontmatter `sources:` — supports both the
/// block form (`sources:\n  - a.pdf`) and the inline form
/// (`sources: [a.pdf, b.pdf]`).
pub fn extract_sources(content: &str) -> Vec<String> {
    let fm = frontmatter(content);
    let mut out = Vec::new();

    let lines: Vec<&str> = fm.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let trimmed = line.trim_end();
        if trimmed.trim_start() == "sources:" || trimmed == "sources:" {
            // Block form: gather following `  - item` lines.
            for next in &lines[i + 1..] {
                let t = next.trim_start();
                if let Some(item) = t.strip_prefix("- ") {
                    let item = item
                        .trim()
                        .trim_start_matches(['"', '\''])
                        .trim_end_matches(['"', '\'']);
                    if !item.is_empty() {
                        out.push(item.to_string());
                    }
                } else if next.trim().is_empty() {
                    continue;
                } else {
                    break;
                }
            }
            return out;
        }
        // Inline form: `sources: [a, b]`
        if let Some(rest) = trimmed.trim_start().strip_prefix("sources:") {
            let rest = rest.trim();
            if let Some(inner) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
                for item in inner.split(',') {
                    let item = item
                        .trim()
                        .trim_start_matches(['"', '\''])
                        .trim_end_matches(['"', '\'']);
                    if !item.is_empty() {
                        out.push(item.to_string());
                    }
                }
                return out;
            }
        }
    }
    out
}

/// All `[[wikilink]]` targets in `content`, alias-stripped
/// (`[[target|alias]]` → `target`) and trimmed.
pub fn extract_wikilinks(content: &str) -> Vec<String> {
    let bytes = content.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            if let Some(close) = content[i + 2..].find("]]") {
                let inner = &content[i + 2..i + 2 + close];
                // Reject nested `[` — not a real wikilink.
                if !inner.contains('[') {
                    let target = inner.split('|').next().unwrap_or(inner).trim();
                    if !target.is_empty() {
                        out.push(target.to_string());
                    }
                }
                i = i + 2 + close + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Resolve a raw wikilink target against the set of known node ids,
/// matching the TS `resolveTarget`: exact, then case-insensitive, then
/// space/hyphen-normalized.
pub fn resolve_target<'a, I>(raw: &str, ids: I) -> Option<String>
where
    I: IntoIterator<Item = &'a String>,
{
    let raw_lower = raw.to_ascii_lowercase();
    let normalized = raw_lower.replace(char::is_whitespace, "-");
    let mut exact: Option<String> = None;
    for id in ids {
        if id == raw {
            return Some(id.clone());
        }
        let id_lower = id.to_ascii_lowercase();
        if id_lower == normalized
            || id_lower == raw_lower
            || id_lower.replace(char::is_whitespace, "-") == normalized
        {
            exact.get_or_insert_with(|| id.clone());
        }
    }
    exact
}

#[cfg(test)]
mod tests {
    use super::*;

    const DOC: &str = "---\ntitle: \"Acme Corp\"\ntype: entity\nsources:\n  - report.pdf\n  - \"memo.txt\"\n---\n# Heading\n\nSee [[Other Page]] and [[target|alias]].\n";

    #[test]
    fn parses_frontmatter_fields() {
        assert_eq!(extract_title(DOC, "acme.md"), "Acme Corp");
        assert_eq!(extract_kind(DOC), "entity");
        assert_eq!(extract_sources(DOC), vec!["report.pdf", "memo.txt"]);
    }

    #[test]
    fn inline_sources() {
        let d = "---\nsources: [a.pdf, \"b.pdf\"]\n---\n";
        assert_eq!(extract_sources(d), vec!["a.pdf", "b.pdf"]);
    }

    #[test]
    fn title_falls_back_to_heading_then_filename() {
        assert_eq!(
            extract_title("# Just A Heading\n", "x.md"),
            "Just A Heading"
        );
        assert_eq!(extract_title("body only", "acme-corp.md"), "acme corp");
    }

    #[test]
    fn wikilinks_strip_aliases() {
        assert_eq!(
            extract_wikilinks(DOC),
            vec!["Other Page".to_string(), "target".to_string()]
        );
    }

    #[test]
    fn resolve_target_normalizes() {
        let ids = vec!["other-page".to_string(), "acme".to_string()];
        assert_eq!(
            resolve_target("Other Page", &ids),
            Some("other-page".into())
        );
        assert_eq!(resolve_target("missing", &ids), None);
    }
}
