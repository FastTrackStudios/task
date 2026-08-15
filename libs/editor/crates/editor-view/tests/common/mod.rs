//! Shared harness for the native (Blitz) component tests.
//!
//! `mount` renders `<Editor>` headlessly through `dioxus-test`, focuses
//! it, and returns the tester. The harness component also renders probe
//! divs mirroring the Rust-side state (selection head/anchor, vim mode,
//! raw doc text) so tests can assert on editor *state*, not just HTML.

use dioxus::prelude::*;
use dioxus_test::{DocumentTester, by_testid, render};
use editor_state::{EditorState, Selection};
use editor_view::Editor;
use editor_vim::VimState;

pub use dioxus_test::keyboard_types::{Key, Modifiers};
pub use dioxus_test::matchers::{contains_substring, eq, inner_html};

/// The editor stylesheet, inlined so screenshots paint with real styles.
pub const EDITOR_CSS: &str = include_str!("../../../editor/assets/editor.css");

/// Per-test configuration, delivered to [`Harness`] via root context.
#[derive(Clone)]
pub struct Setup {
    pub text: &'static str,
    pub caret: usize,
    pub vim: bool,
    pub markdown: bool,
    pub theme: Option<&'static str>,
}

impl Setup {
    pub fn text(text: &'static str) -> Self {
        Self {
            text,
            caret: 0,
            vim: false,
            markdown: false,
            theme: None,
        }
    }
    pub fn caret(mut self, caret: usize) -> Self {
        self.caret = caret;
        self
    }
    pub fn vim(mut self) -> Self {
        self.vim = true;
        self
    }
    /// Attach the standard markdown live-preview decoration source.
    pub fn markdown(mut self) -> Self {
        self.markdown = true;
        self
    }
    /// Inject extra CSS (design tokens etc.) so screenshots render with a
    /// real theme instead of the editor CSS's dark fallbacks.
    pub fn theme(mut self, css: &'static str) -> Self {
        self.theme = Some(css);
        self
    }
}

#[component]
pub fn Harness() -> Element {
    let setup = use_context::<Setup>();
    let state = use_signal(|| {
        let mut s = EditorState::new(setup.text);
        s.selection = Selection::caret(setup.caret);
        s
    });
    let vim_sig = use_signal(VimState::new);
    let vim = if setup.vim { Some(vim_sig) } else { None };
    let decorations = setup
        .markdown
        .then(|| editor_view::DecorationSource::ptr(editor_state::markdown::live_preview));

    let s = state.read();
    let primary = s.selection.primary();
    let mode = format!("{:?}", vim_sig.read().mode);
    let doc = s.doc.to_string();
    drop(s);

    let theme = setup.theme;
    rsx! {
        style { dangerous_inner_html: EDITOR_CSS }
        if let Some(css) = theme {
            style { dangerous_inner_html: css }
        }
        Editor { state, vim, decorations }
        div { "data-testid": "head", "{primary.head}" }
        div { "data-testid": "anchor", "{primary.anchor}" }
        div { "data-testid": "mode", "{mode}" }
        div { "data-testid": "doc", "{doc}" }
    }
}

/// Render WITHOUT focusing — for tests that exercise the real focus
/// path (click / autofocus) instead of the synthetic `.focus()`.
pub fn mount_unfocused(setup: Setup) -> DocumentTester {
    render(Harness).with_root_context(setup).build()
}

/// Render + focus the editor. Returns the tester.
pub fn mount(setup: Setup) -> DocumentTester {
    let tester = render(Harness).with_root_context(setup).build();
    tester
        .query(".editor-root")
        .immediately()
        .expect("editor root should render")
        .focus();
    tester
}

/// Assert the probe with `testid` shows exactly `value` (async — lets
/// pending renders flush first).
pub async fn expect_probe(tester: &DocumentTester, testid: &str, value: &str) {
    tester
        .query(by_testid(testid))
        .expect(inner_html(eq(value.to_string())))
        .await
        .unwrap_or_else(|e| panic!("probe {testid} != {value}: {e:?}"));
}

/// Press a sequence of vim-style keys, where each &str is either a
/// single character or a named key ("Escape", "Enter", ...).
pub fn press(tester: &DocumentTester, keys: &[&str]) {
    for k in keys {
        let key = parse_key(k);
        tester.press_key(key, Modifiers::empty());
    }
}

pub fn parse_key(k: &str) -> Key {
    if k.chars().count() == 1 {
        Key::Character(k.to_string())
    } else {
        match k {
            "Escape" => Key::Escape,
            "Enter" => Key::Enter,
            "Backspace" => Key::Backspace,
            "Delete" => Key::Delete,
            "Tab" => Key::Tab,
            "ArrowLeft" => Key::ArrowLeft,
            "ArrowRight" => Key::ArrowRight,
            "ArrowUp" => Key::ArrowUp,
            "ArrowDown" => Key::ArrowDown,
            "Home" => Key::Home,
            "End" => Key::End,
            other => panic!("unmapped key name: {other}"),
        }
    }
}
