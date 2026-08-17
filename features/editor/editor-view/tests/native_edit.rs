//! Text editing on the native (Blitz) path — typing, Enter, Tab,
//! Backspace/Delete, and typing over a selection. The web path hands
//! these to contenteditable; on native `editor-view` IS the default
//! action, so each is asserted against the doc probe.
#![cfg(feature = "native")]

mod common;
use common::*;

#[tokio::test]
async fn typing_inserts_at_caret() {
    let t = mount(Setup::text("ac").caret(1));
    t.type_text("b");
    expect_probe(&t, "doc", "abc").await;
    expect_probe(&t, "head", "2").await;
}

#[tokio::test]
async fn enter_splits_line() {
    let t = mount(Setup::text("ab").caret(1));
    press(&t, &["Enter"]);
    expect_probe(&t, "doc", "a\nb").await;
    expect_probe(&t, "head", "2").await;
}

#[tokio::test]
async fn tab_inserts_tab() {
    let t = mount(Setup::text("").caret(0));
    press(&t, &["Tab"]);
    expect_probe(&t, "doc", "\t").await;
}

#[tokio::test]
async fn typing_replaces_selection() {
    // Select "ell" (1..4) then type "x" → "hxo".
    let t = mount(Setup::text("hello").caret(1));
    t.press_key(parse_key("ArrowRight"), Modifiers::SHIFT);
    t.press_key(parse_key("ArrowRight"), Modifiers::SHIFT);
    t.press_key(parse_key("ArrowRight"), Modifiers::SHIFT);
    expect_probe(&t, "head", "4").await;
    t.type_text("x");
    expect_probe(&t, "doc", "hxo").await;
    expect_probe(&t, "head", "2").await;
}

#[tokio::test]
async fn backspace_deletes_before_caret() {
    let t = mount(Setup::text("abc").caret(2));
    press(&t, &["Backspace"]);
    expect_probe(&t, "doc", "ac").await;
    expect_probe(&t, "head", "1").await;
}

#[tokio::test]
async fn delete_removes_after_caret() {
    let t = mount(Setup::text("abc").caret(1));
    press(&t, &["Delete"]);
    expect_probe(&t, "doc", "ac").await;
    expect_probe(&t, "head", "1").await;
}

#[tokio::test]
async fn backspace_joins_lines() {
    let t = mount(Setup::text("a\nb").caret(2));
    press(&t, &["Backspace"]);
    expect_probe(&t, "doc", "ab").await;
    expect_probe(&t, "head", "1").await;
}

#[tokio::test]
async fn enter_continues_bullet_list() {
    // Shared-command behavior (editor_state::commands::enter_continue_list)
    // — the same code the web bridge routes Enter through.
    let t = mount(Setup::text("- foo").caret(5));
    press(&t, &["Enter"]);
    expect_probe(&t, "doc", "- foo\n- ").await;
    expect_probe(&t, "head", "8").await;
}

#[tokio::test]
async fn enter_on_empty_list_item_exits_list() {
    let t = mount(Setup::text("- foo\n- ").caret(8));
    press(&t, &["Enter"]);
    expect_probe(&t, "doc", "- foo\n").await;
}

#[tokio::test]
async fn bracket_autopairs_and_backspace_removes_pair() {
    let t = mount(Setup::text(""));
    t.type_text("(");
    expect_probe(&t, "doc", "()").await;
    expect_probe(&t, "head", "1").await;
    press(&t, &["Backspace"]);
    expect_probe(&t, "doc", "").await;
}

#[tokio::test]
async fn multibyte_typing_and_backspace() {
    let t = mount(Setup::text(""));
    t.type_text("aé");
    expect_probe(&t, "doc", "aé").await;
    press(&t, &["Backspace"]);
    expect_probe(&t, "doc", "a").await;
    expect_probe(&t, "head", "1").await;
}

#[tokio::test]
async fn ctrl_backspace_deletes_word() {
    let t = mount(Setup::text("foo bar").caret(7));
    t.press_key(parse_key("Backspace"), Modifiers::CONTROL);
    expect_probe(&t, "doc", "foo ").await;
    expect_probe(&t, "head", "4").await;
    t.press_key(parse_key("Backspace"), Modifiers::CONTROL);
    expect_probe(&t, "doc", "").await;
}

#[tokio::test]
async fn ctrl_delete_deletes_word_forward() {
    let t = mount(Setup::text("foo bar").caret(0));
    t.press_key(parse_key("Delete"), Modifiers::CONTROL);
    expect_probe(&t, "doc", " bar").await;
}

#[tokio::test]
async fn tab_indents_list_item_and_shift_tab_outdents() {
    let t = mount(Setup::text("- foo").caret(5));
    press(&t, &["Tab"]);
    expect_probe(&t, "doc", "  - foo").await;
    t.press_key(parse_key("Tab"), Modifiers::SHIFT);
    expect_probe(&t, "doc", "- foo").await;
}

#[tokio::test]
async fn tab_outside_list_inserts_literal_tab() {
    let t = mount(Setup::text("foo").caret(3));
    press(&t, &["Tab"]);
    expect_probe(&t, "doc", "foo\t").await;
}

#[tokio::test]
async fn multi_char_typing_renders() {
    let t = mount(Setup::text(""));
    t.type_text("hello world");
    expect_probe(&t, "doc", "hello world").await;
    // And the rendered tile tree shows it too.
    t.query(".editor-root")
        .expect(inner_html(contains_substring("hello world")))
        .await
        .unwrap();
}
