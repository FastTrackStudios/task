//! End-to-end tests for the vim state machine. Each test drives
//! `handle_key` against an `EditorState`, applies the returned
//! `TransactionSpec`, and asserts on the resulting doc + caret.

use editor_state::{EditorState, KeySpec, Range, Selection};
use editor_vim::{VimState, handle_key};

fn k(ch: &str) -> KeySpec {
    KeySpec {
        key: ch.to_string(),
        ..Default::default()
    }
}

fn state_with_caret(text: &str, caret: usize) -> EditorState {
    let mut s = EditorState::new(text);
    s.selection = Selection::caret(caret);
    s
}

fn drive(state: EditorState, vim: &mut VimState, keys: &[&str]) -> EditorState {
    let mut s = state;
    for key in keys {
        let key = k(key);
        if let Some(spec) = handle_key(&s, vim, &key) {
            s = s.update(spec);
        }
    }
    s
}

#[test]
fn hjkl_basic_movement() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc\ndef", 0);
    let s = drive(s, &mut vim, &["l", "l"]);
    assert_eq!(s.selection.primary(), Range::caret(2));
    let s = drive(s, &mut vim, &["j"]);
    assert_eq!(s.selection.primary().head, 6); // "abc\nde[f]"
    let s = drive(s, &mut vim, &["h"]);
    assert_eq!(s.selection.primary().head, 5);
    let s = drive(s, &mut vim, &["k"]);
    assert_eq!(s.selection.primary().head, 1);
}

#[test]
fn hjkl_stays_in_bounds() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc\ndef", 0);
    // many h's at start stay put
    let s = drive(s, &mut vim, &["h", "h", "h"]);
    assert_eq!(s.selection.primary().head, 0);
    // k at top stays put
    let s = drive(s, &mut vim, &["k"]);
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn w_advances_to_next_word() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar baz", 0);
    let s = drive(s, &mut vim, &["w"]);
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn dw_deletes_word() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar", 0);
    let s = drive(s, &mut vim, &["d", "w"]);
    assert_eq!(s.doc.to_string(), "bar");
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn daw_deletes_a_word() {
    let mut vim = VimState::new();
    let s = state_with_caret(" foo ", 2);
    let s = drive(s, &mut vim, &["d", "a", "w"]);
    assert_eq!(s.doc.to_string(), "");
}

#[test]
fn yyp_duplicates_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("line", 0);
    let s = drive(s, &mut vim, &["y", "y", "p"]);
    assert_eq!(s.doc.to_string(), "line\nline");
}

#[test]
fn big_w_advances_past_punctuation() {
    // `foo.bar baz` — `foo.bar` is one WORD, so W from 0 lands
    // at the start of `baz` (byte 8), not at the `.`.
    let mut vim = VimState::new();
    let s = state_with_caret("foo.bar baz", 0);
    let s = drive(s, &mut vim, &["W"]);
    assert_eq!(s.selection.primary().head, 8);
}

#[test]
fn big_b_walks_back_past_punctuation() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo.bar baz", 11);
    let s = drive(s, &mut vim, &["B"]);
    assert_eq!(s.selection.primary().head, 8);
}

#[test]
fn big_e_lands_on_last_byte_of_word() {
    // From pos 0 in `foo.bar baz`, E lands on byte 6 (the `r`).
    let mut vim = VimState::new();
    let s = state_with_caret("foo.bar baz", 0);
    let s = drive(s, &mut vim, &["E"]);
    assert_eq!(s.selection.primary().head, 6);
}

#[test]
fn count_three_big_w() {
    // `a.b c.d e.f g` — three WORDs forward from 0 → start of `g`.
    let mut vim = VimState::new();
    let s = state_with_caret("a.b c.d e.f g", 0);
    let s = drive(s, &mut vim, &["3", "W"]);
    assert_eq!(s.selection.primary().head, 12);
}

