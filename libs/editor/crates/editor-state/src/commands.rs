//! Built-in commands. These are intentionally tiny —
//! `fn(&EditorState) -> Option<TransactionSpec>` — so they're
//! testable in isolation and composable into any keymap.
//!
//! Mirrors `@codemirror/commands`. We add commands here as we
//! find we want them in the default keymap.

use crate::change::Changes;
use crate::selection::{Range, Selection};
use crate::state::EditorState;
use crate::transaction::TransactionSpec;

/// Select the entire document. Bound by convention to `Mod-a`.
#[must_use]
pub fn select_all(state: &EditorState) -> Option<TransactionSpec> {
    Some(TransactionSpec::new().selection(Selection::single(Range::new(0, state.doc.len()))))
}

/// Insert a newline at the caret. If there's a non-empty
/// selection, replace it with `"\n"`. Bound by convention to
/// `Enter`.
#[must_use]
pub fn insert_newline(state: &EditorState) -> Option<TransactionSpec> {
    let p = state.selection.primary();
    let (from, to) = (p.from(), p.to());
    Some(TransactionSpec::new().changes(Changes::replace(from..to, "\n")))
}

/// One unit of indentation. CM6 uses a configurable
/// `indentUnit` facet that defaults to two spaces; this is a
/// plain const for now and can be promoted to config later.
pub const INDENT_UNIT: &str = "  ";

/// Toggle a fold range. If a fold with the same `start` exists
/// it's removed; otherwise the given range is inserted sorted
/// by start. Called by the gutter / heading fold-arrow widgets.
#[must_use]
pub fn toggle_fold(state: &EditorState, range: std::ops::Range<usize>) -> Option<TransactionSpec> {
    let mut folds = state.folds.clone();
    let existing = folds.iter().position(|f| f.start == range.start);
    if let Some(i) = existing {
        folds.remove(i);
    } else {
        let pos = folds
            .iter()
            .position(|f| f.start > range.start)
            .unwrap_or(folds.len());
        folds.insert(pos, range);
    }
    Some(TransactionSpec::new().folds(folds))
}

/// Flip the reading-mode flag. Bound to `Mod-e` to match
/// Obsidian's "toggle preview" shortcut.
#[must_use]
pub fn toggle_reading_mode(state: &EditorState) -> Option<TransactionSpec> {
    Some(TransactionSpec::new().reading_mode(!state.reading_mode))
}

/// Try CM6-style "insertBracket" behavior for the given inserted
/// character. Returns a [`TransactionSpec`] when the character
/// should be handled specially (auto-close, skip-over, wrap-
/// selection) and `None` for plain insertion. Mirrors
/// `closebrackets/src/closebrackets.ts:129` (`insertBracket`).
///
/// Behaviors:
/// - `(` / `[` / `{` with an empty caret → insert `()` with
///   caret between.
/// - Same with a non-empty selection → wrap the selection.
/// - `)` / `]` / `}` adjacent to a matching close → skip over
///   it instead of inserting (caret moves +1, no text change).
/// - `'` / `"` / `` ` `` (same-char pairs): tap-to-skip when
///   the next char is already that quote.
#[must_use]
pub fn insert_bracket(state: &EditorState, input: &str) -> Option<TransactionSpec> {
    if input.chars().count() != 1 {
        return None;
    }
    let ch = input.chars().next().unwrap();
    let (open, close, same) = match ch {
        '(' => ('(', ')', false),
        '[' => ('[', ']', false),
        '{' => ('{', '}', false),
        ')' | ']' | '}' => return handle_close(state, ch),
        '\'' => ('\'', '\'', true),
        '"' => ('"', '"', true),
        '`' => ('`', '`', true),
        _ => return None,
    };

    let p = state.selection.primary();
    let (from, to) = (p.from(), p.to());
    let doc = state.doc.to_string();

    // Non-empty selection: wrap.
    if from != to {
        let mut inserted = String::new();
        inserted.push(open);
        inserted.push_str(&doc[from..to]);
        inserted.push(close);
        let caret_anchor = from;
        let caret_head = from + inserted.len();
        return Some(
            TransactionSpec::new()
                .changes(Changes::replace(from..to, &inserted))
                .selection(Selection::single(Range::new(caret_anchor, caret_head))),
        );
    }

    // Caret. For same-char quotes: skip if the next byte is the
    // same quote (covers re-typing a closing quote you don't
    // need to write).
    if same {
        let next_byte = doc.as_bytes().get(from).copied();
        if next_byte == Some(ch as u8) {
            return Some(TransactionSpec::new().selection(Selection::caret(from + 1)));
        }
    }

    // Auto-close only if the next char is whitespace, EOL, or
    // a "closing-before" char (`)`, `]`, `}`, `,`, `;`, `:`,
    // `>`). Mirrors CM6's `before` config
    // (`closebrackets.ts:21`).
    let next_byte = doc.as_bytes().get(from).copied();
    let can_close = match next_byte {
        None => true,
        Some(b) => {
            b == b' '
                || b == b'\n'
                || b == b'\t'
                || matches!(b, b')' | b']' | b'}' | b',' | b';' | b':' | b'>')
        }
    };
    if !can_close {
        return None;
    }
    let mut pair = String::new();
    pair.push(open);
    pair.push(close);
    Some(
        TransactionSpec::new()
            .changes(Changes::insert(from, &pair))
            .selection(Selection::caret(from + open.len_utf8())),
    )
}

/// Skip past a close bracket if the next byte is that exact
/// close and would be the matching one — `closebrackets.ts:180`
/// (`handleClose`). v1 takes a lighter heuristic than CM6
/// (which tracks "auto-inserted" via a StateField): if the
/// next char matches the closer the user typed, skip; otherwise
/// fall through to plain insert.
fn handle_close(state: &EditorState, close_char: char) -> Option<TransactionSpec> {
    let p = state.selection.primary();
    if p.anchor != p.head {
        return None;
    }
    let from = p.head;
    let next = state.doc.to_string().as_bytes().get(from).copied();
    if next == Some(close_char as u8) {
        return Some(TransactionSpec::new().selection(Selection::caret(from + 1)));
    }
    None
}

/// CM6's `deleteBracketPair`
/// (`closebrackets/src/closebrackets.ts:96`). When Backspace is
/// pressed on a caret sitting between a matching `()` / `[]` /
/// `{}` / `''` / `""` / ` `` ` pair, delete both characters at
/// once instead of just the opening one.
#[must_use]
pub fn delete_bracket_pair(state: &EditorState) -> Option<TransactionSpec> {
    let p = state.selection.primary();
    if p.anchor != p.head || p.head == 0 {
        return None;
    }
    let doc = state.doc.to_string();
    let bytes = doc.as_bytes();
    let prev = bytes.get(p.head - 1).copied()?;
    let next = bytes.get(p.head).copied()?;
    let matches = matches!(
        (prev, next),
        (b'(', b')') | (b'[', b']') | (b'{', b'}') | (b'\'', b'\'') | (b'"', b'"') | (b'`', b'`')
    );
    if !matches {
        return None;
    }
    Some(
        TransactionSpec::new()
            .changes(Changes::delete(p.head - 1..p.head + 1))
            .selection(Selection::caret(p.head - 1)),
    )
}

