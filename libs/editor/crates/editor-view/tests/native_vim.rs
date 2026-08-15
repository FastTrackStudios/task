//! Vim behavior through the real rendered component — mode transitions,
//! motions, operators, visual mode, undo — driven by keydown events
//! through Blitz focus routing, asserted via the mode/head/doc probes
//! AND the mode class on the rendered root.
#![cfg(feature = "native")]

mod common;
use common::*;
use dioxus_test::matchers::{attribute, some};

async fn expect_root_class(t: &dioxus_test::DocumentTester, fragment: &str) {
    t.query(".editor-root")
        .expect(attribute(
            "class",
            some(contains_substring(fragment.to_string())),
        ))
        .await
        .unwrap_or_else(|e| panic!("root class should contain {fragment}: {e:?}"));
}

#[tokio::test]
async fn starts_in_normal_mode_with_class() {
    let t = mount(Setup::text("abc").vim());
    expect_probe(&t, "mode", "Normal").await;
    expect_root_class(&t, "vim-mode-normal").await;
}

#[tokio::test]
async fn i_enters_insert_escape_returns() {
    let t = mount(Setup::text("abc").vim());
    press(&t, &["i"]);
    expect_probe(&t, "mode", "Insert").await;
    expect_root_class(&t, "vim-mode-insert").await;
    press(&t, &["Escape"]);
    expect_probe(&t, "mode", "Normal").await;
    expect_root_class(&t, "vim-mode-normal").await;
}

#[tokio::test]
async fn insert_mode_typing_flows_to_doc() {
    let t = mount(Setup::text("bc").vim());
    press(&t, &["i"]);
    t.type_text("a");
    expect_probe(&t, "doc", "abc").await;
    press(&t, &["Escape"]);
    expect_probe(&t, "mode", "Normal").await;
}

#[tokio::test]
async fn normal_mode_letters_do_not_insert() {
    let t = mount(Setup::text("abc").vim());
    press(&t, &["j", "k", "x"]); // x deletes, j/k move — none insert
    expect_probe(&t, "doc", "bc").await; // x deleted 'a'
}

#[tokio::test]
async fn hjkl_moves_in_normal_mode() {
    let t = mount(Setup::text("abc\ndef").vim());
    press(&t, &["l", "l"]);
    expect_probe(&t, "head", "2").await;
    press(&t, &["j"]);
    expect_probe(&t, "head", "6").await;
    press(&t, &["h"]);
    expect_probe(&t, "head", "5").await;
    press(&t, &["k"]);
    expect_probe(&t, "head", "1").await;
}

#[tokio::test]
async fn counts_multiply_motions() {
    let t = mount(Setup::text("abcdefgh").vim());
    press(&t, &["3", "l"]);
    expect_probe(&t, "head", "3").await;
}

#[tokio::test]
async fn w_and_b_word_motions() {
    let t = mount(Setup::text("foo bar baz").vim());
    press(&t, &["w"]);
    expect_probe(&t, "head", "4").await;
    press(&t, &["w"]);
    expect_probe(&t, "head", "8").await;
    press(&t, &["b"]);
    expect_probe(&t, "head", "4").await;
}

#[tokio::test]
async fn dw_deletes_word() {
    let t = mount(Setup::text("foo bar").vim());
    press(&t, &["d", "w"]);
    expect_probe(&t, "doc", "bar").await;
    expect_probe(&t, "head", "0").await;
}

#[tokio::test]
async fn dd_deletes_line() {
    let t = mount(Setup::text("one\ntwo\nthree").caret(5).vim());
    press(&t, &["d", "d"]);
    expect_probe(&t, "doc", "one\nthree").await;
}

#[tokio::test]
async fn x_deletes_char_under_cursor() {
    let t = mount(Setup::text("abc").caret(1).vim());
    press(&t, &["x"]);
    expect_probe(&t, "doc", "ac").await;
}

#[tokio::test]
async fn o_opens_line_below_in_insert() {
    let t = mount(Setup::text("ab").vim());
    press(&t, &["o"]);
    expect_probe(&t, "mode", "Insert").await;
    expect_probe(&t, "doc", "ab\n").await;
    t.type_text("cd");
    expect_probe(&t, "doc", "ab\ncd").await;
}