#[test]
fn count_three_w() {
    let mut vim = VimState::new();
    let s = state_with_caret("a b c d", 0);
    let s = drive(s, &mut vim, &["3", "w"]);
    assert_eq!(s.selection.primary().head, 6);
}

#[test]
fn i_enters_insert_mode() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 1);
    let _ = drive(s, &mut vim, &["i"]);
    assert_eq!(vim.mode, editor_vim::Mode::Insert);
}

#[test]
fn escape_returns_to_normal() {
    let mut vim = VimState::new();
    vim.mode = editor_vim::Mode::Insert;
    let s = state_with_caret("abc", 1);
    let _ = drive(s, &mut vim, &["Escape"]);
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
}

#[test]
fn x_deletes_char_under_caret() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 1);
    let s = drive(s, &mut vim, &["x"]);
    assert_eq!(s.doc.to_string(), "ac");
    assert_eq!(s.selection.primary().head, 1);
}

#[test]
fn dollar_moves_to_line_end() {
    // Normal-mode caret sits ON the last char, never on the `\n`.
    let mut vim = VimState::new();
    let s = state_with_caret("hello\nworld", 0);
    let s = drive(s, &mut vim, &["$"]);
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn zero_moves_to_line_start() {
    let mut vim = VimState::new();
    let s = state_with_caret("hello", 3);
    let s = drive(s, &mut vim, &["0"]);
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn capital_d_deletes_to_eol() {
    let mut vim = VimState::new();
    let s = state_with_caret("hello world\nnext", 6);
    let s = drive(s, &mut vim, &["D"]);
    assert_eq!(s.doc.to_string(), "hello \nnext");
    // Caret clamps onto the new last char (the trailing space).
    assert_eq!(s.selection.primary().head, 5);
}

#[test]
fn capital_c_changes_to_eol_and_enters_insert() {
    let mut vim = VimState::new();
    let s = state_with_caret("hello world", 6);
    let s = drive(s, &mut vim, &["C"]);
    assert_eq!(s.doc.to_string(), "hello ");
    assert_eq!(vim.mode, editor_vim::Mode::Insert);
}

#[test]
fn capital_y_yanks_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("first\nsecond", 0);
    let s = drive(s, &mut vim, &["Y", "j", "p"]);
    // After Y (line yank) → j → p (paste after) on line "second",
    // doc becomes "first\nsecond\nfirst".
    assert_eq!(s.doc.to_string(), "first\nsecond\nfirst");
}

#[test]
fn gg_jumps_to_first_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("first\nsecond\nthird", 14);
    let s = drive(s, &mut vim, &["g", "g"]);
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn gu_motion_lowercases_word() {
    let mut vim = VimState::new();
    let s = state_with_caret("HELLO world", 0);
    let s = drive(s, &mut vim, &["g", "u", "w"]);
    assert_eq!(s.doc.to_string(), "hello world");
}

#[test]
fn gcap_u_motion_uppercases_word() {
    let mut vim = VimState::new();
    let s = state_with_caret("hello world", 0);
    let s = drive(s, &mut vim, &["g", "U", "w"]);
    assert_eq!(s.doc.to_string(), "HELLO world");
}

#[test]
fn g_tilde_doubled_toggles_case_of_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("Hello World", 0);
    let s = drive(s, &mut vim, &["g", "~", "~"]);
    assert_eq!(s.doc.to_string(), "hELLO wORLD");
}

#[test]
fn star_jumps_to_next_occurrence_of_word_under_caret() {
    let mut vim = VimState::new();
    // Caret at 0 ("f" of first "foo"). `*` should jump to the
    // start of the SECOND "foo".
    let s = state_with_caret("foo bar foo baz", 0);
    let s = drive(s, &mut vim, &["*"]);
    assert_eq!(s.selection.primary().head, 8);
}

