//! Page frontmatter + wikilink extraction.
//!
//! Tiny, dependency-free parsers — we don't pull in
//! `yaml-rust`/`serde_yaml` just for flat key/value reads.

use std::collections::HashSet;

#[derive(Debug, Clone)]
#[allow(clippy::struct_field_names)]
pub(crate) struct Page {
    /// Wiki-relative path (e.g. `Concepts/Foo.md`).
    pub rel_path: String,
    /// `title:` from frontmatter, falling back to the
    /// file stem.
    pub title: String,
    /// `type:` from frontmatter; `"untyped"` when absent.
    pub page_type: String,
    /// Tags from `tags: [a, b, c]`.
    #[allow(dead_code)]
    pub tags: Vec<String>,
    /// Source ids from `sources: [a.md, b.md]`.
    pub sources: Vec<String>,
    /// Wikilink targets extracted from the body — `[[Foo]]`
    /// → `"Foo"`. Section refs (`[[Foo#Bar]]`) and alias
    /// labels (`[[Foo|alias]]`) are normalized to the page
    /// portion (`Foo`).
    pub outlinks: HashSet<String>,
    /// Body markdown (everything after the frontmatter
    /// close fence). Used by the context-subgraph builder
    /// to extract first-paragraph summaries.
    pub body: String,
}

pub(crate) fn parse_page(rel_path: String, body: &str) -> Page {
    let (fm, rest) = split_frontmatter(body);
    let fm_kv = parse_frontmatter_kv(fm);

    let title = fm_kv.get("title").cloned().unwrap_or_else(|| {
        std::path::Path::new(&rel_path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string()
    });
    let page_type = fm_kv
        .get("type")
        .cloned()
        .unwrap_or_else(|| "untyped".to_string());
    let tags = parse_array(fm_kv.get("tags").map(String::as_str));
    let sources = parse_array(fm_kv.get("sources").map(String::as_str));
    let outlinks = extract_wikilinks(rest);

    Page {
        rel_path,
        title,
        page_type,
        tags,
        sources,
        outlinks,
        body: rest.to_string(),
    }
}

/// `---\n<fm>\n---\n<body>` — return `(fm, body)`. If no
/// frontmatter, return `("", whole)`.
fn split_frontmatter(src: &str) -> (&str, &str) {
    let Some(rest) = src.strip_prefix("---\n") else {
        return ("", src);
    };
    let Some(end) = rest.find("\n---\n") else {
        return ("", src);
    };
    (&rest[..end], &rest[end + 5..])
}

fn parse_frontmatter_kv(fm: &str) -> std::collections::HashMap<String, String> {
    let mut out = std::collections::HashMap::new();
    for line in fm.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_string();
            let mut v = v.trim().to_string();
            if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
                v = v[1..v.len() - 1].to_string();
            }
            if !k.is_empty() {
                out.insert(k, v);
            }
        }
    }
    out
}

/// Parse a YAML inline array `[a, b, c]` (or a single
/// scalar) into a vec of trimmed strings. Empty input ⇒
/// empty vec.
fn parse_array(value: Option<&str>) -> Vec<String> {
    let Some(s) = value else {
        return Vec::new();
    };
    let s = s.trim();
    if s.is_empty() {
        return Vec::new();
    }
    if let Some(inner) = s.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
        inner
            .split(',')
            .map(|item| {
                let mut v = item.trim().to_string();
                if v.starts_with('"') && v.ends_with('"') && v.len() >= 2 {
                    v = v[1..v.len() - 1].to_string();
                }
                v
            })
            .filter(|v| !v.is_empty())
            .collect()
    } else {
        // Bare scalar — treat as a single-element list.
        vec![s.to_string()]
    }
}

/// Pull every `[[Target]]` reference out of the body.
/// Normalizes section refs (`[[Foo#Bar]]` → `"Foo"`) and
/// alias labels (`[[Foo|alias]]` → `"Foo"`).
pub fn extract_wikilinks(body: &str) -> HashSet<String> {
    let mut out = HashSet::new();
    let bytes = body.as_bytes();
    let mut i = 0;
    while i + 1 < bytes.len() {
        if bytes[i] == b'[' && bytes[i + 1] == b'[' {
            let start = i + 2;
            let Some(close_rel) = body[start..].find("]]") else {
                break;
            };
            let inner = &body[start..start + close_rel];
            // Section ref + alias: keep only the page part
            // before `#` and before `|`.
            let page = inner
                .split('#')
                .next()
                .unwrap_or("")
                .split('|')
                .next()
                .unwrap_or("")
                .trim();
            if !page.is_empty() {
                out.insert(page.to_string());
            }
            i = start + close_rel + 2;
        } else {
            i += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_basic_wikilinks() {
        let body = "See [[Foo]] and [[Bar#section]] and [[Baz|alias]].";
        let mut links: Vec<_> = extract_wikilinks(body).into_iter().collect();
        links.sort();
        assert_eq!(links, vec!["Bar", "Baz", "Foo"]);
    }

    #[test]
    fn parses_inline_array() {
        assert_eq!(parse_array(Some("[a, b, c]")), vec!["a", "b", "c"]);
        assert_eq!(
            parse_array(Some(r#"["quoted", bare]"#)),
            vec!["quoted", "bare"]
        );
        assert!(parse_array(None).is_empty());
    }
}
