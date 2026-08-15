use editor_typst::Compiler;

#[test]
fn compiles_hello_world_to_svg() {
    let mut c = Compiler::new();
    c.set_source("Hello, *world*!");
    let svg = c.compile_svg().expect("compile failed");
    assert!(
        svg.starts_with("<svg"),
        "expected SVG output, got: {}",
        &svg[..svg.len().min(80)]
    );
    assert!(svg.contains("world") || svg.contains("svg"));
}

#[test]
fn compiles_math_block() {
    let mut c = Compiler::new();
    c.set_source("$ E = m c^2 $");
    let svg = c.compile_svg().expect("compile failed");
    assert!(svg.starts_with("<svg"));
}

#[test]
fn compiles_to_pdf_bytes() {
    let mut c = Compiler::new();
    c.set_source("= Section\n\nBody text.");
    let pdf = c.compile_pdf().expect("compile failed");
    // PDF magic header.
    assert_eq!(&pdf[..4], b"%PDF");
}

#[test]
fn syntax_error_returns_diagnostic() {
    let mut c = Compiler::new();
    // Unclosed function call — Typst parser should flag this.
    c.set_source("#text(red, hello");
    let err = c.compile_svg().expect_err("expected error");
    let msg = format!("{err}");
    assert!(!msg.is_empty());
}
