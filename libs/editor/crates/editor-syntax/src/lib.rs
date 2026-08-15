//! Token-level syntax highlighting for code fences via Arborium tree-sitter.

use arborium_tree_sitter::StreamingIterator;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lang {
    #[cfg(feature = "lang-rust")]
    Rust,
    #[cfg(feature = "lang-typescript")]
    TypeScript,
    #[cfg(feature = "lang-javascript")]
    JavaScript,
    #[cfg(feature = "lang-python")]
    Python,
    #[cfg(feature = "lang-json")]
    Json,
    #[cfg(feature = "lang-bash")]
    Bash,
    #[cfg(feature = "lang-markdown")]
    Markdown,
}

impl Lang {
    /// Map a fenced-code-block info tag (e.g. `rs`, `rust`, `ts`) to a [`Lang`].
    #[must_use]
    pub fn from_fence_tag(tag: &str) -> Option<Self> {
        let normalized = tag.trim().to_ascii_lowercase();
        match normalized.as_str() {
            #[cfg(feature = "lang-rust")]
            "rs" | "rust" => Some(Self::Rust),
            #[cfg(feature = "lang-typescript")]
            "ts" | "typescript" | "mts" | "cts" => Some(Self::TypeScript),
            #[cfg(feature = "lang-javascript")]
            "js" | "javascript" | "mjs" | "cjs" | "node" => Some(Self::JavaScript),
            #[cfg(feature = "lang-python")]
            "py" | "python" | "python3" => Some(Self::Python),
            #[cfg(feature = "lang-json")]
            "json" | "jsonc" => Some(Self::Json),
            #[cfg(feature = "lang-bash")]
            "sh" | "bash" | "shell" | "zsh" => Some(Self::Bash),
            #[cfg(feature = "lang-markdown")]
            "md" | "markdown" => Some(Self::Markdown),
            _ => None,
        }
    }
}

/// A highlighted token: a non-overlapping byte range with an Arborium scope tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub start: usize,
    pub end: usize,
    pub tag: &'static str,
}

fn grammar_for(lang: Lang) -> (arborium_tree_sitter::LanguageFn, &'static str) {
    match lang {
        #[cfg(feature = "lang-rust")]
        Lang::Rust => (
            arborium::lang_rust::language(),
            arborium::lang_rust::HIGHLIGHTS_QUERY,
        ),
        #[cfg(feature = "lang-typescript")]
        Lang::TypeScript => (
            arborium::lang_typescript::language(),
            &arborium::lang_typescript::HIGHLIGHTS_QUERY,
        ),
        #[cfg(feature = "lang-javascript")]
        Lang::JavaScript => (
            arborium::lang_javascript::language(),
            arborium::lang_javascript::HIGHLIGHTS_QUERY,
        ),
        #[cfg(feature = "lang-python")]
        Lang::Python => (
            arborium::lang_python::language(),
            arborium::lang_python::HIGHLIGHTS_QUERY,
        ),
        #[cfg(feature = "lang-json")]
        Lang::Json => (
            arborium::lang_json::language(),
            arborium::lang_json::HIGHLIGHTS_QUERY,
        ),
        #[cfg(feature = "lang-bash")]
        Lang::Bash => (
            arborium::lang_bash::language(),
            arborium::lang_bash::HIGHLIGHTS_QUERY,
        ),
        #[cfg(feature = "lang-markdown")]
        Lang::Markdown => (
            arborium::lang_markdown::language(),
            arborium::lang_markdown::HIGHLIGHTS_QUERY,
        ),
    }
}

