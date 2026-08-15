//! Keyflow **chart editor** pane for the setlist experience.
//!
//! Mounts the shared `editor::Editor` with keyflow syntax highlighting
//! (`editor_keyflow_lang::keyflow_decorations`) seeded from the CURRENT
//! song's chart text — the same source `SessionChartPane` renders on the
//! left. Charts-as-code: edit the keyflow on the right, read the engraved
//! chart on the left.
//!
//! Edits push the buffer into `SONG_CHARTS[guid]` (debounced) so the engraved
//! chart on the left re-renders live. A **Transposed** toggle (shown when the
//! song has a key/notation/capo view) swaps the editable original for a
//! read-only preview of the source re-spelled to that view
//! (`keyflow::transpose::transpose_source`) — so you can read the chart in
//! Nashville numbers / a new key without the file ever changing.

use dioxus::prelude::*;
use editor_keyflow_lang::{HighlightTheme, highlight_css, keyflow_decorations};
use session_proto::SongChartHydration;
use session_ui::{SONG_CHARTS, SONG_VIEWS};

/// Debounce before pushing an edit into the live chart (re-engrave is
/// heavy; a newer keystroke supersedes a pending push).
// Referenced only under the wasm timer cfg below.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
const RERENDER_DEBOUNCE_MS: u32 = 250;

/// Keyflow chart source editor for one song. `source` is the song's
/// original chart text; `guid` is its `project_guid` (the `SONG_CHARTS`
/// key the engraved chart reads). The caller keys this component so it
/// remounts — and re-seeds — when the song changes or the chart hydrates.
#[component]
pub fn KeyflowChartEditor(source: String, guid: String) -> Element {
    let mut state = use_signal(|| editor::EditorState::new(source.clone()));
    // Re-seed only on an actual `source` change (song switch / hydration) —
    // buffer edits don't change `source`, so they're preserved.
    use_effect(use_reactive!(|source| {
        state.set(editor::EditorState::new(source));
    }));

    // Live re-render: push the buffer into SONG_CHARTS[guid] (debounced) so
    // the engraver on the left follows edits.
    let mut render_gen = use_signal(|| 0u64);
    let guid_live = guid.clone();
    use_effect(move || {
        let text = state.read().doc.to_string();
        let guid = guid_live.clone();
        let my_render_gen = render_gen.peek().wrapping_add(1);
        render_gen.set(my_render_gen);
        spawn(async move {
            #[cfg(target_arch = "wasm32")]
            gloo_timers::future::TimeoutFuture::new(RERENDER_DEBOUNCE_MS).await;
            if *render_gen.peek() != my_render_gen {
                return; // superseded by a newer edit
            }
            let mut charts = SONG_CHARTS.write();
            charts
                .entry(guid)
                .and_modify(|c| c.chart_text = text.clone())
                .or_insert_with(|| SongChartHydration {
                    project_guid: String::new(),
                    chart_text: text,
                    detected_chords: Vec::new(),
                    chart_fingerprint: String::new(),
                });
        });
    });

    let keymap = use_hook(editor::standard_markdown_keymap);
    // Per-token `.kf-*` color rules — injected once for this pane.
    let css = use_hook(|| highlight_css(&HighlightTheme::default_dark()));

    // The active display view (transpose / notation / capo) for THIS song.
    let view = SONG_VIEWS.read().get(&guid).cloned().unwrap_or_default();
    let transposable = !view.is_identity();
    let mut show_transposed = use_signal(|| false);
    // Nothing to transpose to → force the original view.
    let transposed_on = transposable && show_transposed();

    let buffer = state.read().doc.to_string();
    let empty = buffer.trim().is_empty();

    rsx! {
        document::Style { {css} }
        div { class: "flex h-full min-h-0 flex-col",
            div { class: "flex shrink-0 items-center gap-2 border-b border-border px-3 py-1.5",
                span { class: "text-xs font-semibold text-foreground", "Keyflow source" }
                span { class: "text-[11px] text-muted-foreground",
                    if transposed_on { "transposed preview · read-only" } else { "charts as code" }
                }
                // Original / Transposed toggle — only when a view is active.
                if transposable {
                    div { class: "ml-auto flex overflow-hidden rounded border border-border",
                        button {
                            class: if !transposed_on {
                                "px-2 py-0.5 text-[11px] font-semibold bg-accent text-foreground"
                            } else {
                                "px-2 py-0.5 text-[11px] text-muted-foreground hover:text-foreground"
                            },
                            onclick: move |_| show_transposed.set(false),
                            "Original"
                        }
                        button {
                            class: if transposed_on {
                                "px-2 py-0.5 text-[11px] font-semibold bg-accent text-foreground"
                            } else {
                                "px-2 py-0.5 text-[11px] text-muted-foreground hover:text-foreground"
                            },
                            onclick: move |_| show_transposed.set(true),
                            "Transposed"
                        }
                    }
                }
            }
            if empty {
                div { class: "p-4 text-sm text-muted-foreground",
                    "This song has no chart yet."
                }
            } else if transposed_on {
                // Read-only preview of the source re-spelled to the view. Editing
                // stays on the original so the file is never rewritten transposed.
                div { class: "min-h-0 flex-1 overflow-auto p-3",
                    pre {
                        class: "whitespace-pre font-mono text-sm leading-relaxed text-foreground",
                        "{keyflow::transpose::transpose_source(&buffer, &view.to_chart_view())}"
                    }
                }
            } else {
                // `editor-app props-collapsed` reuses the note editor's chrome
                // (the frontmatter widget is irrelevant here and stays hidden).
                div { class: "editor-app props-collapsed min-h-0 flex-1 overflow-auto",
                    div { class: "editor-frame editor-frame--flush",
                        editor::Editor {
                            state,
                            keymap,
                            decorations: editor::editor_view::DecorationSource::ptr(keyflow_decorations),
                        }
                    }
                }
            }
        }
    }
}