/// Enter — but if the caret is on a list / task item,
/// continue the list on the next line. On an *empty* list
/// item (marker followed by whitespace only), instead remove
/// the marker, exiting the list.
///
/// Ports CM6's `insertNewlineContinueMarkup`
/// (`lang-markdown/src/commands.ts:98`).
///
/// Falls back to a plain `\n` insert when the line isn't a list
/// item.
#[must_use]
pub fn enter_continue_list(state: &EditorState) -> Option<TransactionSpec> {
    let p = state.selection.primary();
    let (from, to) = (p.from(), p.to());
    if from != to {
        // Non-empty selection — defer to plain newline insert.
        return insert_newline(state);
    }
    let doc = state.doc.to_string();
    let (line_from, line_to) = line_bounds(&doc, from);
    let line = &doc[line_from..line_to];
    let cont = match parse_list_continuation(line) {
        Some(c) => c,
        None => return insert_newline(state),
    };
    // Empty item: marker + (optional task box) + whitespace,
    // nothing after. Strip the marker, exit the list.
    let content_starts_at = line_from + cont.marker_end;
    if content_starts_at >= from {
        // Caret is on the marker itself or right after it; line
        // has no real content yet. Delete the marker.
        let changes = Changes::delete(line_from..line_from + cont.marker_end);
        return Some(
            TransactionSpec::new()
                .changes(changes)
                .selection(Selection::caret(line_from)),
        );
    }
    // Build the continuation marker. Tasks always start
    // unchecked on the next line.
    let mut marker = String::new();
    marker.push_str(&cont.indent);
    marker.push_str(&cont.bq_prefix);
    match cont.kind {
        ListKind::Bullet(c) => {
            marker.push(c);
            marker.push_str(&cont.after);
        }
        ListKind::Ordered(n) => {
            marker.push_str(&(n + 1).to_string());
            marker.push('.');
            marker.push_str(&cont.after);
        }
        ListKind::Blockquote => {
            // bq_prefix already contains the `>` chain (with
            // trailing space).
        }
    }
    if cont.task {
        marker.push_str("[ ] ");
    }
    let insert = format!("\n{marker}");
    let caret = from + insert.len();
    let mut all_changes: Vec<crate::change::Change> = vec![crate::change::Change {
        from,
        to,
        inserted: insert.clone(),
    }];
    // Ordered lists: bump each consecutive following item's
    // number by 1 so the inserted `(n+1).` doesn't duplicate the
    // existing one. Mirrors CM6's `renumberList`
    // (`lang-markdown/src/commands.ts:66`).
    if let ListKind::Ordered(n) = cont.kind {
        // The newly inserted item has number `n + 1`. Pass that
        // as the starting expected value so the renumber walk
        // matches the displaced old-`n+1` item first.
        all_changes.extend(renumber_following_ordered(&doc, line_to, n + 1));
    }
    Some(
        TransactionSpec::new()
            .changes(Changes::from_sorted(all_changes))
            .selection(Selection::caret(caret)),
    )
}

/// Walk lines starting at `after_pos` (must be at the start of
/// the line right after the one Enter was pressed on, i.e.
/// `line_to` of the current line — *before* the `\n`). For each
/// consecutive ordered-list item whose number equals the one we'd
/// expect from the unbroken sequence, emit a Change that bumps it
/// by `+1`. Stops on sequence break or non-list line.
fn renumber_following_ordered(
    doc: &str,
    after_line_to: usize,
    inserted_number: u32,
) -> Vec<crate::change::Change> {
    let mut out = Vec::new();
    let bytes = doc.as_bytes();
    // Skip the trailing `\n` of the current line.
    let mut i = if after_line_to < bytes.len() && bytes[after_line_to] == b'\n' {
        after_line_to + 1
    } else {
        return out;
    };
    let mut expected_old = inserted_number;
    while i < bytes.len() {
        let mut line_end = i;
        while line_end < bytes.len() && bytes[line_end] != b'\n' {
            line_end += 1;
        }
        let line = &doc[i..line_end];
        // Find leading whitespace + digits + `.`.
        let leading = line.bytes().take_while(|&b| b == b' ').count();
        let digit_start = i + leading;
        let mut digit_end = digit_start;
        while digit_end < line_end && bytes[digit_end].is_ascii_digit() {
            digit_end += 1;
        }
        if digit_end == digit_start || bytes.get(digit_end) != Some(&b'.') {
            break;
        }
        let n: u32 = match doc[digit_start..digit_end].parse() {
            Ok(v) => v,
            Err(_) => break,
        };
        if n != expected_old {
            break;
        }
        out.push(crate::change::Change {
            from: digit_start,
            to: digit_end,
            inserted: (n + 1).to_string(),
        });
        expected_old = n + 1;
        if line_end >= bytes.len() {
            break;
        }
        i = line_end + 1;
    }
    out
}

/// Indent the line(s) intersecting the primary selection by one
/// [`INDENT_UNIT`]. Ports CM6's `indentMore`
/// (`commands/src/commands.ts:906`).
#[must_use]
pub fn indent_more(state: &EditorState) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let lines = selected_line_starts(state, &doc);
    if lines.is_empty() {
        return None;
    }
    let mut changes = Vec::with_capacity(lines.len());
    for &line_from in &lines {
        changes.push(crate::change::Change {
            from: line_from,
            to: line_from,
            inserted: INDENT_UNIT.to_string(),
        });
    }
    Some(TransactionSpec::new().changes(Changes::from_sorted(changes)))
}

/// Outdent — remove up to [`INDENT_UNIT`] worth of leading
/// whitespace from each selected line. Ports CM6's `indentLess`
/// (`commands/src/commands.ts:916`).
#[must_use]
pub fn indent_less(state: &EditorState) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let lines = selected_line_starts(state, &doc);
    if lines.is_empty() {
        return None;
    }
    let unit = INDENT_UNIT.len();
    let mut changes = Vec::new();
    for &line_from in &lines {
        let bytes = doc.as_bytes();
        let mut leading = 0;
        while leading < unit && bytes.get(line_from + leading) == Some(&b' ') {
            leading += 1;
        }
        if leading > 0 {
            changes.push(crate::change::Change {
                from: line_from,
                to: line_from + leading,
                inserted: String::new(),
            });
        }
    }
    if changes.is_empty() {
        return None;
    }
    Some(TransactionSpec::new().changes(Changes::from_sorted(changes)))
}

