//! Visual-inspection harness: rasterize editor states to PNGs via
//! `DocumentTester::render_png`. Not asserted — a debugging aid for
//! caret/decoration styling that HTML assertions can't catch. Outputs
//! land in the target tmp dir; open them or Read them during dev.
#![cfg(feature = "native")]

mod common;
use common::*;

/// Light theme supplying the exact tokens editor.css reads, so shots
/// render with real colors instead of the dark fallbacks.
const THEME: &str = "
:root { --background:#ffffff; --muted:#f2f4f7; --foreground:#1a1c20;
        --muted-foreground:#6b7280; --primary:#1d4ed8; }
.cm-line { font-size: 22px; line-height: 1.6; }
";

fn out(name: &str) -> String {
    format!("{}/{name}", std::env::temp_dir().display())
}

#[tokio::test]
async fn shot_descender() {
    // Caret on the 'g' (descender) of "going gypsy".
    let t = mount(Setup::text("going gypsy").caret(0).vim().theme(THEME));
    t.render_png(out("editor_descender.png"));
}

#[tokio::test]
async fn shot_visual_line_mode() {
    // V (line-wise) then j — whole lines must highlight.
    let t = mount(Setup::text("first line here\nsecond line\nthird line\nfourth").caret(3).vim().theme(THEME));
    t.press_key(Key::Character("V".into()), Modifiers::SHIFT);
    press(&t, &["j"]);
    t.pump().await.ok();
    t.render_png(out("editor_visual_line.png"));
}

#[tokio::test]
async fn shot_code_and_markers() {
    let doc = "Some text\n\n```rust\nfn main() {\n    println!(\"hi\");\n}\n```\n\n- item one\n- item two\n\n> [!tip] Tip\n> body line one\n> body line two";
    let t = mount(Setup::text(doc).caret(0).markdown().theme(THEME));
    t.render_png(out("editor_code_markers.png"));
}

#[tokio::test]
async fn shot_headings() {
    let t = mount(Setup::text("# H1 heading\n## H2 heading\n### H3 heading\nnormal body text").caret(60).markdown().theme(THEME));
    t.render_png(out("editor_headings.png"));
}

#[tokio::test]
async fn shot_full_styles() {
    // A rich doc exercising the live-preview styles, to eyeball native
    // rendering of headings/bold/lists/callouts/table/code.
    let doc = "# Heading one\n\n**bold**, *italic*, `code`, ==highlight== and a [link](x).\n\n## Heading two\n\n> Blockquote line\n\n- bullet one\n- bullet two\n\n1. ordered\n2. numbered\n\n- [ ] todo\n- [x] done\n\n> [!note] Note\n> callout body\n\n| A | B |\n|---|---|\n| 1 | 2 |\n\n```rust\nfn main() {}\n```";
    let t = mount(Setup::text(doc).caret(200).markdown().theme(THEME));
    t.render_png(out("editor_full_styles.png"));
}

#[tokio::test]
async fn shot_visual_selection() {
    // Enter visual mode and extend a few chars — the range must be
    // highlighted, not just the caret moved.
    let t = mount(Setup::text("select this text
and more").caret(0).vim().theme(THEME));
    press(&t, &["v", "l", "l", "l", "l", "l"]);
    t.pump().await.ok();
    t.render_png(out("editor_selection.png"));
    // multi-line: extend down into the next line
    let m = mount(Setup::text("select this text\nand more here").caret(7).vim().theme(THEME));
    press(&m, &["v", "j", "l", "l"]);
    m.pump().await.ok();
    m.render_png(out("editor_selection_multi.png"));
    // selection over BOLD text (markdown live-preview must be on)
    let b = mount(Setup::text("a **bold** word").caret(0).vim().markdown().theme(THEME));
    press(&b, &["v", "l", "l", "l", "l", "l", "l", "l", "l"]);
    b.pump().await.ok();
    b.render_png(out("editor_selection_bold.png"));
}

#[tokio::test]
async fn shot_mode_line_height() {
    // Normal (block) then Insert (bar) on the SAME 3-line doc; line 2/3
    // y-positions must be identical between the two — the caret must not
    // change line height.
    let n = mount(Setup::text("line one
line two
line three").caret(2).vim().theme(THEME));
    n.render_png(out("editor_mode_normal.png"));
    let i = mount(Setup::text("line one
line two
line three").caret(2).vim().theme(THEME));
    press(&i, &["i"]);
    i.render_png(out("editor_mode_insert.png"));
}

#[tokio::test]
async fn shot_shift_probe() {
    // Two identical lines; caret on the 'w' of the TOP line (offset 6).
    // If inline-block changes advance, "world" on top misaligns with the
    // bottom line's "world".
    let t = mount(Setup::text("hello world
hello world").caret(6).vim().theme(THEME));
    t.render_png(out("editor_shift.png"));
}

#[tokio::test]
async fn shot_empty_line_and_space_caret() {
    // Line 1, an EMPTY line, then more text; caret on the space in
    // "foo bar" (offset in the space between words).
    let t = mount(
        Setup::text("first line

foo bar baz
last")
            .caret(15) // space between "foo" and "bar"
            .vim()
            .theme(THEME),
    );
    t.render_png(out("editor_issues.png"));
}

#[tokio::test]
async fn shot_table() {
    // Caret away from the table so pipe source hides and it renders as a
    // grid widget. Verifies native table layout (borders, cell padding).
    let doc = "intro line\n\n| Feature | Status | Notes |\n|---|---|---|\n| Headings | ok | Mod-1..6 |\n| Tables | ok | GFM pipe |\n| Vim | ok | operators |\n\ntail line";
    let t = mount(Setup::text(doc).caret(0).markdown().theme(THEME));
    t.render_png(out("editor_table.png"));
}

#[tokio::test]
async fn shot_caret_on_empty_line() {
    // Caret sits on the empty line (offset 11 = the empty line start).
    let t = mount(Setup::text("first line

after").caret(11).vim().theme(THEME));
    t.render_png(out("editor_empty_caret.png"));
}