#[test]
fn hash_jumps_to_previous_occurrence() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar foo baz", 8);
    let s = drive(s, &mut vim, &["#"]);
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn n_repeats_last_search_forward() {
    let mut vim = VimState::new();
    // `foo` appears 3 times. `*` jumps to occurrence 2; `n`
    // jumps to occurrence 3.
    let s = state_with_caret("foo bar foo baz foo end", 0);
    let s = drive(s, &mut vim, &["*", "n"]);
    assert_eq!(s.selection.primary().head, 16);
}

#[test]
fn capital_n_reverses_search_direction() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar foo baz foo end", 0);
    // `*` → occurrence 2 (pos 8). `N` reverses → back to 0.
    let s = drive(s, &mut vim, &["*", "N"]);
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn star_requires_whole_word_match() {
    let mut vim = VimState::new();
    // Caret on `foo` — `*` must skip `foobar` and land on the
    // standalone `foo`.
    let s = state_with_caret("foo foobar baz foo end", 0);
    let s = drive(s, &mut vim, &["*"]);
    assert_eq!(s.selection.primary().head, 15);
}

// ═══ Regression suite for the production-readiness overhaul ═══

#[test]
fn shift_o_opens_line_above_on_first_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 1);
    let s = drive(s, &mut vim, &["O"]);
    assert_eq!(s.doc.to_string(), "\nabc");
    assert_eq!(s.selection.primary().head, 0);
    assert_eq!(vim.mode, editor_vim::Mode::Insert);
}

#[test]
fn shift_o_opens_line_above_mid_doc() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc\ndef", 5);
    let s = drive(s, &mut vim, &["O"]);
    assert_eq!(s.doc.to_string(), "abc\n\ndef");
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn o_opens_line_below() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc\ndef", 1);
    let s = drive(s, &mut vim, &["o"]);
    assert_eq!(s.doc.to_string(), "abc\n\ndef");
    assert_eq!(s.selection.primary().head, 4);
}

// ── dt/dT vs text objects (the sentinel-collision bug) ──

#[test]
fn dt_deletes_till_char_not_text_object() {
    // `dtw` used to resolve as `daw` because the pending-input
    // sentinel for `t` collided with the text-object marker.
    let mut vim = VimState::new();
    let s = state_with_caret("a bwc", 0);
    let s = drive(s, &mut vim, &["d", "t", "w"]);
    assert_eq!(s.doc.to_string(), "wc");
}

#[test]
fn df_deletes_through_char() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar", 0);
    let s = drive(s, &mut vim, &["d", "f", "b"]);
    assert_eq!(s.doc.to_string(), "ar");
}

#[test]
fn diw_deletes_inner_word() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar baz", 5);
    let s = drive(s, &mut vim, &["d", "i", "w"]);
    assert_eq!(s.doc.to_string(), "foo  baz");
}

#[test]
fn di_quote_deletes_inside_quotes() {
    let mut vim = VimState::new();
    let s = state_with_caret(r#"say "hello" now"#, 6);
    let s = drive(s, &mut vim, &["d", "i", "\""]);
    assert_eq!(s.doc.to_string(), r#"say "" now"#);
}

// ── inclusive motions with operators ──

#[test]
fn de_includes_last_char_of_word() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar", 0);
    let s = drive(s, &mut vim, &["d", "e"]);
    assert_eq!(s.doc.to_string(), " bar");
}

#[test]
fn cw_acts_like_ce_on_word() {
    // vim's `cw` special case: does NOT eat trailing whitespace.
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar", 0);
    let s = drive(s, &mut vim, &["c", "w"]);
    assert_eq!(s.doc.to_string(), " bar");
    assert_eq!(vim.mode, editor_vim::Mode::Insert);
}

#[test]
fn dw_still_eats_trailing_space() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar", 0);
    let s = drive(s, &mut vim, &["d", "w"]);
    assert_eq!(s.doc.to_string(), "bar");
}

// ── linewise operator motions ──