/// Byte offset of the previous word-group boundary from `pos` — CM6's
/// "group" semantics (`Ctrl-ArrowLeft` / `Alt-ArrowLeft` on mac): skip
/// any whitespace backward, then a run of same-class characters
/// (word chars vs punctuation). Newlines count as whitespace, so the
/// motion crosses line boundaries like the browser's does.
#[must_use]
pub fn word_boundary_left(state: &EditorState, pos: usize) -> usize {
    let rope = state.doc.rope();
    let mut ci = rope.byte_to_char(pos.min(rope.len_bytes()));
    while ci > 0 && char_class(rope.char(ci - 1)) == CharClass::Space {
        ci -= 1;
    }
    if ci > 0 {
        let cls = char_class(rope.char(ci - 1));
        while ci > 0 && char_class(rope.char(ci - 1)) == cls {
            ci -= 1;
        }
    }
    rope.char_to_byte(ci)
}

/// Byte offset of the next word-group boundary from `pos` — mirror of
/// [`word_boundary_left`].
#[must_use]
pub fn word_boundary_right(state: &EditorState, pos: usize) -> usize {
    let rope = state.doc.rope();
    let len = rope.len_chars();
    let mut ci = rope.byte_to_char(pos.min(rope.len_bytes()));
    while ci < len && char_class(rope.char(ci)) == CharClass::Space {
        ci += 1;
    }
    if ci < len {
        let cls = char_class(rope.char(ci));
        while ci < len && char_class(rope.char(ci)) == cls {
            ci += 1;
        }
    }
    rope.char_to_byte(ci)
}

/// Delete from the previous word-group boundary to the caret (CM6's
/// `deleteGroupBackward`, the `Ctrl-Backspace` default). A non-empty
/// selection deletes itself.
#[must_use]
pub fn delete_word_backward(state: &EditorState) -> Option<TransactionSpec> {
    let p = state.selection.primary();
    let (from, to) = (p.from(), p.to());
    if from != to {
        return Some(TransactionSpec::new().changes(Changes::delete(from..to)));
    }
    let target = word_boundary_left(state, from);
    if target == from {
        return None;
    }
    Some(TransactionSpec::new().changes(Changes::delete(target..from)))
}

/// Delete from the caret to the next word-group boundary (CM6's
/// `deleteGroupForward`, the `Ctrl-Delete` default).
#[must_use]
pub fn delete_word_forward(state: &EditorState) -> Option<TransactionSpec> {
    let p = state.selection.primary();
    let (from, to) = (p.from(), p.to());
    if from != to {
        return Some(TransactionSpec::new().changes(Changes::delete(from..to)));
    }
    let target = word_boundary_right(state, to);
    if target == to {
        return None;
    }
    Some(TransactionSpec::new().changes(Changes::delete(to..target)))
}

/// The Tab default action on a markdown list/task line: indent the item
/// (`dedent` = Shift-Tab, outdent) and renumber the surrounding ordered
/// sequences — the level the item left closes its gap, the level it
/// joined counts it in (an item opening a fresh sublevel restarts at
/// `1.`). Returns `None` when the caret line isn't a list item — the
/// caller inserts a literal tab (or nothing on Shift-Tab), matching
/// Obsidian's behavior.
#[must_use]
pub fn tab_list_indent(state: &EditorState, dedent: bool) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let (line_from, line_to) = line_bounds(&doc, state.selection.primary().head);
    parse_list_continuation(&doc[line_from..line_to])?;
    let lines = selected_line_starts(state, &doc);
    if lines.is_empty() {
        return None;
    }
    let unit = INDENT_UNIT.len();
    let bytes = doc.as_bytes();
    let mut changes: Vec<crate::change::Change> = Vec::new();
    // line start → signed byte delta its indentation receives; the
    // renumber walk below sees post-edit indent widths while emitting
    // original-doc ranges.
    let mut deltas: std::collections::HashMap<usize, isize> = std::collections::HashMap::new();
    for &lf in &lines {
        if dedent {
            let mut leading = 0;
            while leading < unit && bytes.get(lf + leading) == Some(&b' ') {
                leading += 1;
            }
            if leading > 0 {
                changes.push(crate::change::Change {
                    from: lf,
                    to: lf + leading,
                    inserted: String::new(),
                });
                deltas.insert(lf, -(leading as isize));
            }
        } else {
            changes.push(crate::change::Change {
                from: lf,
                to: lf,
                inserted: INDENT_UNIT.to_string(),
            });
            deltas.insert(lf, unit as isize);
        }
    }
    if changes.is_empty() {
        // Shift-Tab with nothing left to remove.
        return None;
    }
    changes.extend(renumber_block_with_deltas(&doc, &deltas));
    changes.sort_by_key(|c| (c.from, c.to));
    Some(TransactionSpec::new().changes(Changes::from_sorted(changes)))
}

/// Is `line` a plain (non-blockquote) list item — the lines the
/// block-renumber walk covers?
fn is_renumberable_list_line(line: &str) -> bool {
    parse_list_continuation(line).is_some_and(|c| {
        c.bq_prefix.is_empty() && matches!(c.kind, ListKind::Bullet(_) | ListKind::Ordered(_))
    })
}

/// Renumber the contiguous list block around the lines in `deltas`
/// (line start → pending indentation byte delta), emitting digit fixes
/// for every ordered item whose number no longer matches its
/// sequence. Levels are tracked by effective (post-edit) indent width:
/// a moved item opening a fresh sublevel restarts at `1.`; everything
/// else continues its level's count. Untouched items that OPEN a level
/// keep their number, so lists that deliberately start at `7.` stay
/// put. Emitted ranges are original-doc coordinates, disjoint from the
/// indent edits (digits sit after the leading whitespace).
fn renumber_block_with_deltas(
    doc: &str,
    deltas: &std::collections::HashMap<usize, isize>,
) -> Vec<crate::change::Change> {
    let Some(&first_changed) = deltas.keys().min() else {
        return Vec::new();
    };
    // Walk up to the top of the contiguous list block.
    let mut block_start = {
        let (lf, _) = line_bounds(doc, first_changed.min(doc.len()));
        lf
    };
    while block_start > 0 {
        let newline = block_start - 1;
        let (prev_from, _) = line_bounds(doc, newline);
        if is_renumberable_list_line(&doc[prev_from..newline]) {
            block_start = prev_from;
        } else {
            break;
        }
    }

    /// One open indentation level in the walk.
    struct Level {
        indent: usize,
        ordered: bool,
        counter: u32,
        started: bool,
    }
    let mut stack: Vec<Level> = Vec::new();
    let mut out = Vec::new();
    let bytes = doc.as_bytes();
    let mut i = block_start;
    loop {
        let (lf, lt) = line_bounds(doc, i);
        let line = &doc[lf..lt];
        if !is_renumberable_list_line(line) {
            break;
        }
        let cont = parse_list_continuation(line).expect("checked renumberable");
        let moved = deltas.get(&lf).copied().unwrap_or(0);
        let eff_indent = usize::try_from(cont.indent.len() as isize + moved).unwrap_or(0);
        let ordered = matches!(cont.kind, ListKind::Ordered(_));

        while stack.last().is_some_and(|l| l.indent > eff_indent) {
            stack.pop();
        }
        match stack.last_mut() {
            Some(l) if l.indent == eff_indent => {
                // A marker-kind flip at the same depth starts a new
                // markdown list — restart the count.
                if l.ordered != ordered {
                    l.ordered = ordered;
                    l.counter = 0;
                    l.started = false;
                }
            }
            _ => stack.push(Level {
                indent: eff_indent,
                ordered,
                counter: 0,
                started: false,
            }),
        }
        let level = stack.last_mut().expect("level pushed above");
        if let ListKind::Ordered(n) = cont.kind {
            let expected = if !level.started && moved == 0 {
                n
            } else if !level.started {
                1
            } else {
                level.counter + 1
            };
            level.started = true;
            level.counter = expected;
            if n != expected {
                let digit_start = lf + cont.indent.len();
                let mut digit_end = digit_start;
                while digit_end < lt && bytes[digit_end].is_ascii_digit() {
                    digit_end += 1;
                }
                out.push(crate::change::Change {
                    from: digit_start,
                    to: digit_end,
                    inserted: expected.to_string(),
                });
            }
        } else {
            level.started = true;
        }
        if lt >= doc.len() {
            break;
        }
        i = lt + 1;
    }
    out
}

