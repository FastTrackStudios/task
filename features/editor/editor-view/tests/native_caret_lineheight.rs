//! The caret must not change line height — switching Normal (block) and
//! Insert (bar) must leave every line at the same y. cm-line is a block
//! element, so its position is measurable (unlike inline text).
#![cfg(feature = "native")]

mod common;
use common::*;

const THEME: &str = ":root { --background:#ffffff; --foreground:#1a1c20; --primary:#1d4ed8; } .cm-line { font-size: 28px; }";

async fn line_tops(insert: bool) -> Vec<f64> {
    let t = mount(Setup::text("aaaa\nbbbb\ncccc").caret(1).vim().theme(THEME));
    if insert {
        press(&t, &["i"]);
        expect_probe(&t, "mode", "Insert").await;
    }
    t.query_all(".cm-line")
        .immediately()
        .iter()
        .map(|l| l.upper_left().page().y)
        .collect()
}

#[tokio::test]
async fn caret_mode_does_not_move_lines() {
    let normal = line_tops(false).await;
    let insert = line_tops(true).await;
    assert_eq!(normal.len(), 3);
    assert_eq!(insert.len(), 3);
    for (i, (n, ins)) in normal.iter().zip(insert.iter()).enumerate() {
        assert!(
            (n - ins).abs() < 0.5,
            "line {i} moved between Normal ({n:.2}) and Insert ({ins:.2}) — \
             the caret is changing line height (delta {:.2}px)",
            n - ins
        );
    }
}

#[tokio::test]
async fn block_caret_keeps_uniform_line_spacing() {
    // Caret on line 1 (block). Spacing line0→1 must equal line1→2, i.e.
    // the block caret doesn't grow its own line.
    let tops = line_tops(false).await;
    let g01 = tops[1] - tops[0];
    let g12 = tops[2] - tops[1];
    assert!(
        (g01 - g12).abs() < 0.5,
        "block caret grew its line: gaps {g01:.2} vs {g12:.2}"
    );
}