#[test]
fn dj_deletes_two_lines() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 1);
    let s = drive(s, &mut vim, &["d", "j"]);
    assert_eq!(s.doc.to_string(), "three");
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn dk_deletes_current_and_previous_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 5);
    let s = drive(s, &mut vim, &["d", "k"]);
    assert_eq!(s.doc.to_string(), "three");
}

#[test]
fn d_gg_deletes_to_first_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 5);
    let s = drive(s, &mut vim, &["d", "g", "g"]);
    assert_eq!(s.doc.to_string(), "three");
}

#[test]
fn d_cap_g_deletes_to_last_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 5);
    let s = drive(s, &mut vim, &["d", "G"]);
    assert_eq!(s.doc.to_string(), "one");
}

// ── cc / S keep the newline ──

#[test]
fn cc_keeps_trailing_newline() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 5);
    let s = drive(s, &mut vim, &["c", "c"]);
    assert_eq!(s.doc.to_string(), "one\n\nthree");
    assert_eq!(vim.mode, editor_vim::Mode::Insert);
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn cc_preserves_indent() {
    let mut vim = VimState::new();
    let s = state_with_caret("    body text\nnext", 6);
    let s = drive(s, &mut vim, &["c", "c"]);
    assert_eq!(s.doc.to_string(), "    \nnext");
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn dd_lands_on_first_nonblank_of_next_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\n  two\nthree", 1);
    let s = drive(s, &mut vim, &["d", "d"]);
    assert_eq!(s.doc.to_string(), "  two\nthree");
    assert_eq!(s.selection.primary().head, 2);
}

#[test]
fn dd_on_last_line_removes_dangling_newline() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo", 5);
    let s = drive(s, &mut vim, &["d", "d"]);
    assert_eq!(s.doc.to_string(), "one");
}

// ── counts on single-char commands ──

#[test]
fn count_x_deletes_n_chars_but_not_newline() {
    let mut vim = VimState::new();
    let s = state_with_caret("abcd\nef", 1);
    let s = drive(s, &mut vim, &["3", "x"]);
    assert_eq!(s.doc.to_string(), "a\nef");
    // Caret clamps back onto 'a' (end of line).
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn x_at_end_of_line_does_not_join() {
    let mut vim = VimState::new();
    // Caret on the newline (position 3): x must be a no-op, not
    // eat the '\n'.
    let s = state_with_caret("abc\ndef", 3);
    let s = drive(s, &mut vim, &["x"]);
    assert_eq!(s.doc.to_string(), "abc\ndef");
}

#[test]
fn count_tilde_toggles_n_chars() {
    let mut vim = VimState::new();
    let s = state_with_caret("abcd", 0);
    let s = drive(s, &mut vim, &["3", "~"]);
    assert_eq!(s.doc.to_string(), "ABCd");
    assert_eq!(s.selection.primary().head, 3);
}

#[test]
fn count_r_replaces_n_chars() {
    let mut vim = VimState::new();
    let s = state_with_caret("abcd", 0);
    let s = drive(s, &mut vim, &["3", "r", "z"]);
    assert_eq!(s.doc.to_string(), "zzzd");
    // Caret on the last replaced char.
    assert_eq!(s.selection.primary().head, 2);
}

#[test]
fn count_r_past_line_end_is_noop() {
    let mut vim = VimState::new();
    let s = state_with_caret("ab\ncd", 0);
    let s = drive(s, &mut vim, &["5", "r", "z"]);
    assert_eq!(s.doc.to_string(), "ab\ncd");
}

#[test]
fn count_j_joins_n_lines() {
    let mut vim = VimState::new();
    let s = state_with_caret("a\nb\nc\nd", 0);
    let s = drive(s, &mut vim, &["3", "J"]);
    assert_eq!(s.doc.to_string(), "a b c\nd");
}

#[test]
fn join_strips_leading_indent_of_next_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\n    two", 0);
    let s = drive(s, &mut vim, &["J"]);
    assert_eq!(s.doc.to_string(), "one two");
    assert_eq!(s.selection.primary().head, 3);
}