#[derive(PartialEq, Eq, Clone, Copy)]
enum CharClass {
    Space,
    Word,
    Punct,
}

fn char_class(c: char) -> CharClass {
    if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

// ── helpers ─────────────────────────────────────────────────

fn line_bounds(doc: &str, pos: usize) -> (usize, usize) {
    let bytes = doc.as_bytes();
    let mut start = pos.min(bytes.len());
    while start > 0 && bytes[start - 1] != b'\n' {
        start -= 1;
    }
    let mut end = pos.min(bytes.len());
    while end < bytes.len() && bytes[end] != b'\n' {
        end += 1;
    }
    (start, end)
}

fn selected_line_starts(state: &EditorState, doc: &str) -> Vec<usize> {
    let p = state.selection.primary();
    let (from, to) = (p.from(), p.to());
    let (first_line, _) = line_bounds(doc, from);
    let (last_line, _) = if to > from {
        // If selection ends exactly on a newline, don't include
        // the next line.
        let probe = if to > 0 && doc.as_bytes()[to - 1] == b'\n' {
            to - 1
        } else {
            to
        };
        line_bounds(doc, probe)
    } else {
        line_bounds(doc, from)
    };
    let mut out = Vec::new();
    let bytes = doc.as_bytes();
    let mut i = first_line;
    while i <= last_line {
        out.push(i);
        while i < bytes.len() && bytes[i] != b'\n' {
            i += 1;
        }
        if i >= bytes.len() {
            break;
        }
        i += 1;
    }
    out
}

#[derive(Debug, Clone, Copy)]
enum ListKind {
    Bullet(char),
    Ordered(u32),
    /// Blockquote line with no inner list marker. Carries the
    /// blockquote-nesting depth (`>` for 1, `> >` for 2, etc.)
    /// so Enter reproduces the same depth on the next line.
    Blockquote,
}

struct ListContinuation {
    /// Verbatim prefix to repeat on Enter (indentation, any `>`
    /// chain, list marker / task box, trailing space). For
    /// ordered lists the `(n+1).` substitution is done after
    /// reconstruction in [`enter_continue_list`]; this string
    /// keeps the *original* marker bytes.
    indent: String,
    /// Combined `>` / `> >` / `> > >` blockquote prefix the
    /// new line should start with — empty if the line wasn't a
    /// blockquote.
    bq_prefix: String,
    kind: ListKind,
    after: String,
    task: bool,
    marker_end: usize,
}

fn parse_list_continuation(line: &str) -> Option<ListContinuation> {
    let bytes = line.as_bytes();
    let leading = bytes.iter().take_while(|&&c| c == b' ').count();

    // Consume any leading `>` / `> ` chain (with optional space
    // after each `>`) — supports nested blockquotes and the
    // common `> - foo` "list inside a blockquote" pattern.
    let mut i = leading;
    let mut bq_prefix = String::new();
    while bytes.get(i) == Some(&b'>') {
        bq_prefix.push('>');
        i += 1;
        if bytes.get(i) == Some(&b' ') {
            bq_prefix.push(' ');
            i += 1;
        }
    }
    let after_indent_pos = i;

    // After any `>` chain, look for a list marker. If none,
    // the line is a plain blockquote (or plain text if no `>`
    // either, in which case we bail).
    let inner_bytes = &bytes[after_indent_pos..];
    let (kind, after_marker) = match inner_bytes.first() {
        Some(c @ (b'-' | b'*' | b'+')) => (ListKind::Bullet(*c as char), 1),
        Some(c) if c.is_ascii_digit() => {
            let n_end = inner_bytes
                .iter()
                .take_while(|&&x| x.is_ascii_digit())
                .count();
            if inner_bytes.get(n_end) != Some(&b'.') {
                if bq_prefix.is_empty() {
                    return None;
                }
                // Pure blockquote — no inner list.
                return Some(ListContinuation {
                    indent: " ".repeat(leading),
                    bq_prefix,
                    kind: ListKind::Blockquote,
                    after: String::new(),
                    task: false,
                    marker_end: after_indent_pos,
                });
            }
            let n: u32 = std::str::from_utf8(&inner_bytes[..n_end])
                .ok()?
                .parse()
                .ok()?;
            (ListKind::Ordered(n), n_end + 1)
        }
        _ => {
            if bq_prefix.is_empty() {
                return None;
            }
            // Pure blockquote.
            return Some(ListContinuation {
                indent: " ".repeat(leading),
                bq_prefix,
                kind: ListKind::Blockquote,
                after: String::new(),
                task: false,
                marker_end: after_indent_pos,
            });
        }
    };
    let inner_start = after_indent_pos;
    let after_marker_abs = inner_start + after_marker;

    // Whitespace after the list marker.
    let ws_count = bytes[after_marker_abs..]
        .iter()
        .take_while(|&&x| x == b' ')
        .count();
    if ws_count == 0 && bytes.len() > after_marker_abs {
        // A bare `-foo` is not a list — bail (unless we're
        // already committed to a blockquote with valid markers
        // — but then we'd have returned earlier).
        if bq_prefix.is_empty() {
            return None;
        }
        return Some(ListContinuation {
            indent: " ".repeat(leading),
            bq_prefix,
            kind: ListKind::Blockquote,
            after: String::new(),
            task: false,
            marker_end: after_indent_pos,
        });
    }
    let after = " ".repeat(ws_count.max(1));
    let mut marker_end = after_marker_abs + ws_count;

    // Optional task box `[ ]` / `[x]`.
    let task = bytes.get(marker_end..marker_end + 3).is_some_and(|sl| {
        sl.len() == 3 && sl[0] == b'[' && sl[2] == b']' && matches!(sl[1], b' ' | b'x' | b'X')
    });
    if task {
        marker_end += 3;
        if bytes.get(marker_end) == Some(&b' ') {
            marker_end += 1;
        }
    }
    Some(ListContinuation {
        indent: " ".repeat(leading),
        bq_prefix,
        kind,
        after,
        task,
        marker_end,
    })
}

/// Delete the character before the caret. With a non-empty
/// selection, deletes the selection. Bound by convention to
/// `Backspace`.
///
/// First tries [`delete_bracket_pair`] so Backspace between an
/// empty `()` / `[]` / `{}` / `""` / `''` / ` `` ` pair deletes
/// both characters, matching CM6's `closeBracketsKeymap`.
#[must_use]
pub fn delete_backward(state: &EditorState) -> Option<TransactionSpec> {
    if let Some(spec) = delete_bracket_pair(state) {
        return Some(spec);
    }
    let p = state.selection.primary();
    let (from, to) = (p.from(), p.to());
    if from != to {
        return Some(TransactionSpec::new().changes(Changes::delete(from..to)));
    }
    if from == 0 {
        return None;
    }
    // Char-wise via the rope — `from - 1` would split a multi-byte
    // character and panic downstream on the invalid boundary.
    let rope = state.doc.rope();
    let ci = rope.byte_to_char(from);
    let prev = rope.char_to_byte(ci - 1);
    Some(TransactionSpec::new().changes(Changes::delete(prev..from)))
}

/// Toggle bold markdown markers (`**…**`) at the caret /
/// around the current selection. Behavior:
///
/// - **Empty caret, doc[caret..] starts with `**`**: caret is
///   sitting just before a closing marker (typical "I'm done
///   typing bold content" case). Skip past it — no doc change,
///   just move the caret +2.
/// - **Empty caret elsewhere**: insert `****` and park the
///   caret between the markers, so subsequent typing goes
///   inside the bold span.
/// - **Non-empty selection**: wrap the selection with `**…**`,
///   keeping the wrapped range selected.
///
/// Bound by convention to `Mod-b`.
#[must_use]
pub fn toggle_bold(state: &EditorState) -> Option<TransactionSpec> {
    toggle_marker(state, "**")
}

/// Same as [`toggle_bold`] but with single `*…*` for italic.
/// Bound to `Mod-i`.
#[must_use]
pub fn toggle_italic(state: &EditorState) -> Option<TransactionSpec> {
    toggle_marker(state, "*")
}

fn toggle_marker(state: &EditorState, marker: &str) -> Option<TransactionSpec> {
    let sel = state.selection.primary();
    let from = sel.from();
    let to = sel.to();
    let doc = state.doc.to_string();
    let m = marker;
    let mlen = m.len();

    if from == to {
        // Empty caret. If the next bytes are the marker, skip
        // past it — closes an open span the user just filled.
        if doc.get(from..).is_some_and(|s| s.starts_with(m)) {
            return Some(TransactionSpec::new().selection(Selection::caret(from + mlen)));
        }
        // Open a new span: insert "marker + marker" with caret
        // in the middle.
        let pair = format!("{m}{m}");
        return Some(
            TransactionSpec::new()
                .changes(Changes::insert(from, pair))
                .selection(Selection::caret(from + mlen)),
        );
    }
    // Wrap the selection.
    let selected = doc.get(from..to).unwrap_or("");
    let wrapped = format!("{m}{selected}{m}");
    let new_to = from + wrapped.len();
    Some(
        TransactionSpec::new()
            .changes(Changes::replace(from..to, wrapped))
            .selection(Selection::single(Range::new(from, new_to))),
    )
}

/// `Mod-k` — wrap the selection in `[…](url)`. Empty selection
/// inserts `[]()` with the caret between the brackets so the
/// user types the link text first. With a selection that looks
/// like an existing link (`[text](url)`) the markers are
/// stripped (toggle behavior).
#[must_use]
pub fn toggle_link(state: &EditorState) -> Option<TransactionSpec> {
    let sel = state.selection.primary();
    let (from, to) = (sel.from(), sel.to());
    let doc = state.doc.to_string();
    if from == to {
        let insert = "[]()";
        return Some(
            TransactionSpec::new()
                .changes(Changes::insert(from, insert))
                .selection(Selection::caret(from + 1)),
        );
    }
    let body = doc.get(from..to).unwrap_or("");
    // Toggle: if the body is already `[…](…)`, strip back to the
    // inner text. Else wrap.
    if body.starts_with('[') && body.ends_with(')') {
        if let Some(rb) = body.find("](") {
            let inner = &body[1..rb];
            let inner_owned = inner.to_string();
            let new_to = from + inner_owned.len();
            return Some(
                TransactionSpec::new()
                    .changes(Changes::replace(from..to, inner_owned))
                    .selection(Selection::single(Range::new(from, new_to))),
            );
        }
    }
    let wrapped = format!("[{body}]()");
    let url_caret = from + wrapped.len() - 1; // inside the `()`
    Some(
        TransactionSpec::new()
            .changes(Changes::replace(from..to, wrapped))
            .selection(Selection::caret(url_caret)),
    )
}

/// `Mod-1` … `Mod-6` — set the current line's heading level.
/// Strips any existing `#…#` prefix first, then prepends the
/// new one. Level `0` removes the heading entirely. Operates on
/// every line covered by the selection.
#[must_use]
pub fn set_heading(state: &EditorState, level: u8) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let starts = selected_line_starts(state, &doc);
    if starts.is_empty() {
        return None;
    }
    let mut changes: Vec<crate::Change> = Vec::new();
    for line_start in starts {
        let line_end = doc[line_start..]
            .find('\n')
            .map_or(doc.len(), |n| line_start + n);
        let line = &doc[line_start..line_end];
        // Strip existing prefix.
        let hashes = line.chars().take_while(|c| *c == '#').count();
        let strip_to =
            if (1..=6).contains(&hashes) && line.as_bytes().get(hashes).copied() == Some(b' ') {
                hashes + 1
            } else {
                0
            };
        let body = &line[strip_to..];
        let new_line = if level == 0 {
            body.to_string()
        } else {
            let prefix = "#".repeat(level as usize);
            format!("{prefix} {body}")
        };
        changes.push(crate::Change {
            from: line_start,
            to: line_end,
            inserted: new_line,
        });
    }
    Some(
        TransactionSpec::new()
            .changes(Changes::from_sorted(changes))
            .selection(state.selection.clone()),
    )
}