#[tokio::test]
async fn shift_o_opens_line_above() {
    // Regression guard for the shift-normalization path: Blitz reports
    // 'o' + shift, the editor must uppercase it before vim sees it.
    let t = mount(Setup::text("ab").caret(1).vim());
    t.press_key(parse_key("O"), Modifiers::SHIFT);
    expect_probe(&t, "mode", "Insert").await;
    expect_probe(&t, "doc", "\nab").await;
}

#[tokio::test]
async fn visual_mode_extends_and_deletes() {
    let t = mount(Setup::text("abcdef").vim());
    press(&t, &["v", "l", "l"]);
    expect_probe(&t, "mode", "VisualChar").await;
    expect_root_class(&t, "vim-mode-visual").await;
    press(&t, &["d"]);
    expect_probe(&t, "doc", "def").await;
    expect_probe(&t, "mode", "Normal").await;
}

#[tokio::test]
async fn undo_restores_deleted_text() {
    let t = mount(Setup::text("foo bar").vim());
    press(&t, &["d", "w"]);
    expect_probe(&t, "doc", "bar").await;
    press(&t, &["u"]);
    expect_probe(&t, "doc", "foo bar").await;
}

#[tokio::test]
async fn a_appends_after_cursor() {
    let t = mount(Setup::text("ac").vim());
    press(&t, &["a"]);
    expect_probe(&t, "mode", "Insert").await;
    t.type_text("b");
    expect_probe(&t, "doc", "abc").await;
}

#[tokio::test]
async fn yy_p_duplicates_line() {
    let t = mount(Setup::text("one\ntwo").vim());
    press(&t, &["y", "y", "p"]);
    expect_probe(&t, "doc", "one\none\ntwo").await;
}

#[tokio::test]
async fn dw_then_p_pastes_deleted_word() {
    let t = mount(Setup::text("foo bar").vim());
    press(&t, &["d", "w"]);
    expect_probe(&t, "doc", "bar").await;
    press(&t, &["$", "p"]);
    expect_probe(&t, "doc", "barfoo ").await;
}

#[tokio::test]
async fn diw_deletes_inner_word() {
    let t = mount(Setup::text("foo bar baz").caret(5).vim());
    press(&t, &["d", "i", "w"]);
    expect_probe(&t, "doc", "foo  baz").await;
}

#[tokio::test]
async fn ciw_changes_word_and_types() {
    let t = mount(Setup::text("foo bar baz").caret(5).vim());
    press(&t, &["c", "i", "w"]);
    expect_probe(&t, "mode", "Insert").await;
    t.type_text("qux");
    expect_probe(&t, "doc", "foo qux baz").await;
}

#[tokio::test]
async fn count_applies_to_operator_motion() {
    let t = mount(Setup::text("a b c d").vim());
    press(&t, &["d", "2", "w"]);
    expect_probe(&t, "doc", "c d").await;
}

#[tokio::test]
async fn visual_line_deletes_whole_lines() {
    let t = mount(Setup::text("one\ntwo\nthree").caret(5).vim());
    press(&t, &["V", "d"]);
    expect_probe(&t, "doc", "one\nthree").await;
}

#[tokio::test]
async fn gg_and_g_jump_doc_bounds() {
    let t = mount(Setup::text("one\ntwo\nthree").caret(5).vim());
    press(&t, &["g", "g"]);
    expect_probe(&t, "head", "0").await;
    press(&t, &["G"]);
    expect_probe(&t, "head", "8").await; // first non-blank of "three"
}

#[tokio::test]
async fn r_replaces_char_under_cursor() {
    let t = mount(Setup::text("abc").caret(1).vim());
    press(&t, &["r", "x"]);
    expect_probe(&t, "doc", "axc").await;
}

#[tokio::test]
async fn capital_d_deletes_to_eol() {
    let t = mount(Setup::text("hello\nworld").caret(2).vim());
    press(&t, &["D"]);
    expect_probe(&t, "doc", "he\nworld").await;
}

// ── Change-family text objects (ciw / ci" / ci( / ca( / diw / dap) ──

