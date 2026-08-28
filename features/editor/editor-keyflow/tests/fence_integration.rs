//! The keyflow fence, end to end.
//!
//! `editor-state` renders ```` ```kf ```` fences through a registry rather
//! than a direct dependency, so it can only test the plumbing against a
//! stub. This is the other half: register the *real* renderer and check a
//! real chart comes out of the markdown pass.
//!
//! It lives here because this is the first crate that can see both sides —
//! `editor-state` below it and the chart engine above.

use std::sync::Arc;

use editor_state::doc::Doc;
use editor_state::fence_renderer::register_fence_renderer;
use editor_state::markdown::live_preview;
use editor_state::selection::Selection;
use editor_state::{EditorState, decoration::DecorationKind};

fn state(text: &str) -> EditorState {
    EditorState {
        doc: Doc::from_str(text),
        selection: Selection::caret(text.len()),
        folds: Vec::new(),
        reading_mode: false,
    }
}

fn keyflow_widget(text: &str) -> Option<String> {
    register_fence_renderer("kf", Arc::new(editor_keyflow::Fences));
    live_preview(&state(text))
        .into_iter()
        .find_map(|d| match d.kind {
            DecorationKind::Widget { html } if html.contains("md-keyflow-widget") => Some(html),
            _ => None,
        })
}

#[test]
fn a_kf_fence_engraves_a_real_chart() {
    // The snippet the keyflow guide's chords chapter ships.
    let html = keyflow_widget("```kf\nCmaj7 | F#m7b5 | Bbmaj9 | G7b9\n```\n\ntail")
        .expect("a kf fence should produce a chart widget");
    assert!(
        html.contains("<svg"),
        "the widget should embed engraved SVG"
    );
    assert!(
        html.contains("md-keyflow-toggle"),
        "a rendered chart carries the source toggle"
    );
}

#[test]
fn the_source_is_keyflow_highlighted_not_plain_text() {
    let html = keyflow_widget("```kf+\nCmaj7 | F#m7b5\n```\n\ntail")
        .expect("a kf+ fence should produce a widget");
    assert!(
        html.contains("class=\"kf-root\""),
        "the source block should carry keyflow highlighting"
    );
}

#[test]
fn without_registration_a_chart_falls_back_to_source() {
    // Not a failure mode — it is the documented behaviour for any fence
    // language the host has not plugged a renderer in for. Asserted with
    // the trait directly rather than by unregistering, because the
    // registry is process-wide and this test shares it with the others.
    use editor_state::fence_renderer::fence_renderer;
    assert!(
        fence_renderer("a-language-nobody-registered").is_none(),
        "an unregistered language must resolve to None, not panic"
    );
}
