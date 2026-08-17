//! Text objects: `iw aw i" a" i{ a{ i( a( i[ a[ it at ip ap`.
//!
//! vim ref: codemirror-vim/src/vim.js (`textObjects`)
//! vim ref: helix/helix-core/src/textobject.rs

use editor_state::EditorState;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextObject {
    Word,        // w
    DoubleQuote, // "
    SingleQuote, // '
    Backtick,    // `
    Paren,       // ( or )
    Bracket,     // [ or ]
    Brace,       // { or }
    AngleTag,    // t — XML/HTML tag (v1: stub, returns caret-only)
    Paragraph,   // p
}

impl TextObject {
    #[must_use]
    pub fn from_char(ch: char) -> Option<Self> {
        Some(match ch {
            'w' => Self::Word,
            '"' => Self::DoubleQuote,
            '\'' => Self::SingleQuote,
            '`' => Self::Backtick,
            '(' | ')' => Self::Paren,
            '[' | ']' => Self::Bracket,
            '{' | '}' => Self::Brace,
            't' => Self::AngleTag,
            'p' => Self::Paragraph,
            _ => return None,
        })
    }
}

/// Resolve a text object to a half-open byte range `[lo, hi)`
/// around `pos`. `around=true` for `a<obj>` (includes delimiters
/// / trailing space); `around=false` for `i<obj>` (inner).
#[must_use]
pub fn apply(
    state: &EditorState,
    obj: TextObject,
    around: bool,
    pos: usize,
) -> std::ops::Range<usize> {
    let s = state.doc.to_string();
    let bytes = s.as_bytes();
    match obj {
        TextObject::Word => word_object(bytes, pos, around),
        TextObject::DoubleQuote => pair_object(bytes, pos, b'"', b'"', around),
        TextObject::SingleQuote => pair_object(bytes, pos, b'\'', b'\'', around),
        TextObject::Backtick => pair_object(bytes, pos, b'`', b'`', around),
        TextObject::Paren => pair_object(bytes, pos, b'(', b')', around),
        TextObject::Bracket => pair_object(bytes, pos, b'[', b']', around),
        TextObject::Brace => pair_object(bytes, pos, b'{', b'}', around),
        TextObject::Paragraph => paragraph_object(bytes, pos, around),
        TextObject::AngleTag => pos..pos, // v1: not implemented
    }
}

fn is_word(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn word_object(bytes: &[u8], pos: usize, around: bool) -> std::ops::Range<usize> {
    if pos >= bytes.len() {
        return pos..pos;
    }
    // If we're on whitespace, the inner-word covers the run of
    // whitespace; the `aw` covers ws + following word. vim's
    // exact rules — close-enough for v1.
    let on_word = is_word(bytes[pos]);
    let mut lo = pos;
    let mut hi = pos;
    if on_word {
        while lo > 0 && is_word(bytes[lo - 1]) {
            lo -= 1;
        }
        while hi < bytes.len() && is_word(bytes[hi]) {
            hi += 1;
        }
        if around {
            // include surrounding whitespace on both sides — vim
            // is asymmetric here but the cases that matter (and
            // our test fixtures) want the whole island.
            let had_trail = hi < bytes.len() && bytes[hi] == b' ';
            while hi < bytes.len() && bytes[hi] == b' ' {
                hi += 1;
            }
            if had_trail {
                // also strip leading ws to mop up isolated words
                while lo > 0 && bytes[lo - 1] == b' ' {
                    lo -= 1;
                }
            } else {
                while lo > 0 && bytes[lo - 1] == b' ' {
                    lo -= 1;
                }
            }
        }
    } else {
        // whitespace run
        while lo > 0 && bytes[lo - 1] == b' ' {
            lo -= 1;
        }
        while hi < bytes.len() && bytes[hi] == b' ' {
            hi += 1;
        }
        if around && hi < bytes.len() && is_word(bytes[hi]) {
            while hi < bytes.len() && is_word(bytes[hi]) {
                hi += 1;
            }
        }
    }
    lo..hi
}

fn pair_object(
    bytes: &[u8],
    pos: usize,
    open: u8,
    close: u8,
    around: bool,
) -> std::ops::Range<usize> {
    // Find the opening delim by scanning left, then the matching
    // closing delim by scanning right. Identical-delim case
    // (quotes) doesn't nest; just find nearest neighbors.
    if open == close {
        let mut lo = None;
        for i in (0..pos).rev() {
            if bytes[i] == open {
                lo = Some(i);
                break;
            }
        }
        let mut hi = None;
        #[allow(clippy::needless_range_loop)]
        for i in pos..bytes.len() {
            if bytes[i] == close && Some(i) != lo {
                hi = Some(i);
                break;
            }
        }
        let (Some(lo), Some(hi)) = (lo, hi) else {
            return pos..pos;
        };
        return if around { lo..(hi + 1) } else { (lo + 1)..hi };
    }
    // Brackets: walk left counting depth.
    let mut depth = 0i32;
    let mut open_idx = None;
    for i in (0..=pos.min(bytes.len().saturating_sub(1))).rev() {
        if bytes[i] == close {
            depth += 1;
        } else if bytes[i] == open {
            if depth == 0 {
                open_idx = Some(i);
                break;
            }
            depth -= 1;
        }
    }
    let Some(open_idx) = open_idx else {
        return pos..pos;
    };
    let mut depth = 0i32;
    let mut close_idx = None;
    #[allow(clippy::needless_range_loop)]
    for i in open_idx..bytes.len() {
        if bytes[i] == open {
            depth += 1;
        } else if bytes[i] == close {
            depth -= 1;
            if depth == 0 {
                close_idx = Some(i);
                break;
            }
        }
    }
    let Some(close_idx) = close_idx else {
        return pos..pos;
    };
    if around {
        open_idx..(close_idx + 1)
    } else {
        (open_idx + 1)..close_idx
    }
}

fn paragraph_object(bytes: &[u8], pos: usize, around: bool) -> std::ops::Range<usize> {
    let is_blank = |line_start: usize, line_end: usize| {
        bytes[line_start..line_end]
            .iter()
            .all(u8::is_ascii_whitespace)
    };
    // find current line start
    let p = pos.min(bytes.len());
    let mut start = p;
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    // walk back over non-blank lines
    let mut lo = start;
    loop {
        if lo == 0 {
            break;
        }
        let prev_end = lo - 1; // newline
        let mut prev_start = prev_end;
        while prev_start > 0 && bytes[prev_start - 1] != b'\n' {
            prev_start -= 1;
        }
        if is_blank(prev_start, prev_end) {
            break;
        }
        lo = prev_start;
    }
    // walk forward over non-blank lines
    let mut hi = start;
    loop {
        let line_end = match bytes[hi..].iter().position(|&b| b == b'\n') {
            Some(i) => hi + i,
            None => bytes.len(),
        };
        if is_blank(hi, line_end) {
            break;
        }
        hi = (line_end + 1).min(bytes.len());
        if hi == bytes.len() {
            break;
        }
    }
    if around {
        // include trailing blank lines
        while hi < bytes.len() {
            let line_end = match bytes[hi..].iter().position(|&b| b == b'\n') {
                Some(i) => hi + i,
                None => bytes.len(),
            };
            if !is_blank(hi, line_end) {
                break;
            }
            hi = (line_end + 1).min(bytes.len());
        }
    }
    // drop the trailing `\n` of the last non-blank line for inner
    if !around && hi > lo && hi <= bytes.len() && bytes.get(hi.saturating_sub(1)) == Some(&b'\n') {
        hi -= 1;
    }
    let _ = pos;
    lo..hi
}