#[tokio::test]
async fn ci_quote_changes_inside_quotes() {
    let t = mount(Setup::text(r#"say "hello" now"#).caret(7).vim());
    press(&t, &["c", "i", "\""]);
    expect_probe(&t, "mode", "Insert").await;
    t.type_text("bye");
    expect_probe(&t, "doc", r#"say "bye" now"#).await;
}

#[tokio::test]
async fn ci_paren_changes_inside_parens() {
    let t = mount(Setup::text("f(a, b) end").caret(3).vim());
    press(&t, &["c", "i", "("]);
    t.type_text("x");
    expect_probe(&t, "doc", "f(x) end").await;
}

#[tokio::test]
async fn ca_paren_changes_around_parens() {
    let t = mount(Setup::text("f(a, b) end").caret(3).vim());
    press(&t, &["c", "a", "("]);
    t.type_text("!");
    expect_probe(&t, "doc", "f! end").await;
}

#[tokio::test]
async fn ci_brace_changes_inside_braces() {
    let t = mount(Setup::text("x {old} y").caret(4).vim());
    press(&t, &["c", "i", "{"]);
    t.type_text("new");
    expect_probe(&t, "doc", "x {new} y").await;
}

#[tokio::test]
async fn caw_deletes_word_and_space_then_types() {
    // Engine semantic (see vim.rs daw tests): `aw` spans the word plus
    // surrounding whitespace on BOTH sides.
    let t = mount(Setup::text("foo bar baz").caret(5).vim());
    press(&t, &["c", "a", "w"]);
    t.type_text("X");
    expect_probe(&t, "doc", "fooXbaz").await;
}

#[tokio::test]
async fn yiw_then_p_duplicates_word() {
    let t = mount(Setup::text("abc def").caret(1).vim());
    press(&t, &["y", "i", "w", "e", "p"]);
    expect_probe(&t, "doc", "abcabc def").await;
}

#[tokio::test]
async fn k_spam_at_top_never_enters_insert() {
    // Regression guard (web sibling lives in browser_only.spec.js):
    // hammering `k` past the top of a doc with YAML frontmatter must
    // never flip to Insert or type anything.
    let t = mount(
        Setup::text("---\ntitle: t\ntags: []\n---\nbody line")
            .caret(28) // on "body line"
            .vim()
            .markdown(),
    );
    for _ in 0..10 {
        press(&t, &["k"]);
    }
    expect_probe(&t, "mode", "Normal").await;
    press(&t, &["k", "k", "k"]);
    expect_probe(&t, "mode", "Normal").await;
    expect_probe(&t, "doc", "---\ntitle: t\ntags: []\n---\nbody line").await;
}

#[tokio::test]
async fn dollar_and_zero_line_motions() {
    let t = mount(Setup::text("hello\nworld").caret(2).vim());
    press(&t, &["$"]);
    expect_probe(&t, "head", "4").await; // on 'o' (normal-mode caret sits ON last char)
    press(&t, &["0"]);
    expect_probe(&t, "head", "0").await;
}

#[tokio::test]
async fn visual_mode_paints_selection_highlight() {
    // Regression: visual mode must PAINT the selected range, not just move
    // the caret. The .ed-selection mark wraps the selected chars.
    let t = mount(Setup::text("select this").vim());
    press(&t, &["v", "l", "l", "l"]);
    t.query(".editor-root")
        .expect(inner_html(contains_substring("ed-selection")))
        .await
        .unwrap();
}

#[tokio::test]
async fn no_selection_highlight_without_visual() {
    // A plain caret (Normal mode) must not paint a selection.
    let t = mount(Setup::text("select this").vim());
    let html = t.query(".editor-root").immediately().unwrap().outer_html();
    assert!(
        !html.contains("ed-selection"),
        "no selection in Normal mode"
    );
}

#[tokio::test]
async fn visual_line_mode_highlights_whole_rows() {
    // V (line-wise) marks whole lines via .ed-selection-line (block bg),
    // not the char-range .ed-selection mark.
    let t = mount(Setup::text("one\ntwo\nthree").caret(0).vim());
    t.press_key(parse_key("V"), Modifiers::SHIFT);
    press(&t, &["j"]);
    expect_probe(&t, "mode", "VisualLine").await;
    t.query(".editor-root")
        .expect(inner_html(contains_substring("ed-selection-line")))
        .await
        .unwrap();
}