/// `Mod-l` — cycle the current line's list marker through
/// `none → -  → 1. → - [ ] → none`. Operates on every line in
/// the selection, snapping all of them to the same target so a
/// multi-line cycle stays predictable.
#[must_use]
pub fn cycle_list(state: &EditorState) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let starts = selected_line_starts(state, &doc);
    if starts.is_empty() {
        return None;
    }
    // Determine the first line's current state to compute the
    // target for the whole batch.
    let first_state = list_marker_state(&doc, starts[0]);
    let target = first_state.next();
    let mut changes: Vec<crate::Change> = Vec::new();
    for line_start in starts {
        let line_end = doc[line_start..]
            .find('\n')
            .map_or(doc.len(), |n| line_start + n);
        let current = list_marker_state(&doc, line_start);
        let body_start = line_start + current.prefix_bytes();
        let body = doc.get(body_start..line_end).unwrap_or("");
        let new_line = target.apply_to(body);
        changes.push(crate::Change {
            from: line_start,
            to: line_end,
            inserted: new_line,
        });
    }
    Some(
        TransactionSpec::new()
            .changes(Changes::from_sorted(changes))
            .selection(state.selection.clone()),
    )
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum ListMarkerState {
    None,
    Unordered,     // `- `
    Ordered,       // `1. `
    UnorderedTask, // `- [ ] `
}

