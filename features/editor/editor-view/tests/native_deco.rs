//! Markdown live-preview decorations on the native (Blitz) path —
//! marker hide/reveal around the caret, heading/bold classes, and the
//! caret-proximity behavior that makes the editor feel like Obsidian.
#![cfg(feature = "native")]

mod common;
use common::*;
use dioxus_test::matchers::not;

#[tokio::test]
async fn bold_markers_hidden_when_caret_away() {
    // Caret at doc start, well outside the bold span at the end.
    let t = mount(Setup::text("x **bold**").caret(0).markdown());
    t.query(".editor-root")
        .expect(inner_html(contains_substring("bold")))
        .await
        .unwrap();
    t.query(".editor-root")
        .expect(inner_html(not(contains_substring("**"))))
        .await
        .unwrap();
}

#[tokio::test]
async fn bold_markers_revealed_when_caret_inside() {
    // Caret inside the bold word — source markers must be visible.
    let t = mount(Setup::text("x **bold**").caret(6).markdown());
    t.query(".editor-root")
        .expect(inner_html(contains_substring("**")))
        .await
        .unwrap();
}

#[tokio::test]
async fn moving_caret_into_bold_reveals_markers() {
    let t = mount(Setup::text("**b** xxxx").caret(9).markdown());
    t.query(".editor-root")
        .expect(inner_html(not(contains_substring("**"))))
        .await
        .unwrap();
    // Walk the caret left into the bold span.
    press(&t, &["Home"]);
    press(&t, &["ArrowRight", "ArrowRight"]);
    t.query(".editor-root")
        .expect(inner_html(contains_substring("**")))
        .await
        .unwrap();
}

#[tokio::test]
async fn heading_marker_hidden_when_caret_on_other_line() {
    let t = mount(Setup::text("# Title\nbody").caret(10).markdown());
    t.query(".editor-root")
        .expect(inner_html(contains_substring("Title")))
        .await
        .unwrap();
    t.query(".editor-root")
        .expect(inner_html(not(contains_substring("md-heading-marker"))))
        .await
        .unwrap();
}

#[tokio::test]
async fn heading_marker_revealed_on_caret_line() {
    let t = mount(Setup::text("# Title\nbody").caret(3).markdown());
    t.query(".editor-root")
        .expect(inner_html(contains_substring("md-heading-marker")))
        .await
        .unwrap();
}

// ── Block ops (block mode) — Logseq-style ids/refs ──────────────────

#[tokio::test]
async fn block_id_line_is_hidden() {
    let t = mount(
        Setup::text("a block\nid:: 5f9c1234-abcd-4e5f-8a9b-0c1d2e3f4a5b\nafter")
            .caret(0)
            .markdown(),
    );
    t.query(".editor-root")
        .expect(inner_html(not(contains_substring("id::"))))
        .await
        .unwrap();
    t.query(".editor-root")
        .expect(inner_html(contains_substring("after")))
        .await
        .unwrap();
}

#[tokio::test]
async fn block_reference_renders_as_chip() {
    let t = mount(
        Setup::text("see ((5f9c1234-abcd-4e5f-8a9b-0c1d2e3f4a5b)) here")
            .caret(0)
            .markdown(),
    );
    t.query(".editor-root")
        .expect(inner_html(contains_substring("md-block-ref-chip")))
        .await
        .unwrap();
}

#[tokio::test]
async fn add_block_id_command_appends_id_line() {
    // The command layer itself (what Mod-Shift-K binds) — asserted
    // through the component by applying it to the shared state.
    use editor_state::EditorState;
    let s = {
        let mut s = EditorState::new("a block");
        s.selection = editor_state::Selection::caret(3);
        s
    };
    // The returned string is the paste-ready `((uuid))` reference.
    let (spec, block_ref) = editor_state::commands::add_block_id(&s).expect("block id");
    let uuid = block_ref
        .strip_prefix("((")
        .and_then(|r| r.strip_suffix("))"))
        .expect("ref shaped ((uuid))");
    let next = s.update(spec);
    let text = next.doc.to_string();
    assert!(
        text.starts_with("a block\nid:: ") && text.contains(uuid),
        "expected id:: line appended, got {text:?}"
    );
}

#[tokio::test]
async fn typing_next_to_bold_keeps_it_decorated() {
    let t = mount(Setup::text("**b** x").caret(7).markdown());
    t.type_text("y");
    expect_probe(&t, "doc", "**b** xy").await;
    // Still decorated (markers hidden) — the decoration source re-runs
    // on every transaction.
    t.query(".editor-root")
        .expect(inner_html(not(contains_substring("**"))))
        .await
        .unwrap();
}

#[tokio::test]
async fn heading_line_gets_size_class() {
    // Regression: line-level decoration classes (md-h1) must reach the
    // native .cm-line so heading font-scaling / blockquote bars / callouts
    // actually render (render_dx was dropping extra_classes).
    let t = mount(Setup::text("# Big heading\nbody").caret(15).markdown());
    t.query(".editor-root")
        .expect(inner_html(contains_substring("md-h1")))
        .await
        .unwrap();
}

#[tokio::test]
async fn blockquote_line_gets_class() {
    let t = mount(Setup::text("> quoted line\nbody").caret(15).markdown());
    t.query(".editor-root")
        .expect(inner_html(contains_substring("md-blockquote")))
        .await
        .unwrap();
}