#[test]
fn count_p_pastes_n_times() {
    let mut vim = VimState::new();
    let s = state_with_caret("ab", 0);
    let s = drive(s, &mut vim, &["y", "l", "2", "p"]);
    // yl yanks "a"; 2p pastes "aa" after caret.
    assert_eq!(s.doc.to_string(), "aaab");
}

// ── G / gg with counts ──

#[test]
fn cap_g_goes_to_last_line_first_nonblank() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\n  three", 0);
    let s = drive(s, &mut vim, &["G"]);
    assert_eq!(s.selection.primary().head, 10);
}

#[test]
fn count_g_goes_to_line_n() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 0);
    let s = drive(s, &mut vim, &["2", "G"]);
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn count_gg_goes_to_line_n() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 8);
    let s = drive(s, &mut vim, &["2", "g", "g"]);
    assert_eq!(s.selection.primary().head, 4);
}

// ── word motions across lines ──

#[test]
fn w_crosses_line_boundary_in_one_press() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo\nbar", 0);
    let s = drive(s, &mut vim, &["w"]);
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn w_stops_on_empty_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo\n\nbar", 0);
    let s = drive(s, &mut vim, &["w"]);
    // Empty line is a word target (caret on its position).
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn b_crosses_line_boundary() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo\nbar", 4);
    let s = drive(s, &mut vim, &["b"]);
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn e_crosses_line_boundary() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo\nbar", 2);
    let s = drive(s, &mut vim, &["e"]);
    assert_eq!(s.selection.primary().head, 6);
}

#[test]
fn e_at_end_of_doc_stays_on_last_char() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar", 4);
    let s = drive(s, &mut vim, &["e", "e", "e"]);
    assert_eq!(s.selection.primary().head, 6);
}

#[test]
fn ge_goes_to_end_of_previous_word() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar", 4);
    let s = drive(s, &mut vim, &["g", "e"]);
    assert_eq!(s.selection.primary().head, 2);
}

// ── f/t line-locality + ; , repeat ──

#[test]
fn f_does_not_cross_lines() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc\nxyz", 0);
    let s = drive(s, &mut vim, &["f", "x"]);
    // 'x' only exists on the next line — caret must not move.
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn semicolon_repeats_find() {
    let mut vim = VimState::new();
    let s = state_with_caret("a.b.c", 0);
    let s = drive(s, &mut vim, &["f", "."]);
    assert_eq!(s.selection.primary().head, 1);
    let s = drive(s, &mut vim, &[";"]);
    assert_eq!(s.selection.primary().head, 3);
}

#[test]
fn comma_repeats_find_reversed() {
    let mut vim = VimState::new();
    let s = state_with_caret("a.b.c", 0);
    let s = drive(s, &mut vim, &["f", ".", ";", ","]);
    assert_eq!(s.selection.primary().head, 1);
}

#[test]
fn find_works_with_multibyte_char() {
    let mut vim = VimState::new();
    let s = state_with_caret("abcédef", 0);
    let s = drive(s, &mut vim, &["f", "é"]);
    assert_eq!(s.selection.primary().head, 3);
}

// ── insert-mode exit ──

#[test]
fn escape_from_insert_steps_caret_left() {
    let mut vim = VimState::new();
    vim.mode = editor_vim::Mode::Insert;
    let s = state_with_caret("abc", 2);
    let s = drive(s, &mut vim, &["Escape"]);
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
    assert_eq!(s.selection.primary().head, 1);
}

#[test]
fn escape_at_line_start_stays_put() {
    let mut vim = VimState::new();
    vim.mode = editor_vim::Mode::Insert;
    let s = state_with_caret("abc\ndef", 4);
    let s = drive(s, &mut vim, &["Escape"]);
    assert_eq!(s.selection.primary().head, 4);
}

// ── a / A ──

