//! Position conversion — editor byte offsets ↔ LSP UTF-16
//! line/character positions. The correctness-critical seam of the
//! whole crate: every diagnostic range and every incremental
//! `didChange` passes through here.
//!
//! The editor addresses text by **byte offset** into UTF-8 (see
//! `editor-state::doc`). LSP positions are `{line, character}` where
//! `character` counts **UTF-16 code units** from the line start (the
//! spec's default `positionEncoding`; we don't negotiate `utf-8`
//! because plenty of servers only implement `utf-16`). So an emoji
//! is 4 bytes to us and 2 characters to the server; `é` is 2 bytes
//! and 1 character. Documents are `\n`-separated — the editor never
//! stores CRLF.
//!
//! Conversions lean on the rope's line index (`byte_to_line` /
//! `line_to_byte` are O(log n)); only the within-line prefix is
//! scanned, so long documents don't pay for early lines.

use editor_state::{Changes, Doc};
use lsp_types::{Position, Range, TextDocumentContentChangeEvent};

/// Convert a byte offset into an LSP UTF-16 [`Position`].
///
/// `byte` is clamped to the document length; offsets inside a
/// multi-byte character are treated as pointing at that character's
/// start (the scan stops before crossing it).
#[must_use]
pub fn byte_to_position(doc: &Doc, byte: usize) -> Position {
    let rope = doc.rope();
    let byte = byte.min(rope.len_bytes());
    // Floor to a char boundary (byte_to_char floors, so the
    // round-trip lands on the containing char's start) — an offset
    // inside a multi-byte char maps to that char's position.
    let byte = rope.char_to_byte(rope.byte_to_char(byte));
    let line = rope.byte_to_line(byte);
    let line_start = rope.line_to_byte(line);
    // UTF-16 length of the line prefix [line_start, byte).
    let character: usize = rope
        .byte_slice(line_start..byte)
        .chars()
        .map(char::len_utf16)
        .sum();
    Position {
        line: line as u32,
        character: character as u32,
    }
}

/// Convert an LSP UTF-16 [`Position`] into a byte offset.
///
/// Follows the spec's clamping rules: a line past the end of the
/// document maps to the document end; a `character` past the end of
/// the line maps to the line end (before the `\n`). A `character`
/// that would split a surrogate pair rounds *down* to the start of
/// that code point.
#[must_use]
pub fn position_to_byte(doc: &Doc, pos: Position) -> usize {
    let rope = doc.rope();
    let line = pos.line as usize;
    if line >= rope.len_lines() {
        return rope.len_bytes();
    }
    let line_start = rope.line_to_byte(line);
    let line_end = rope.line_to_byte(line + 1); // one-past-the-end is valid
    let mut remaining = pos.character as usize;
    let mut offset = line_start;
    for ch in rope.byte_slice(line_start..line_end).chars() {
        if remaining == 0 || ch == '\n' {
            break;
        }
        let width = ch.len_utf16();
        if width > remaining {
            break; // position splits a surrogate pair — round down
        }
        remaining -= width;
        offset += ch.len_utf8();
    }
    offset
}

/// Convert a byte range into an LSP [`Range`].
#[must_use]
pub fn byte_range_to_range(doc: &Doc, range: std::ops::Range<usize>) -> Range {
    Range {
        start: byte_to_position(doc, range.start),
        end: byte_to_position(doc, range.end),
    }
}

/// Convert an LSP [`Range`] into a byte range (clamped, and
/// normalized so `start <= end`).
#[must_use]
pub fn range_to_byte_range(doc: &Doc, range: Range) -> std::ops::Range<usize> {
    let from = position_to_byte(doc, range.start);
    let to = position_to_byte(doc, range.end);
    from.min(to)..from.max(to)
}