impl ListMarkerState {
    fn next(self) -> Self {
        match self {
            Self::None => Self::Unordered,
            Self::Unordered => Self::Ordered,
            Self::Ordered => Self::UnorderedTask,
            Self::UnorderedTask => Self::None,
        }
    }
    fn prefix_bytes(self) -> usize {
        match self {
            Self::None => 0,
            Self::Unordered => 2,
            Self::Ordered => 3,
            Self::UnorderedTask => 6,
        }
    }
    fn apply_to(self, body: &str) -> String {
        match self {
            Self::None => body.to_string(),
            Self::Unordered => format!("- {body}"),
            Self::Ordered => format!("1. {body}"),
            Self::UnorderedTask => format!("- [ ] {body}"),
        }
    }
}

fn list_marker_state(doc: &str, line_start: usize) -> ListMarkerState {
    let line = doc.get(line_start..).unwrap_or("");
    let line = line.split('\n').next().unwrap_or("");
    let b = line.as_bytes();
    if b.len() >= 6
        && (b[0] == b'-' || b[0] == b'*' || b[0] == b'+')
        && b[1] == b' '
        && b[2] == b'['
        && b[4] == b']'
        && b[5] == b' '
    {
        return ListMarkerState::UnorderedTask;
    }
    if b.len() >= 3 && b[0].is_ascii_digit() && b[1] == b'.' && b[2] == b' ' {
        return ListMarkerState::Ordered;
    }
    if b.len() >= 2 && (b[0] == b'-' || b[0] == b'*' || b[0] == b'+') && b[1] == b' ' {
        return ListMarkerState::Unordered;
    }
    ListMarkerState::None
}

/// `Mod-Shift-K` — give the block at the caret a UUID and
/// write `id:: <uuid>` on the line below. If the block already
/// has an id line, the existing UUID is reused.
///
/// Returns `(TransactionSpec, ref_string)` so callers can
/// also copy `((uuid))` to the clipboard.
#[must_use]
pub fn add_block_id(state: &EditorState) -> Option<(TransactionSpec, String)> {
    let doc = state.doc.to_string();
    let caret = state.selection.primary().head.min(doc.len());
    let line_start = doc[..caret].rfind('\n').map_or(0, |n| n + 1);
    let line_end = doc[line_start..]
        .find('\n')
        .map_or(doc.len(), |n| line_start + n);
    let line = &doc[line_start..line_end];
    if line.trim().is_empty() {
        return None;
    }
    // If the next line is already `id:: <uuid>`, reuse it.
    if line_end < doc.len() {
        let next_start = line_end + 1;
        let next_end = doc[next_start..]
            .find('\n')
            .map_or(doc.len(), |n| next_start + n);
        let next_line = &doc[next_start..next_end];
        if let Some(uuid) = next_line.strip_prefix("id:: ") {
            let uuid = uuid.trim();
            if uuid.len() == 36 {
                return Some((
                    TransactionSpec::new().selection(state.selection.clone()),
                    format!("(({uuid}))"),
                ));
            }
        }
    }
    let uuid = uuid::Uuid::now_v7().to_string();
    let insert = format!("\nid:: {uuid}");
    let prev_sel = state.selection.clone();
    Some((
        TransactionSpec::new()
            .changes(Changes::insert(line_end, insert))
            .selection(prev_sel)
            .annotate("origin", "block-id"),
        format!("(({uuid}))"),
    ))
}

/// `Mod-t` — toggle the task checkbox on the current line.
/// `[ ]` ↔ `[x]`. Non-task lines first promote to `- [ ]` (cycle
/// → task) so the user can `Mod-t` an empty line and start
/// checking off immediately.
#[must_use]
pub fn toggle_task(state: &EditorState) -> Option<TransactionSpec> {
    let doc = state.doc.to_string();
    let starts = selected_line_starts(state, &doc);
    if starts.is_empty() {
        return None;
    }
    let mut changes: Vec<crate::Change> = Vec::new();
    for line_start in starts {
        let line_end = doc[line_start..]
            .find('\n')
            .map_or(doc.len(), |n| line_start + n);
        let line = &doc[line_start..line_end];
        let b = line.as_bytes();
        let is_task = b.len() >= 5
            && (b[0] == b'-' || b[0] == b'*' || b[0] == b'+')
            && b[1] == b' '
            && b[2] == b'['
            && b[4] == b']';
        let new_line = if is_task {
            let inner = b[3];
            let new_inner = if inner == b' ' { 'x' } else { ' ' };
            let mut bytes = b.to_vec();
            bytes[3] = new_inner as u8;
            String::from_utf8(bytes).unwrap_or_else(|_| line.to_string())
        } else {
            // Promote to a task line.
            format!("- [ ] {line}")
        };
        changes.push(crate::Change {
            from: line_start,
            to: line_end,
            inserted: new_line,
        });
    }
    Some(
        TransactionSpec::new()
            .changes(Changes::from_sorted(changes))
            .selection(state.selection.clone()),
    )
}