#[test]
fn a_at_line_end_inserts_before_newline() {
    let mut vim = VimState::new();
    let s = state_with_caret("ab\ncd", 1);
    let s = drive(s, &mut vim, &["a"]);
    // caret was on 'b' (last char) — `a` goes to 2 (before \n),
    // never onto the next line.
    assert_eq!(s.selection.primary().head, 2);
    assert_eq!(vim.mode, editor_vim::Mode::Insert);
}

// ── visual mode ──

#[test]
fn v_l_d_deletes_two_chars_inclusive() {
    let mut vim = VimState::new();
    let s = state_with_caret("abcdef", 1);
    let s = drive(s, &mut vim, &["v", "l", "d"]);
    // Selection covers 'b' and 'c' (inclusive of head char).
    assert_eq!(s.doc.to_string(), "adef");
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
}

#[test]
fn v_d_deletes_single_char() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 1);
    let s = drive(s, &mut vim, &["v", "d"]);
    assert_eq!(s.doc.to_string(), "ac");
}

#[test]
fn v_x_deletes_selection() {
    let mut vim = VimState::new();
    let s = state_with_caret("abcdef", 0);
    let s = drive(s, &mut vim, &["v", "2", "l", "x"]);
    assert_eq!(s.doc.to_string(), "def");
}

#[test]
fn visual_line_d_deletes_whole_lines() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 5);
    let s = drive(s, &mut vim, &["V", "d"]);
    assert_eq!(s.doc.to_string(), "one\nthree");
}

#[test]
fn visual_line_j_d_deletes_two_lines() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 0);
    let s = drive(s, &mut vim, &["V", "j", "d"]);
    assert_eq!(s.doc.to_string(), "three");
}

#[test]
fn visual_backward_selection_includes_anchor_char() {
    let mut vim = VimState::new();
    let s = state_with_caret("abcdef", 3);
    let s = drive(s, &mut vim, &["v", "h", "h", "d"]);
    // Anchor 'd', head walked back to 'b' — deletes "bcd".
    assert_eq!(s.doc.to_string(), "aef");
}

#[test]
fn viw_selects_inner_word_then_d() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar baz", 5);
    let s = drive(s, &mut vim, &["v", "i", "w", "d"]);
    assert_eq!(s.doc.to_string(), "foo  baz");
}

#[test]
fn visual_o_swaps_ends() {
    let mut vim = VimState::new();
    let s = state_with_caret("abcdef", 2);
    let s = drive(s, &mut vim, &["v", "l", "o", "h", "d"]);
    // v at 2, l → head 3; o → head back at 2; h → head 1.
    // Deletes 'b'..'d' inclusive.
    assert_eq!(s.doc.to_string(), "aef");
}

#[test]
fn visual_u_lowercases_selection() {
    let mut vim = VimState::new();
    let s = state_with_caret("ABCDEF", 0);
    let s = drive(s, &mut vim, &["v", "2", "l", "u"]);
    assert_eq!(s.doc.to_string(), "abcDEF");
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
}

#[test]
fn visual_tilde_toggles_selection() {
    let mut vim = VimState::new();
    let s = state_with_caret("aBcD", 0);
    let s = drive(s, &mut vim, &["v", "3", "l", "~"]);
    assert_eq!(s.doc.to_string(), "AbCd");
}

#[test]
fn visual_p_replaces_selection_with_register() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo bar", 0);
    // Yank "foo" then select "bar" and paste over it.
    let s = drive(s, &mut vim, &["v", "2", "l", "y", "w", "v", "2", "l", "p"]);
    assert_eq!(s.doc.to_string(), "foo foo");
}

#[test]
fn visual_escape_returns_to_normal() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 0);
    let s = drive(s, &mut vim, &["v", "l", "Escape"]);
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
    let r = s.selection.primary();
    assert_eq!(r.anchor, r.head);
}

#[test]
fn visual_v_toggle_exits() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 0);
    let _ = drive(s, &mut vim, &["v", "v"]);
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
}

