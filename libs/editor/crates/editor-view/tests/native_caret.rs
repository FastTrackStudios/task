//! Painted-caret rendering on the native path. Blitz's autofocus sets
//! focus WITHOUT dispatching focusin, and clicks only focus text
//! inputs — so the editor must not depend on focus events to paint its
//! caret (regression: window rendered and typed fine but showed no
//! cursor at all).
#![cfg(feature = "native")]

mod common;
use common::*;

#[tokio::test]
async fn block_caret_paints_at_mount_in_normal_mode() {
    // No synthetic focus, no click — mount alone must show the caret
    // (native autofocus owns focus but fires no focusin).
    let t = mount_unfocused(Setup::text("hello world").vim());
    t.query(".editor-root")
        .expect(inner_html(contains_substring("ed-modal-caret-block")))
        .await
        .unwrap();
}

#[tokio::test]
async fn caret_still_painted_after_click() {
    let t = mount_unfocused(Setup::text("hello\nworld").vim());
    let lines = t.query_all(".cm-line").immediately();
    let c = lines[1].center();
    t.click_at(c.page().x as f32, c.page().y as f32);
    t.query(".editor-root")
        .expect(inner_html(contains_substring("ed-modal-caret-block")))
        .await
        .unwrap();
}

#[tokio::test]
async fn insert_mode_paints_bar_caret() {
    let t = mount(Setup::text("hello").vim());
    press(&t, &["i"]);
    t.query(".editor-root")
        .expect(inner_html(contains_substring("ed-native-caret")))
        .await
        .unwrap();
}

#[tokio::test]
async fn no_vim_still_paints_insert_bar_caret() {
    // vim off → the native bar caret is the only caret; must paint
    // from mount.
    let t = mount_unfocused(Setup::text("hello"));
    t.query(".editor-root")
        .expect(inner_html(contains_substring("ed-native-caret")))
        .await
        .unwrap();
}
