//! Turnkey [`EditorApp`] Dioxus component.
//!
//! Apps that just want "a working markdown editor" mount
//! `EditorApp{}` and get: the [`Editor`] widget, markdown
//! live-preview decorations, bracket-pair highlighting, vim
//! modal editing, slash-command palette, and the bundled
//! stylesheet. No props, no setup.
//!
//! For more control (custom decorations, vault-aware
//! cross-doc resolution, custom keymap, alternate seed text),
//! call [`Editor`] directly the way `features/editor/examples/
//! playground/src/main.rs` does — `EditorApp` is just the
//! "no-config" entry point.

use dioxus::prelude::*;

use crate::{
    DecoratedRange, Editor, EditorState, Keymap, bracket_match, commands, editor_view,
    editor_vim::VimState, markdown,
};

/// Bundled stylesheet — lifted from the playground's
/// `assets/playground.css` so consumers don't have to vendor it.
/// `asset!` lets Dioxus serve it through the bundled-assets path
/// on web + desktop alike.
pub const EDITOR_STYLE: Asset = asset!("/assets/editor.css");

/// Zero-config markdown editor component. Wraps [`Editor`] with
/// the standard markdown setup: live-preview decorations,
/// bracket matching, vim modal editing, slash-command palette,
/// and the bundled stylesheet linked in `<head>`.
///
/// Mount with:
/// ```ignore
/// rsx! { editor::EditorApp {} }
/// ```
///
/// Initial document content is a one-line welcome string —
/// callers that want to seed from disk or persist edits should
/// use [`Editor`] directly.
#[component]
pub fn EditorApp() -> Element {
    let state = use_signal(|| EditorState::new(WELCOME.to_string()));
    let keymap = use_signal(standard_markdown_keymap);
    let vim = use_signal(VimState::new);
    // Vim is a physical-keyboard idiom — soft keyboards have no Esc,
    // so Normal mode on a phone is a trap. Decide once at mount:
    // coarse pointer → plain editing (`vim: None`).
    let vim = (!use_hook(editor_view::coarse_pointer)).then_some(vim);
    let slash = use_signal(|| None::<editor_view::slash::SlashState>);

    rsx! {
        document::Link { rel: "stylesheet", href: EDITOR_STYLE }
        div { class: "editor-app",
            div { class: "editor-frame",
                Editor {
                    state,
                    keymap: keymap.read().clone(),
                    decorations: editor_view::DecorationSource::ptr(combined_decorations),
                    vim,
                    slash: Some(slash),
                }
                editor_view::slash::SlashMenu { state, slash }
            }
        }
    }
}

/// Live-preview pass plus bracket-pair highlighting. The two
/// builders dedupe nothing, but their mark ranges don't
/// conflict in practice.
pub fn combined_decorations(state: &EditorState) -> Vec<DecoratedRange> {
    let mut out = markdown::live_preview(state);
    out.extend(bracket_match::bracket_match(state));
    out
}

/// Bindings the playground exercises — kept narrow on purpose,
/// since the view's `beforeinput` bridge already handles the
/// common typing path (Enter / Backspace / Delete go through
/// commands so list-continuation + atomic-line tracking work).
pub fn standard_markdown_keymap() -> Keymap {
    Keymap::new()
        .with("Mod-a", commands::select_all as _)
        .with("Mod-b", commands::toggle_bold as _)
        .with("Mod-i", commands::toggle_italic as _)
        .with("Mod-k", commands::toggle_link as _)
        .with("Mod-Shift-k", |s: &_| {
            commands::add_block_id(s).map(|(t, _)| t)
        })
        .with("Mod-l", commands::cycle_list as _)
        .with("Mod-t", commands::toggle_task as _)
        .with("Mod-1", |s: &_| commands::set_heading(s, 1))
        .with("Mod-2", |s: &_| commands::set_heading(s, 2))
        .with("Mod-3", |s: &_| commands::set_heading(s, 3))
        .with("Mod-4", |s: &_| commands::set_heading(s, 4))
        .with("Mod-5", |s: &_| commands::set_heading(s, 5))
        .with("Mod-6", |s: &_| commands::set_heading(s, 6))
        .with("Mod-0", |s: &_| commands::set_heading(s, 0))
        .with("Mod-e", commands::toggle_reading_mode as _)
        // List lines get the smart Tab (indent the item + renumber the
        // ordered sequences it leaves/joins); everything else keeps the
        // plain block indent.
        .with("Tab", |s: &_| {
            commands::tab_list_indent(s, false).or_else(|| commands::indent_more(s))
        })
        .with("Shift-Tab", |s: &_| {
            commands::tab_list_indent(s, true).or_else(|| commands::indent_less(s))
        })
        .with("Backspace", commands::delete_backward as _)
        .with("Delete", commands::delete_forward as _)
}

const WELCOME: &str = "# Welcome to Task\n\nStart typing. Markdown live-preview, vim, and `/` slash commands are wired in.\n";

// (asset cache-bust: song-strip line CSS, 2026-07-22)
