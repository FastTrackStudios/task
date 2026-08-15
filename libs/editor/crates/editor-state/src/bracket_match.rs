//! Bracket-matching decoration source. Mirrors CM6's
//! `language/src/matchbrackets.ts:57` (`bracketDeco`).
//!
//! When the primary selection is a caret next to a bracket
//! character, scan in the appropriate direction for the matching
//! one and emit `Mark` decorations on both. The view paints them
//! via `.md-bracket-match` / `.md-bracket-mismatch`.

use crate::decoration::{DecoratedRange, Decoration};
use crate::state::EditorState;

const PAIRS: &[(u8, u8)] = &[(b'(', b')'), (b'[', b']'), (b'{', b'}')];

/// Decoration source — registers in `editor_view::DecorationSource`.
/// Walks at most [`SCAN_LIMIT`] bytes from the caret looking for
/// the matching bracket. Stops if the doc is malformed or the
/// match falls outside the scan window.
#[must_use]
pub fn bracket_match(state: &EditorState) -> Vec<DecoratedRange> {
    let primary = state.selection.primary();
    if primary.anchor != primary.head {
        return Vec::new();
    }
    let caret = primary.head;
    let doc = state.doc.to_string();
    let bytes = doc.as_bytes();
    let mut out = Vec::new();

    // Try the byte to the LEFT first (CM6 does the same — a
    // caret right after `)` matches its `(`), falling back to
    // the byte to the RIGHT.
    if caret > 0 {
        if let Some((from, to, matched)) = match_at(bytes, caret - 1) {
            push_pair(&mut out, from, to, matched);
            return out;
        }
    }
    if caret < bytes.len() {
        if let Some((from, to, matched)) = match_at(bytes, caret) {
            push_pair(&mut out, from, to, matched);
        }
    }
    out
}

fn push_pair(out: &mut Vec<DecoratedRange>, from: usize, to: usize, matched: bool) {
    let class = if matched {
        "md-bracket-match"
    } else {
        "md-bracket-mismatch"
    };
    out.push(Decoration::mark(from..from + 1, class));
    if to != from {
        out.push(Decoration::mark(to..to + 1, class));
    }
}

const SCAN_LIMIT: usize = 10_000;

fn match_at(bytes: &[u8], at: usize) -> Option<(usize, usize, bool)> {
    let c = *bytes.get(at)?;
    for &(open, close) in PAIRS {
        if c == open {
            let matched = scan_forward(bytes, at, open, close);
            return Some((at, matched.unwrap_or(at), matched.is_some()));
        }
        if c == close {
            let matched = scan_backward(bytes, at, open, close);
            return Some((matched.unwrap_or(at), at, matched.is_some()));
        }
    }
    None
}

fn scan_forward(bytes: &[u8], at: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 1;
    let end = (at + 1 + SCAN_LIMIT).min(bytes.len());
    // Bracket scan walks a tight inner loop; the `for i in
    // start..end` shape is the natural one even though
    // clippy's `needless_range_loop` would prefer a slice
    // iter. We need `i` itself for the `Some(i)` return.
    #[allow(clippy::needless_range_loop)]
    for i in (at + 1)..end {
        let b = bytes[i];
        if b == open {
            depth += 1;
        } else if b == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

fn scan_backward(bytes: &[u8], at: usize, open: u8, close: u8) -> Option<usize> {
    let mut depth = 1;
    let start = at.saturating_sub(SCAN_LIMIT);
    let mut i = at;
    while i > start {
        i -= 1;
        let b = bytes[i];
        if b == close {
            depth += 1;
        } else if b == open {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selection::Selection;

    fn state_at(text: &str, head: usize) -> EditorState {
        let mut s = EditorState::new(text);
        s.selection = Selection::caret(head);
        s
    }

    #[test]
    fn matches_paren_pair_when_caret_after_open() {
        let s = state_at("(foo)", 1);
        let d = bracket_match(&s);
        assert_eq!(d.len(), 2);
        assert!(d.iter().any(|x| x.from == 0));
        assert!(d.iter().any(|x| x.from == 4));
    }

    #[test]
    fn matches_paren_pair_when_caret_before_close() {
        let s = state_at("(foo)", 4);
        let d = bracket_match(&s);
        assert_eq!(d.len(), 2);
        assert!(d.iter().any(|x| x.from == 0));
        assert!(d.iter().any(|x| x.from == 4));
    }

    #[test]
    fn unmatched_open_marks_as_mismatch() {
        let s = state_at("(foo", 1);
        let d = bracket_match(&s);
        // 1 mark only (the open) marked mismatch.
        assert_eq!(d.len(), 1);
        if let crate::decoration::DecorationKind::Mark { class, .. } = &d[0].kind {
            assert_eq!(class, "md-bracket-mismatch");
        } else {
            panic!("expected Mark");
        }
    }

    #[test]
    fn nested_brackets_match_correct_pair() {
        let s = state_at("((a))", 1);
        let d = bracket_match(&s);
        // Caret after outer `(` — should pair with outer `)`.
        assert!(d.iter().any(|x| x.from == 0));
        assert!(d.iter().any(|x| x.from == 4));
    }

    #[test]
    fn non_caret_selection_emits_nothing() {
        let mut s = EditorState::new("(foo)");
        s.selection = Selection::single(crate::selection::Range::new(0, 5));
        assert!(bracket_match(&s).is_empty());
    }
}