#[test]
fn visual_y_then_p_pastes() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 0);
    let s = drive(s, &mut vim, &["v", "l", "y", "$", "p"]);
    // yank "ab", $ → caret on 'c' (2), p pastes after → "abcab"
    assert_eq!(s.doc.to_string(), "abcab");
}

// ── operator-pending abort ──

#[test]
fn invalid_operator_key_aborts_pending() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc def", 0);
    // `dz` is invalid — pending op must clear so the next `w` is
    // a plain motion, not `dw`.
    let s = drive(s, &mut vim, &["d", "z", "w"]);
    assert_eq!(s.doc.to_string(), "abc def");
    assert_eq!(s.selection.primary().head, 4);
}

// ── registers ──

#[test]
fn named_register_yank_and_paste() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 0);
    let s = drive(s, &mut vim, &["\"", "a", "y", "l", "$", "\"", "a", "p"]);
    assert_eq!(s.doc.to_string(), "abca");
}

#[test]
fn linewise_paste_before_with_cap_p() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo", 4);
    let s = drive(s, &mut vim, &["y", "y", "k", "P"]);
    assert_eq!(s.doc.to_string(), "two\none\ntwo");
    assert_eq!(s.selection.primary().head, 0);
}

// ── dot repeat ──

#[test]
fn dot_repeats_dd() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\nthree", 0);
    let s = drive(s, &mut vim, &["d", "d", "."]);
    assert_eq!(s.doc.to_string(), "three");
}

#[test]
fn dot_repeats_df() {
    let mut vim = VimState::new();
    let s = state_with_caret("a,b,c", 0);
    let s = drive(s, &mut vim, &["d", "f", ",", "."]);
    assert_eq!(s.doc.to_string(), "c");
}

// ── multibyte safety ──

#[test]
fn j_k_are_utf8_column_safe() {
    let mut vim = VimState::new();
    // Line 1 has multibyte chars before the caret column.
    let s = state_with_caret("héllo\nwörld", 0);
    let s = drive(s, &mut vim, &["l", "l", "l"]);
    // h(1) é(2) l → caret at byte 4 (col 3)
    assert_eq!(s.selection.primary().head, 4);
    let s = drive(s, &mut vim, &["j"]);
    // Line 2 starts at byte 7 ("héllo\n" is 7 bytes): w(7),
    // ö(8..10), r(10), l(11) — char-col 3 lands on byte 11.
    assert_eq!(s.selection.primary().head, 11);
}

#[test]
fn x_on_multibyte_char_deletes_whole_char() {
    let mut vim = VimState::new();
    let s = state_with_caret("héllo", 1);
    let s = drive(s, &mut vim, &["x"]);
    assert_eq!(s.doc.to_string(), "hllo");
}

#[test]
fn goal_column_sticks_across_short_lines() {
    let mut vim = VimState::new();
    let s = state_with_caret("longline\nab\nlongline", 5);
    let s = drive(s, &mut vim, &["j"]);
    // Line "ab" is short — caret clamps to its last char.
    assert_eq!(s.selection.primary().head, 10);
    let s = drive(s, &mut vim, &["j"]);
    // Goal column 5 restored on the long line (starts at 12).
    assert_eq!(s.selection.primary().head, 17);
}

// ── % bracket matching ──

#[test]
fn percent_scans_forward_on_line_for_bracket() {
    let mut vim = VimState::new();
    let s = state_with_caret("let x = (a + b);", 0);
    let s = drive(s, &mut vim, &["%"]);
    assert_eq!(s.selection.primary().head, 14);
}

// ═══ command-line mode (`:`, `/`, `?`) ═══

fn type_keys(keys: &str) -> Vec<String> {
    keys.chars().map(|c| c.to_string()).collect()
}

fn drive_str(state: EditorState, vim: &mut VimState, keys: &str) -> EditorState {
    let keys: Vec<String> = type_keys(keys);
    let refs: Vec<&str> = keys.iter().map(String::as_str).collect();
    drive(state, vim, &refs)
}

