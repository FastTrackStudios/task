//! Obsidian `wordcount` CLI compatibility.
//!
//! Strips frontmatter, fenced code blocks, and inline code spans
//! before counting words/characters. Mirrors the shape returned by
//! `obsidian wordcount` (pages / words / characters).

#![cfg(not(target_arch = "wasm32"))]

use crate::obsidian_parse::parse_frontmatter;

use crate::vault::{Vault, VaultPage};

/// Aggregate word-count result.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WordCount {
    pub pages: usize,
    pub words: usize,
    pub characters: usize,
}

/// Vault-wide totals.
#[must_use]
pub fn vault_wordcount(vault: &Vault) -> WordCount {
    let mut out = WordCount::default();
    for page in &vault.pages {
        let pc = page_wordcount(page);
        out.pages += pc.pages;
        out.words += pc.words;
        out.characters += pc.characters;
    }
    out
}

/// Word/char count for a single page.
#[must_use]
pub fn page_wordcount(page: &VaultPage) -> WordCount {
    let cleaned = clean_for_count(&page.raw);
    WordCount {
        pages: 1,
        words: cleaned.split_ascii_whitespace().count(),
        characters: cleaned.chars().count(),
    }
}

/// Strip frontmatter, fenced code blocks, and inline code spans.
fn clean_for_count(raw: &str) -> String {
    // 1) Frontmatter — slice from the byte offset the parser
    //    hands back.
    let (_fm, body_offset) = parse_frontmatter(raw);
    let body = if body_offset <= raw.len() {
        &raw[body_offset..]
    } else {
        raw
    };

    // 2) Walk lines, tracking fenced-code state.
    let mut out = String::with_capacity(body.len());
    let mut in_fence = false;
    for line in body.split_inclusive('\n') {
        let trimmed_start = line.trim_start();
        if trimmed_start.starts_with("```") || trimmed_start.starts_with("~~~") {
            // Toggle and skip the fence delimiter line itself.
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        out.push_str(line);
    }

    // 3) Inline code — strip `...` spans within a single line.
    strip_inline_code(&out)
}

/// Remove `` `...` `` runs that do not cross a newline. Unbalanced
/// backticks at end-of-line are left in place.
fn strip_inline_code(src: &str) -> String {
    let mut out = String::with_capacity(src.len());
    let bytes = src.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'`' {
            // Find the next backtick before a newline.
            let mut j = i + 1;
            let mut found = None;
            while j < bytes.len() {
                match bytes[j] {
                    b'\n' => break,
                    b'`' => {
                        found = Some(j);
                        break;
                    }
                    _ => j += 1,
                }
            }
            if let Some(end) = found {
                // Skip the whole span (including both backticks).
                i = end + 1;
                continue;
            }
            // No closing backtick on this line — keep literally.
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn touch(p: &Path, body: &str) {
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, body).unwrap();
    }

    #[test]
    fn counts_plain_words() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a.md"), "hello world foo bar\n");
        let v = Vault::open(dir.path()).unwrap();
        let wc = vault_wordcount(&v);
        assert_eq!(wc.pages, 1);
        assert_eq!(wc.words, 4);
    }

    #[test]
    fn frontmatter_is_excluded() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("a.md"),
            "---\ntitle: Hello World Title\n---\njust body here\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let wc = vault_wordcount(&v);
        // "just body here" = 3 words.
        assert_eq!(wc.words, 3);
    }

    #[test]
    fn fenced_code_is_excluded() {
        let dir = tempfile::tempdir().unwrap();
        touch(
            &dir.path().join("a.md"),
            "one two\n```rust\nfn foo() { let x = 1; }\n```\nthree four five\n",
        );
        let v = Vault::open(dir.path()).unwrap();
        let wc = vault_wordcount(&v);
        // "one two" + "three four five" = 5 words; code stripped.
        assert_eq!(wc.words, 5);
    }

    #[test]
    fn inline_code_is_excluded() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a.md"), "use `cargo build` to compile\n");
        let v = Vault::open(dir.path()).unwrap();
        let wc = vault_wordcount(&v);
        // "use   to compile" -> 3 words (split_ascii_whitespace
        // collapses runs).
        assert_eq!(wc.words, 3);
    }

    #[test]
    fn page_wordcount_pages_is_one() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a.md"), "alpha beta gamma\n");
        let v = Vault::open(dir.path()).unwrap();
        let p = v.page("a.md").unwrap();
        let wc = page_wordcount(p);
        assert_eq!(wc.pages, 1);
        assert_eq!(wc.words, 3);
        // 3 words + 2 spaces + trailing newline = 18 chars.
        assert_eq!(wc.characters, "alpha beta gamma\n".chars().count());
    }

    #[test]
    fn vault_wordcount_sums_multiple_pages() {
        let dir = tempfile::tempdir().unwrap();
        touch(&dir.path().join("a.md"), "one two\n");
        touch(&dir.path().join("Folder/b.md"), "three four five\n");
        let v = Vault::open(dir.path()).unwrap();
        let wc = vault_wordcount(&v);
        assert_eq!(wc.pages, 2);
        assert_eq!(wc.words, 5);
    }
}