/// Delete the character after the caret. With a non-empty
/// selection, deletes the selection. Bound by convention to
/// `Delete`.
#[must_use]
pub fn delete_forward(state: &EditorState) -> Option<TransactionSpec> {
    let p = state.selection.primary();
    let (from, to) = (p.from(), p.to());
    if from != to {
        return Some(TransactionSpec::new().changes(Changes::delete(from..to)));
    }
    if to >= state.doc.len() {
        return None;
    }
    // Char-wise via the rope — `to + 1` would split a multi-byte
    // character and panic downstream on the invalid boundary.
    let rope = state.doc.rope();
    let ci = rope.byte_to_char(to);
    let next = rope.char_to_byte(ci + 1);
    Some(TransactionSpec::new().changes(Changes::delete(to..next)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, caret: usize) -> EditorState {
        let mut s = EditorState::new(text);
        s.selection = Selection::caret(caret);
        s
    }

    #[test]
    fn enter_continues_bullet_list() {
        let s = at("- foo", 5);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "- foo\n- ");
        assert_eq!(next.selection.primary().head, 8);
    }

    #[test]
    fn enter_continues_ordered_list_increments() {
        let s = at("1. foo", 6);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "1. foo\n2. ");
        assert_eq!(next.selection.primary().head, 10);
    }

    #[test]
    fn bracket_open_inserts_pair_with_caret_between() {
        let s = at("", 0);
        let next = s.update(insert_bracket(&s, "(").unwrap());
        assert_eq!(next.doc.to_string(), "()");
        assert_eq!(next.selection.primary().head, 1);
    }

    #[test]
    fn bracket_open_does_not_close_when_next_is_word_char() {
        let s = at("foo", 0);
        // Next byte is 'f' — should NOT auto-close.
        assert!(insert_bracket(&s, "(").is_none());
    }

    #[test]
    fn bracket_open_wraps_selection() {
        let mut s = EditorState::new("hello");
        s.selection = Selection::single(Range::new(0, 5));
        let next = s.update(insert_bracket(&s, "(").unwrap());
        assert_eq!(next.doc.to_string(), "(hello)");
        let p = next.selection.primary();
        assert_eq!((p.from(), p.to()), (0, 7));
    }

    #[test]
    fn bracket_close_skips_over_matching_close() {
        // Setup: caret at position 4 in "abc)def".
        let s = at("abc)def", 3);
        // Type `)` — should skip the existing close instead of
        // inserting a duplicate.
        let next = s.update(insert_bracket(&s, ")").unwrap());
        assert_eq!(next.doc.to_string(), "abc)def");
        assert_eq!(next.selection.primary().head, 4);
    }

    #[test]
    fn quote_skips_when_next_is_same_quote() {
        // Caret at 1 sits between `a` and `"`. Typing `"` should
        // hop over the existing quote instead of inserting another.
        let s = at("a\"b", 1);
        let next = s.update(insert_bracket(&s, "\"").unwrap());
        assert_eq!(next.doc.to_string(), "a\"b");
        assert_eq!(next.selection.primary().head, 2);
    }

    #[test]
    fn delete_bracket_pair_collapses_empty_pair() {
        let s = at("()", 1);
        let next = s.update(delete_bracket_pair(&s).unwrap());
        assert_eq!(next.doc.to_string(), "");
        assert_eq!(next.selection.primary().head, 0);
    }

    #[test]
    fn delete_bracket_pair_no_op_outside_pair() {
        let s = at("(a)", 2);
        assert!(delete_bracket_pair(&s).is_none());
    }

    #[test]
    fn enter_continues_blockquote() {
        let s = at("> quoted", 8);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "> quoted\n> ");
        assert_eq!(next.selection.primary().head, 11);
    }

    #[test]
    fn enter_continues_nested_blockquote() {
        let s = at("> > deep", 8);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "> > deep\n> > ");
    }

    #[test]
    fn enter_on_empty_blockquote_exits() {
        let s = at("> ", 2);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "");
    }

    #[test]
    fn enter_continues_list_inside_blockquote() {
        let s = at("> - item", 8);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "> - item\n> - ");
    }

    #[test]
    fn enter_renumbers_subsequent_ordered_items() {
        let s = at("1. one\n2. two\n3. three", 6);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "1. one\n2. \n3. two\n4. three");
        // Caret right after the new `2. ` marker.
        assert_eq!(next.selection.primary().head, 10);
    }

    #[test]
    fn enter_renumber_stops_at_sequence_break() {
        let s = at("1. one\n2. two\n5. five", 6);
        let next = s.update(enter_continue_list(&s).unwrap());
        // Only the first following item gets bumped; the `5.`
        // stays untouched because the sequence already broke.
        assert_eq!(next.doc.to_string(), "1. one\n2. \n3. two\n5. five");
    }

    #[test]
    fn enter_on_empty_list_item_exits_list() {
        let s = at("- ", 2);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "");
        assert_eq!(next.selection.primary().head, 0);
    }

    #[test]
    fn enter_continues_task_unchecked_after_checked() {
        let s = at("- [x] done", 10);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "- [x] done\n- [ ] ");
        assert_eq!(next.selection.primary().head, 17);
    }

    #[test]
    fn enter_outside_list_falls_back_to_newline() {
        let s = at("plain", 5);
        let next = s.update(enter_continue_list(&s).unwrap());
        assert_eq!(next.doc.to_string(), "plain\n");
    }

    #[test]
    fn indent_more_inserts_two_spaces() {
        let s = at("foo", 1);
        let next = s.update(indent_more(&s).unwrap());
        assert_eq!(next.doc.to_string(), "  foo");
    }

    #[test]
    fn indent_less_removes_leading_pair() {
        let s = at("  foo", 2);
        let next = s.update(indent_less(&s).unwrap());
        assert_eq!(next.doc.to_string(), "foo");
    }

    #[test]
    fn indent_less_at_zero_is_noop() {
        let s = at("foo", 0);
        assert!(indent_less(&s).is_none());
    }

    #[test]
    fn indent_more_across_selection_indents_each_line() {
        let mut s = EditorState::new("a\nb\nc");
        s.selection = Selection::single(Range::new(0, 5));
        let next = s.update(indent_more(&s).unwrap());
        assert_eq!(next.doc.to_string(), "  a\n  b\n  c");
    }

    #[test]
    fn tab_indents_ordered_item_restarts_sublevel_and_closes_gap() {
        // Tab on `2. b`: it becomes a fresh sublevel (`1.`) and the
        // top level closes the gap (`3. c` → `2.`).
        let s = at("1. a\n2. b\n3. c", 8); // caret inside "2. b"
        let next = s.update(tab_list_indent(&s, false).unwrap());
        assert_eq!(next.doc.to_string(), "1. a\n  1. b\n2. c");
    }

    #[test]
    fn shift_tab_outdents_into_parent_sequence() {
        // Shift-Tab on the nested `1. x`: it joins the parent
        // sequence as `2.` and the old `2. b` becomes `3.`.
        let s = at("1. a\n  1. x\n2. b", 10); // caret inside "  1. x"
        let next = s.update(tab_list_indent(&s, true).unwrap());
        assert_eq!(next.doc.to_string(), "1. a\n2. x\n3. b");
    }

    #[test]
    fn tab_indent_renumbers_following_siblings_of_new_level() {
        // Indenting `3. c` under an existing sublevel appends to that
        // sublevel's sequence instead of restarting it.
        let s = at("1. a\n2. b\n  1. x\n3. c", 20); // caret inside "3. c"
        let next = s.update(tab_list_indent(&s, false).unwrap());
        assert_eq!(next.doc.to_string(), "1. a\n2. b\n  1. x\n  2. c");
    }

    #[test]
    fn tab_bullet_item_indents_without_touching_numbers() {
        let s = at("1. a\n- b\n2. c", 6); // caret inside "- b"
        let next = s.update(tab_list_indent(&s, false).unwrap());
        assert_eq!(next.doc.to_string(), "1. a\n  - b\n2. c");
    }

    #[test]
    fn tab_outside_list_returns_none() {
        let s = at("plain text", 3);
        assert!(tab_list_indent(&s, false).is_none());
        assert!(tab_list_indent(&s, true).is_none());
    }

    #[test]
    fn untouched_list_start_number_is_preserved() {
        // A list deliberately starting at 7 keeps its base when a
        // later item is indented.
        let s = at("7. a\n8. b\n9. c", 8); // caret inside "8. b"
        let next = s.update(tab_list_indent(&s, false).unwrap());
        assert_eq!(next.doc.to_string(), "7. a\n  1. b\n8. c");
    }

    #[test]
    fn select_all_covers_doc() {
        let s = EditorState::new("hello");
        let spec = select_all(&s).unwrap();
        let next = s.update(spec);
        let p = next.selection.primary();
        assert_eq!(p.from(), 0);
        assert_eq!(p.to(), 5);
    }

    #[test]
    fn delete_backward_at_pos_5() {
        let mut s = EditorState::new("hello");
        s.selection = Selection::caret(5);
        let next = s.update(delete_backward(&s).unwrap());
        assert_eq!(next.doc.to_string(), "hell");
        assert_eq!(next.selection.primary().head, 4);
    }

    #[test]
    fn delete_backward_at_start_is_noop() {
        let mut s = EditorState::new("hello");
        s.selection = Selection::caret(0);
        assert!(delete_backward(&s).is_none());
    }

    #[test]
    fn delete_backward_with_selection_deletes_range() {
        let mut s = EditorState::new("hello");
        s.selection = Selection::single(Range::new(1, 4));
        let next = s.update(delete_backward(&s).unwrap());
        assert_eq!(next.doc.to_string(), "ho");
    }

    #[test]
    fn toggle_bold_with_empty_caret_inserts_pair() {
        let mut s = EditorState::new("Testing ");
        s.selection = Selection::caret(8);
        let next = s.update(toggle_bold(&s).unwrap());
        assert_eq!(next.doc.to_string(), "Testing ****");
        // Caret parked between the markers.
        assert_eq!(next.selection.primary().head, 10);
        assert_eq!(next.selection.primary().anchor, 10);
    }

    #[test]
    fn toggle_bold_skips_past_closing_marker() {
        // "Testing **bold**" with caret at 14 (just after
        // "bold", before closing "**"). Pressing toggle_bold
        // should move caret to 16 without changing doc.
        let mut s = EditorState::new("Testing **bold**");
        s.selection = Selection::caret(14);
        let next = s.update(toggle_bold(&s).unwrap());
        assert_eq!(next.doc.to_string(), "Testing **bold**"); // unchanged
        assert_eq!(next.selection.primary().head, 16);
    }

    #[test]
    fn toggle_bold_wraps_selection() {
        let mut s = EditorState::new("Make this bold");
        s.selection = Selection::single(Range::new(5, 9)); // "this"
        let next = s.update(toggle_bold(&s).unwrap());
        assert_eq!(next.doc.to_string(), "Make **this** bold");
        let p = next.selection.primary();
        assert_eq!(p.from(), 5);
        assert_eq!(p.to(), 13); // covers **this**
    }

    #[test]
    fn toggle_italic_uses_single_marker() {
        let mut s = EditorState::new("foo");
        s.selection = Selection::caret(3);
        let next = s.update(toggle_italic(&s).unwrap());
        assert_eq!(next.doc.to_string(), "foo**");
        assert_eq!(next.selection.primary().head, 4);
    }

    #[test]
    fn toggle_link_inserts_empty_link_with_caret_in_text() {
        let s = at("", 0);
        let next = s.update(toggle_link(&s).unwrap());
        assert_eq!(next.doc.to_string(), "[]()");
        assert_eq!(next.selection.primary().head, 1);
    }

    #[test]
    fn toggle_link_wraps_selection() {
        let mut s = at("hello world", 0);
        s.selection = Selection::single(Range::new(0, 5));
        let next = s.update(toggle_link(&s).unwrap());
        assert_eq!(next.doc.to_string(), "[hello]() world");
        // Caret ends inside the (url) parens.
        assert_eq!(next.selection.primary().head, 8);
    }

    #[test]
    fn set_heading_prepends_hashes() {
        let s = at("hello", 3);
        let next = s.update(set_heading(&s, 2).unwrap());
        assert_eq!(next.doc.to_string(), "## hello");
    }

    #[test]
    fn set_heading_replaces_existing_level() {
        let s = at("# hello", 3);
        let next = s.update(set_heading(&s, 3).unwrap());
        assert_eq!(next.doc.to_string(), "### hello");
    }

    #[test]
    fn set_heading_zero_strips() {
        let s = at("### hello", 3);
        let next = s.update(set_heading(&s, 0).unwrap());
        assert_eq!(next.doc.to_string(), "hello");
    }

    #[test]
    fn cycle_list_walks_through_marker_states() {
        let s = at("foo", 0);
        let s = s.update(cycle_list(&s).unwrap());
        assert_eq!(s.doc.to_string(), "- foo");
        let s = s.update(cycle_list(&s).unwrap());
        assert_eq!(s.doc.to_string(), "1. foo");
        let s = s.update(cycle_list(&s).unwrap());
        assert_eq!(s.doc.to_string(), "- [ ] foo");
        let s = s.update(cycle_list(&s).unwrap());
        assert_eq!(s.doc.to_string(), "foo");
    }

    #[test]
    fn toggle_task_flips_existing_checkbox() {
        let s = at("- [ ] thing", 0);
        let s = s.update(toggle_task(&s).unwrap());
        assert_eq!(s.doc.to_string(), "- [x] thing");
        let s = s.update(toggle_task(&s).unwrap());
        assert_eq!(s.doc.to_string(), "- [ ] thing");
    }

    #[test]
    fn toggle_task_promotes_non_task_line() {
        let s = at("just a paragraph", 0);
        let s = s.update(toggle_task(&s).unwrap());
        assert_eq!(s.doc.to_string(), "- [ ] just a paragraph");
    }

    #[test]
    fn add_block_id_inserts_id_line_below_block() {
        let s = at("block content", 3);
        let (spec, ref_str) = add_block_id(&s).unwrap();
        let s = s.update(spec);
        let doc = s.doc.to_string();
        assert!(doc.starts_with("block content\nid:: "));
        // Ref string surrounds the same UUID that was inserted.
        let line2 = doc.lines().nth(1).unwrap();
        let uuid = line2.strip_prefix("id:: ").unwrap();
        assert_eq!(ref_str, format!("(({uuid}))"));
    }

    #[test]
    fn add_block_id_reuses_existing_id() {
        let uuid = "5f9c1234-abcd-4ef0-8123-fedcba012345";
        let src = format!("block content\nid:: {uuid}");
        let s = at(&src, 3);
        let (spec, ref_str) = add_block_id(&s).unwrap();
        let s2 = s.update(spec);
        // Doc unchanged.
        assert_eq!(s2.doc.to_string(), src);
        assert_eq!(ref_str, format!("(({uuid}))"));
    }

    #[test]
    fn add_block_id_refuses_blank_line() {
        let s = at("", 0);
        assert!(add_block_id(&s).is_none());
    }
}