#[test]
fn colon_enters_command_mode_and_buffers() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 0);
    let _ = drive_str(s, &mut vim, ":wq");
    assert_eq!(vim.mode, editor_vim::Mode::Command);
    assert_eq!(vim.command_line.as_ref().unwrap().buffer, "wq");
}

#[test]
fn colon_escape_cancels() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 0);
    let s = drive_str(s, &mut vim, ":q");
    let _ = drive(s, &mut vim, &["Escape"]);
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
    assert!(vim.command_line.is_none());
}

#[test]
fn colon_backspace_past_start_exits() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 0);
    let s = drive_str(s, &mut vim, ":w");
    let _ = drive(s, &mut vim, &["Backspace", "Backspace"]);
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
}

#[test]
fn colon_w_emits_save_event() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc", 0);
    let s = drive_str(s, &mut vim, ":w");
    let spec = editor_vim::handle_key(
        &s,
        &mut vim,
        &KeySpec { key: "Enter".into(), ..Default::default() },
    )
    .expect(":w must produce a spec");
    assert_eq!(spec.user_event.as_deref(), Some("save"));
    assert_eq!(vim.mode, editor_vim::Mode::Normal);
}

#[test]
fn colon_number_goes_to_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("one\ntwo\n  three", 0);
    let s = drive_str(s, &mut vim, ":3");
    let s = drive(s, &mut vim, &["Enter"]);
    assert_eq!(s.selection.primary().head, 10);
}

#[test]
fn colon_substitute_current_line() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo foo\nfoo", 0);
    let s = drive_str(s, &mut vim, ":s/foo/bar/");
    let s = drive(s, &mut vim, &["Enter"]);
    // Only first occurrence on the current line.
    assert_eq!(s.doc.to_string(), "bar foo\nfoo");
}

#[test]
fn colon_substitute_whole_doc_global() {
    let mut vim = VimState::new();
    let s = state_with_caret("foo foo\nfoo", 0);
    let s = drive_str(s, &mut vim, ":%s/foo/bar/g");
    let s = drive(s, &mut vim, &["Enter"]);
    assert_eq!(s.doc.to_string(), "bar bar\nbar");
}

#[test]
fn colon_substitute_line_range() {
    let mut vim = VimState::new();
    let s = state_with_caret("aX\nbX\ncX\ndX", 0);
    let s = drive_str(s, &mut vim, ":2,3s/X/Y/");
    let s = drive(s, &mut vim, &["Enter"]);
    assert_eq!(s.doc.to_string(), "aX\nbY\ncY\ndX");
}

#[test]
fn slash_search_jumps_and_n_repeats() {
    let mut vim = VimState::new();
    let s = state_with_caret("abc needle def needle x", 0);
    let s = drive_str(s, &mut vim, "/needle");
    let s = drive(s, &mut vim, &["Enter"]);
    assert_eq!(s.selection.primary().head, 4);
    let s = drive(s, &mut vim, &["n"]);
    assert_eq!(s.selection.primary().head, 15);
    // wraps
    let s = drive(s, &mut vim, &["n"]);
    assert_eq!(s.selection.primary().head, 4);
}

#[test]
fn question_search_goes_backward() {
    let mut vim = VimState::new();
    let s = state_with_caret("needle abc needle", 10);
    let s = drive_str(s, &mut vim, "?needle");
    let s = drive(s, &mut vim, &["Enter"]);
    assert_eq!(s.selection.primary().head, 0);
}

#[test]
fn slash_search_is_substring_not_whole_word() {
    let mut vim = VimState::new();
    let s = state_with_caret("xx fooba xx", 0);
    let s = drive_str(s, &mut vim, "/oob");
    let s = drive(s, &mut vim, &["Enter"]);
    assert_eq!(s.selection.primary().head, 4);
}