/// Parse `src` with `lang`'s grammar and return non-overlapping byte-range tokens
/// sorted by start. Overlapping captures are resolved by preferring the innermost
/// (most recently opened) one.
#[must_use]
pub fn highlight(lang: Lang, src: &str) -> Vec<Token> {
    let (language_fn, highlights_query) = grammar_for(lang);
    let ts_language: arborium_tree_sitter::Language = language_fn.into();

    let mut parser = arborium_tree_sitter::Parser::new();
    if parser.set_language(&ts_language).is_err() {
        return Vec::new();
    }
    let query = match arborium_tree_sitter::Query::new(&ts_language, highlights_query) {
        Ok(q) => q,
        Err(_) => return Vec::new(),
    };
    let tree = match parser.parse(src, None) {
        Some(t) => t,
        None => return Vec::new(),
    };

    let bytes = src.as_bytes();
    let capture_names = query.capture_names();
    let mut raw: Vec<RawSpan> = Vec::new();

    let mut cursor = arborium_tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), bytes);
    while let Some(m) = matches.next() {
        for capture in m.captures {
            let name = capture_names[capture.index as usize];
            if name.starts_with('_') || name.starts_with("injection.") {
                continue;
            }
            let Some(tag) = arborium_theme::tag_for_capture(name) else {
                continue;
            };
            raw.push(RawSpan {
                start: capture.node.start_byte() as u32,
                end: capture.node.end_byte() as u32,
                tag,
            });
        }
    }

    flatten(raw, src.len())
}

#[derive(Debug, Clone, Copy)]
struct RawSpan {
    start: u32,
    end: u32,
    tag: &'static str,
}

/// Resolve overlaps by sweeping events with a stack and emitting the innermost
/// active tag for each non-overlapping segment. Mirrors dioxus-code's segment
/// algorithm but yields only tagged ranges and coalesces adjacent same-tag
/// outputs.
fn flatten(mut raw: Vec<RawSpan>, src_len: usize) -> Vec<Token> {
    if raw.is_empty() {
        return Vec::new();
    }

    // Outer-first ordering means inner spans get pushed onto the stack later
    // and therefore win at any overlapping position.
    raw.sort_by(|a, b| a.start.cmp(&b.start).then_with(|| b.end.cmp(&a.end)));

    let mut events: Vec<(u32, bool, usize)> = Vec::with_capacity(raw.len() * 2);
    for (i, span) in raw.iter().enumerate() {
        if span.start >= span.end {
            continue;
        }
        events.push((span.start, true, i));
        events.push((span.end, false, i));
    }
    // Process ends before starts at the same position to close before opening.
    events.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let mut tokens: Vec<Token> = Vec::new();
    let mut stack: Vec<usize> = Vec::new();
    let mut last_pos: u32 = 0;

    for (pos, is_start, idx) in events {
        if pos > last_pos {
            if let Some(&top) = stack.last() {
                push_token(&mut tokens, last_pos as usize, pos as usize, raw[top].tag);
            }
            last_pos = pos;
        }

        if is_start {
            stack.push(idx);
        } else if let Some(i) = stack.iter().rposition(|&j| j == idx) {
            stack.remove(i);
        }
    }

    // Clip any trailing overrun to the source bounds.
    if let Some(last) = tokens.last_mut() {
        if last.end > src_len {
            last.end = src_len;
        }
        if last.start >= last.end {
            tokens.pop();
        }
    }

    tokens
}

fn push_token(tokens: &mut Vec<Token>, start: usize, end: usize, tag: &'static str) {
    if start >= end {
        return;
    }
    if let Some(last) = tokens.last_mut() {
        if last.end == start && last.tag == tag {
            last.end = end;
            return;
        }
    }
    tokens.push(Token { start, end, tag });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "lang-rust")]
    #[test]
    fn highlights_rust_fn_keyword() {
        let tokens = highlight(Lang::Rust, "fn main() {}");
        assert!(
            tokens
                .iter()
                .any(|t| t.tag == "k" && t.start == 0 && t.end == 2),
            "expected a keyword token for `fn` at 0..2, got {tokens:?}",
        );
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn tokens_are_sorted_and_non_overlapping() {
        let tokens = highlight(Lang::Rust, "fn main() { let x = 1; }");
        for pair in tokens.windows(2) {
            assert!(pair[0].end <= pair[1].start, "overlap: {pair:?}");
        }
    }

    #[cfg(feature = "lang-rust")]
    #[test]
    fn fence_tag_rust_aliases() {
        assert_eq!(Lang::from_fence_tag("rust"), Some(Lang::Rust));
        assert_eq!(Lang::from_fence_tag("rs"), Some(Lang::Rust));
        assert_eq!(Lang::from_fence_tag("RUST"), Some(Lang::Rust));
    }

    #[test]
    fn fence_tag_unknown_returns_none() {
        assert_eq!(Lang::from_fence_tag("nope-not-a-lang"), None);
    }
}
