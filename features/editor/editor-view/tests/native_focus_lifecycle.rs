//! Regression: clicking the editor must FOCUS it and keep focus so
//! subsequent keys route to it. Upstream Blitz cleared focus on any
//! click that didn't land on a form control (our editor root is a
//! plain focussable div), so keydown went to the document root and
//! typing was dead. Fixed in the FTS blitz fork (handle_click treats
//! a focussable element as a match; set_attribute re-flushes
//! is_focussable for tabindex).
#![cfg(feature = "native")]

mod common;
use common::*;

#[tokio::test]
async fn click_then_type_edits_the_doc() {
    // No vim → letters insert literally, so the assertion is direct.
    let t = mount_unfocused(Setup::text("ac"));
    let line = t.query(".cm-line").immediately().unwrap();
    let c = line.center();
    t.click_at(c.page().x as f32, c.page().y as f32);
    // Caret lands at the clicked span's start (offset 0); type there.
    t.type_text("Z");
    expect_probe(&t, "doc", "Zac").await;
}

#[tokio::test]
async fn focus_survives_click() {
    let t = mount_unfocused(Setup::text("hello").vim());
    let focus_before = t.blitz_focus();
    let line = t.query(".cm-line").immediately().unwrap();
    let c = line.center();
    t.click_at(c.page().x as f32, c.page().y as f32);
    // Focus must still be the same (real) node, not fall back to root.
    // (Upstream Blitz blurred it on the click event — see module docs.)
    assert_eq!(
        t.blitz_focus(),
        focus_before,
        "click must not blur the editor"
    );
}