/// Translate an editor [`Changes`] set into incremental LSP
/// `didChange` content-change events.
///
/// The impedance mismatch this bridges: every `Change` in a
/// `Changes` carries offsets against **`doc_before`** (they apply
/// "simultaneously"), while LSP content changes apply
/// **sequentially** — each event's range addresses the document as
/// left by the previous event. Emitting the (sorted, non-overlapping)
/// changes in *reverse* document order resolves it: applying an edit
/// never shifts positions before it, so each earlier range is still
/// valid against `doc_before` when its turn comes.
#[must_use]
pub fn changes_to_content_changes(
    doc_before: &Doc,
    changes: &Changes,
) -> Vec<TextDocumentContentChangeEvent> {
    let mut ordered: Vec<_> = changes.iter().collect();
    ordered.reverse();
    ordered
        .into_iter()
        .map(|c| TextDocumentContentChangeEvent {
            range: Some(byte_range_to_range(doc_before, c.from..c.to)),
            range_length: None,
            text: c.inserted.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use editor_state::Change;

    fn pos(line: u32, character: u32) -> Position {
        Position { line, character }
    }

    #[test]
    fn ascii_single_line() {
        let d = Doc::from_str("hello");
        assert_eq!(byte_to_position(&d, 0), pos(0, 0));
        assert_eq!(byte_to_position(&d, 3), pos(0, 3));
        assert_eq!(byte_to_position(&d, 5), pos(0, 5));
        assert_eq!(position_to_byte(&d, pos(0, 3)), 3);
    }

    #[test]
    fn ascii_multi_line() {
        let d = Doc::from_str("ab\ncd\nef");
        // Start of line 1 is byte 3.
        assert_eq!(byte_to_position(&d, 3), pos(1, 0));
        assert_eq!(byte_to_position(&d, 4), pos(1, 1));
        // The newline itself belongs to the line it ends.
        assert_eq!(byte_to_position(&d, 2), pos(0, 2));
        assert_eq!(position_to_byte(&d, pos(2, 1)), 7);
    }

    #[test]
    fn two_byte_utf8_is_one_utf16_unit() {
        // "é" is 2 bytes UTF-8, 1 UTF-16 unit.
        let d = Doc::from_str("aéb");
        assert_eq!(byte_to_position(&d, 1), pos(0, 1)); // before é
        assert_eq!(byte_to_position(&d, 3), pos(0, 2)); // after é
        assert_eq!(position_to_byte(&d, pos(0, 2)), 3);
    }

    #[test]
    fn three_byte_utf8_is_one_utf16_unit() {
        // "€" is 3 bytes UTF-8, 1 UTF-16 unit.
        let d = Doc::from_str("€x");
        assert_eq!(byte_to_position(&d, 3), pos(0, 1));
        assert_eq!(position_to_byte(&d, pos(0, 1)), 3);
    }

    #[test]
    fn emoji_is_two_utf16_units() {
        // "😀" (U+1F600) is 4 bytes UTF-8, a surrogate pair (2 units)
        // in UTF-16.
        let d = Doc::from_str("a😀b");
        assert_eq!(byte_to_position(&d, 1), pos(0, 1)); // before emoji
        assert_eq!(byte_to_position(&d, 5), pos(0, 3)); // after emoji
        assert_eq!(position_to_byte(&d, pos(0, 3)), 5);
        assert_eq!(position_to_byte(&d, pos(0, 1)), 1);
    }

    #[test]
    fn position_splitting_surrogate_pair_rounds_down() {
        let d = Doc::from_str("😀");
        // character 1 lands between the emoji's two UTF-16 units —
        // round down to the code point start.
        assert_eq!(position_to_byte(&d, pos(0, 1)), 0);
        assert_eq!(position_to_byte(&d, pos(0, 2)), 4);
    }

    #[test]
    fn byte_inside_multibyte_char_maps_to_char_start() {
        let d = Doc::from_str("😀");
        assert_eq!(byte_to_position(&d, 2), pos(0, 0));
    }

    #[test]
    fn multibyte_on_later_lines() {
        let d = Doc::from_str("plain\n汉字 line\nx😀y");
        // "汉" starts line 1 at byte 6; each CJK char is 3 bytes,
        // 1 UTF-16 unit.
        assert_eq!(byte_to_position(&d, 6), pos(1, 0));
        assert_eq!(byte_to_position(&d, 12), pos(1, 2)); // after 汉字
        assert_eq!(position_to_byte(&d, pos(1, 2)), 12);
        // Line 2 starts at byte 18; after 'x' + emoji = character 3.
        assert_eq!(byte_to_position(&d, 23), pos(2, 3));
        assert_eq!(position_to_byte(&d, pos(2, 3)), 23);
    }

    #[test]
    fn clamps_past_line_end_and_doc_end() {
        let d = Doc::from_str("ab\ncd");
        // Character way past the line end clamps to before the \n.
        assert_eq!(position_to_byte(&d, pos(0, 99)), 2);
        // Line past the document clamps to doc end.
        assert_eq!(position_to_byte(&d, pos(9, 0)), 5);
        // Byte past the document clamps too.
        assert_eq!(byte_to_position(&d, 99), pos(1, 2));
    }

    #[test]
    fn empty_doc() {
        let d = Doc::from_str("");
        assert_eq!(byte_to_position(&d, 0), pos(0, 0));
        assert_eq!(position_to_byte(&d, pos(0, 0)), 0);
        assert_eq!(position_to_byte(&d, pos(3, 7)), 0);
    }

    #[test]
    fn trailing_newline() {
        let d = Doc::from_str("ab\n");
        // Byte 3 (doc end) is the start of the (empty) final line.
        assert_eq!(byte_to_position(&d, 3), pos(1, 0));
        assert_eq!(position_to_byte(&d, pos(1, 0)), 3);
        assert_eq!(position_to_byte(&d, pos(1, 5)), 3);
    }

    #[test]
    fn round_trips_every_char_boundary() {
        let d = Doc::from_str("héllo 😀 wörld\n汉字\nplain 🚀end\n");
        let text = d.to_string();
        for byte in text
            .char_indices()
            .map(|(i, _)| i)
            .chain(std::iter::once(text.len()))
        {
            let p = byte_to_position(&d, byte);
            assert_eq!(
                position_to_byte(&d, p),
                byte,
                "round-trip failed at byte {byte} (position {p:?})"
            );
        }
    }

    #[test]
    fn range_conversions() {
        let d = Doc::from_str("a😀b\ncd");
        let r = byte_range_to_range(&d, 1..6);
        assert_eq!(r.start, pos(0, 1));
        assert_eq!(r.end, pos(0, 4));
        assert_eq!(range_to_byte_range(&d, r), 1..6);
    }

    #[test]
    fn content_changes_single_insert() {
        let d = Doc::from_str("hello");
        let events = changes_to_content_changes(&d, &Changes::insert(5, " world"));
        assert_eq!(events.len(), 1);
        let range = events[0].range.unwrap();
        assert_eq!(range.start, pos(0, 5));
        assert_eq!(range.end, pos(0, 5));
        assert_eq!(events[0].text, " world");
    }

    #[test]
    fn content_changes_multiple_emitted_in_reverse_order() {
        // Two simultaneous edits against "hello world": replace both
        // words. LSP applies sequentially, so the later edit (byte
        // 6..11) must come first — its range is unaffected by the
        // earlier edit that hasn't been applied yet.
        let d = Doc::from_str("hello world");
        let changes = Changes::from_sorted(vec![
            Change::replace(0..5, "HI"),
            Change::replace(6..11, "rust"),
        ]);
        let events = changes_to_content_changes(&d, &changes);
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].range.unwrap().start, pos(0, 6));
        assert_eq!(events[0].text, "rust");
        assert_eq!(events[1].range.unwrap().start, pos(0, 0));
        assert_eq!(events[1].text, "HI");

        // Cross-check by simulating sequential application.
        let mut text = d.to_string();
        for ev in &events {
            let range = ev.range.unwrap();
            let cur = Doc::from_str(&text);
            let from = position_to_byte(&cur, range.start);
            let to = position_to_byte(&cur, range.end);
            text.replace_range(from..to, &ev.text);
        }
        assert_eq!(text, changes.apply(&d).to_string());
        assert_eq!(text, "HI rust");
    }

    #[test]
    fn content_changes_delete_with_multibyte_prefix() {
        // Deleting "b" after an emoji: byte range 5..6, but UTF-16
        // character 3..4.
        let d = Doc::from_str("a😀bc");
        let events = changes_to_content_changes(&d, &Changes::delete(5..6));
        let range = events[0].range.unwrap();
        assert_eq!(range.start, pos(0, 3));
        assert_eq!(range.end, pos(0, 4));
        assert_eq!(events[0].text, "");
    }
}
