//! Testing app for the editor crate. Hosts an `<Editor>`
//! plus a debug panel that mirrors the live `EditorState` so we
//! can see selection / doc / transactions while typing.

use dioxus::prelude::*;
use editor::{
    bracket_match, commands, editor_view, markdown, DecoratedRange, Editor, EditorState, Keymap,
};

#[cfg(not(target_arch = "wasm32"))]
mod lsp;

/// Combined decoration source — markdown live-preview plus
/// bracket-pair highlighting. Wrapped via
/// `DecorationSource::ptr` (the stateless fn-pointer shape) so
/// composition is just concatenation; the inner builders dedupe
/// nothing, but our overlapping mark spans on brackets sit next
/// to each other without conflict.
fn combined_decorations(state: &EditorState) -> Vec<DecoratedRange> {
    let mut out = markdown::live_preview(state);
    out.extend(bracket_match::bracket_match(state));
    out
}

/// Demo trigger-autocomplete source — a static "vault" so the
/// playground exercises `[[` wikilink and `#` tag completion. Real
/// hosts pass a stateful `CompletionSource::new(..)` closing over
/// their vault index; the editor only sees `(query, kind) ->
/// Vec<Candidate>`.
fn demo_completion(
    query: &str,
    kind: editor_view::trigger::CompletionKind,
) -> Vec<editor_view::trigger::Candidate> {
    use editor_view::trigger::{Candidate, CompletionKind};
    let pool: &[(&str, &str)] = match kind {
        CompletionKind::Wikilink => &[
            ("Welcome", "playground/Welcome.md"),
            ("Daily Note", "journal/Daily Note.md"),
            ("Editor Design", "docs/Editor Design.md"),
            ("Roadmap", "docs/Roadmap.md"),
        ],
        CompletionKind::Tag => &[
            ("project", ""),
            ("project/active", ""),
            ("inbox", ""),
            ("someday", ""),
        ],
    };
    let q = query.to_lowercase();
    pool.iter()
        .filter(|(name, _)| name.to_lowercase().contains(&q))
        .map(|(name, detail)| Candidate {
            label: (*name).to_string(),
            insert_text: (*name).to_string(),
            detail: (*detail).to_string(),
        })
        .collect()
}

// Web/desktop link this bundled asset; native inlines the CSS via
// `include_str!` (see `App`), so the asset handle is unused there.
#[cfg(not(feature = "native"))]
const STYLE: Asset = asset!("/assets/playground.css");

fn main() {
    init_tracing();
    tracing::info!("playground starting");
    // Native uses the fork's `dioxus_native::launch` (Blitz window + vello).
    // Web/desktop use the `dioxus` crate's launcher. `App` returns a
    // dioxus-core `Element` either way — both renderers share that core.
    #[cfg(feature = "native")]
    dioxus_native::launch(App);
    #[cfg(not(feature = "native"))]
    dioxus::launch(App);
}

/// Initialize tracing for the desktop binary: stdout + a rolling
/// logfile so we can tail edits while developing. The logfile
/// goes in the repo's `target/` (gitignored). Web target skips
/// this entirely — wasm can't write files; that path will get a
/// `tracing-wasm` subscriber in a follow-up commit.
#[cfg(not(target_arch = "wasm32"))]
fn init_tracing() {
    use tracing_subscriber::{
        fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
    };
    // Logfile in the repo's target/ dir so it tags along with
    // builds and is gitignored. `daily` rolls without bound; for
    // a dev playground that's fine.
    let log_dir = std::path::Path::new("target");
    let _ = std::fs::create_dir_all(log_dir);
    let file_appender = tracing_appender::rolling::daily(log_dir, "playground.log");
    // `_guard` must outlive the process so we don't lose buffered
    // writes on shutdown. Leak it intentionally — the binary's
    // lifetime is the right scope.
    let (nb_writer, guard) = tracing_appender::non_blocking(file_appender);
    std::mem::forget(guard);

    let env_filter = || {
        EnvFilter::try_from_env("EDITOR_LOG")
            .or_else(|_| EnvFilter::try_new("info,editor=debug,editor_view=debug,playground=debug"))
            .unwrap()
    };

    let stdout_layer = fmt::layer()
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_filter(env_filter());
    let file_layer = fmt::layer()
        .with_writer(nb_writer)
        .with_ansi(false)
        .with_target(true)
        .with_thread_ids(false)
        .with_thread_names(false)
        .with_filter(env_filter());

    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer)
        .init();
}

#[cfg(target_arch = "wasm32")]
fn init_tracing() {
    // Browser DevTools console gets the structured log stream.
    // Filter via `?log=trace` query for verbose; otherwise default
    // to `debug` for the editor crates and `info` everywhere else.
    let level = if read_query_flag("log") {
        tracing::Level::TRACE
    } else {
        tracing::Level::DEBUG
    };
    let cfg = tracing_wasm::WASMLayerConfigBuilder::new()
        .set_max_level(level)
        .build();
    tracing_wasm::set_as_global_default_with_config(cfg);
}

/// Look for a `?seed=...` query param in the page URL and
/// percent-decode it. Returns `Some` only on the web target;
/// on desktop there's no URL so we always use the default
/// seed.
///
/// Hand-rolled parser instead of `web_sys::UrlSearchParams` —
/// the latter pulls in a transitive `getrandom 0.3` dep that
/// needs the `wasm_js` feature flag we're not configuring.
#[cfg(target_arch = "wasm32")]
fn read_seed_query() -> Option<String> {
    let window = web_sys::window()?;
    let search = window.location().search().ok()?;
    // search starts with `?` — strip it and split on `&`.
    let trimmed = search.strip_prefix('?').unwrap_or(&search);
    for pair in trimmed.split('&') {
        if let Some(rest) = pair.strip_prefix("seed=") {
            return Some(percent_decode(rest));
        }
    }
    None
}

/// Look for `?flag` or `?flag=1` etc. — returns true when the
/// query string contains the named flag with a truthy value.
fn read_query_flag(_name: &str) -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        let window = match web_sys::window() {
            Some(w) => w,
            None => return false,
        };
        let search = window.location().search().unwrap_or_default();
        let trimmed = search.strip_prefix('?').unwrap_or(&search);
        for pair in trimmed.split('&') {
            let (k, v) = match pair.split_once('=') {
                Some((k, v)) => (k, v),
                None => (pair, "1"),
            };
            if k == _name {
                return matches!(v, "1" | "true" | "yes" | "on" | "");
            }
        }
        false
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        false
    }
}

#[cfg(target_arch = "wasm32")]
fn percent_decode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut bytes = s.bytes();
    while let Some(b) = bytes.next() {
        if b == b'%' {
            let h = bytes.next().and_then(|x| (x as char).to_digit(16));
            let l = bytes.next().and_then(|x| (x as char).to_digit(16));
            if let (Some(h), Some(l)) = (h, l) {
                out.push(((h * 16 + l) as u8) as char);
            }
        } else if b == b'+' {
            out.push(' ');
        } else {
            out.push(b as char);
        }
    }
    out
}

#[cfg(not(target_arch = "wasm32"))]
fn read_seed_query() -> Option<String> {
    None
}

#[component]
fn App() -> Element {
    // The whole editor state lives in this signal — the
    // `<Editor>` component reads it for rendering and writes a
    // new state on every input. We mirror it into the debug
    // panel so changes are visible as you type.
    // Initial seed text. Tests can override via `?seed=` query
    // (URL-decoded) so they don't have to start from the
    // welcome message — useful for isolating decoration-aware
    // typing tests from the markdown in the default seed.
    let state = use_signal(|| {
        let seed = read_seed_query().unwrap_or_else(|| {
            String::from(
                "---\n\
                 title: Editor playground\n\
                 tags: [editor, demo, live-preview]\n\
                 published: true\n\
                 draft: false\n\
                 author: cody\n\
                 created: 2026-05-20\n\
                 priority: 3\n\
                 description: |\n\
                   Multi-line YAML scalar — used to demo block-\n\
                   scalar parsing and editing in the Properties\n\
                   panel above.\n\
                 aliases:\n\
                   - playground\n\
                   - demo doc\n\
                 ---\n\
                 # Welcome to the Editor playground\n\
                 \n\
                 Setext-style heading too\n\
                 ========================\n\
                 \n\
                 Subtitle via dashes\n\
                 -------------------\n\
                 \n\
                 ## Inline styles\n\
                 \n\
                 **bold**, *italic*, ***bold italic***, ~~strikethrough~~, \
                 ==highlight==, `inline code`, and an inline footnote^[click the marker to edit me].\n\
                 \n\
                 Links: standard [Anthropic](https://anthropic.com), \
                 an autolink <https://obsidian.md>, \
                 wikilinks: [[Editor Roadmap]], [[Project README|the readme]], \
                 and to a header [[Editor Roadmap#Goals]]. Unresolved targets render red \
                 (no vault yet, so every wikilink is unresolved). \
                 Tags like #editor #live-preview #notes/howto, \
                 and a footnote ref [^1]. \
                 Block id at end of paragraph ^demo-block-id\n\
                 \n\
                 ## Block styles\n\
                 \n\
                 > Blockquotes are just blockquotes.\n\
                 > Multi-line works too.\n\
                 \n\
                 - Unordered list item\n\
                 - Another item\n\
                 \n\
                 1. Ordered list\n\
                 2. Stays numbered\n\
                 \n\
                 - [ ] Click the checkbox to toggle\n\
                 - [x] Done\n\
                 - [/] In progress (custom Tasks-plugin status)\n\
                 - [>] Forwarded\n\
                 - [-] Cancelled\n\
                 \n\
                 ### Callouts (all 13 types)\n\
                 \n\
                 > [!note] Note\n\
                 > Callouts share the blockquote syntax.\n\
                 \n\
                 > [!tip] Tip\n\
                 > Press `/` anywhere to open the slash-command menu.\n\
                 \n\
                 > [!warning]+ Collapsible warning\n\
                 > The `+`/`-` on the type marker controls folded default.\n\
                 \n\
                 > [!danger] Danger\n\
                 > High-stakes call-out style.\n\
                 \n\
                 > [!info] Info\n\
                 > Use the slash menu `/callout` to insert any of the others — \
                 abstract, info, success, question, failure, bug, example, quote.\n\
                 \n\
                 > [!example] Nested callouts\n\
                 > The outer is an example callout.\n\
                 > > [!warning] Two levels deep\n\
                 > > The body inherits the inner kind.\n\
                 > > > [!danger] Three levels\n\
                 > > > Rare in practice but supported.\n\
                 > Back to the outer level.\n\
                 \n\
                 ### Table\n\
                 \n\
                 | Feature             | Status        | Notes                          |\n\
                 |---------------------|---------------|--------------------------------|\n\
                 | Headings (1-6)      | ✅ Mod-1..6   | Mod-0 strips                   |\n\
                 | Tables              | ✅            | GFM pipe form                  |\n\
                 | Math (inline+block) | ✅ Typst      | Compiled per-pass via cache    |\n\
                 | Mermaid             | ✅ pure Rust  | mermaid-rs-renderer            |\n\
                 | Frontmatter         | ✅ editable   | bool/number/date/list/text     |\n\
                 | Vim                 | ✅ default-on | C/D/Y / gg / gu/gU/g~ / */#/n  |\n\
                 | Slash menu          | ✅ `/` opens  | `/callout`, `/typst`, …        |\n\
                 \n\
                 ### Math\n\
                 \n\
                 Inline math compiles via Typst: $E = m c^2$, and a longer one — \
                 $sum_(i=1)^n i = n(n+1)/2$.\n\
                 \n\
                 $$ integral_0^1 x^2 d x = 1/3 $$\n\
                 \n\
                 ### Typst block\n\
                 \n\
                 ```typst\n\
                 = Typst block heading\n\
                 \n\
                 Full Typst documents render in-place.\n\
                 \n\
                 $ A = mat(1, 2; 3, 4) $\n\
                 ```\n\
                 \n\
                 ### Mermaid diagram\n\
                 \n\
                 ```mermaid\n\
                 flowchart LR\n\
                   A[Keystroke] --> B{Live preview}\n\
                   B -->|markdown| C[Decorations]\n\
                   B -->|math| D[Typst SVG]\n\
                   B -->|diagram| E[Mermaid SVG]\n\
                   C --> F[DOM patch]\n\
                   D --> F\n\
                   E --> F\n\
                 ```\n\
                 \n\
                 ### Editor commands\n\
                 \n\
                 - **Mod-B** / **Mod-I** — bold / italic\n\
                 - **Mod-K** — wrap as `[…](url)`\n\
                 - **Mod-L** — cycle list marker (none → `-` → `1.` → `- [ ]`)\n\
                 - **Mod-T** — toggle task on current line\n\
                 - **Mod-1**..**Mod-6** — heading levels; **Mod-0** strips\n\
                 - **Mod-E** — toggle reading mode\n\
                 - **`/`** — open the slash-command palette\n\
                 \n\
                 ### Embeds\n\
                 \n\
                 Wikilink embed: `![[diagram.png|320]]` (renders an `<img>` when the file resolves).\n\
                 \n\
                 ### Block IDs (Logseq-style references)\n\
                 \n\
                 This is a referenceable block — press Mod-Shift-K on any block to give it an id.\n\
                 id:: 5f9c1234-abcd-4ef0-8123-fedcba012345\n\
                 \n\
                 You can reference it inline like this: ((5f9c1234-abcd-4ef0-8123-fedcba012345)).\n\
                 \n\
                 Or embed the whole block as a card:\n\
                 \n\
                 {{embed ((5f9c1234-abcd-4ef0-8123-fedcba012345))}}\n\
                 \n\
                 ### Page + section embeds (Obsidian-style)\n\
                 \n\
                 Embed a whole page (placeholder until multi-file lookup):\n\
                 \n\
                 ![[Project README]]\n\
                 \n\
                 Embed a section by heading — resolves intra-doc if the heading lives in this file:\n\
                 \n\
                 ![[#Math]]\n\
                 \n\
                 Embed a block by Obsidian short-id (the `^demo-block-id` anchor near the top):\n\
                 \n\
                 ![[#^demo-block-id]]\n\
                 \n\
                 ### Code fences (syntax highlighting)\n\
                 \n\
                 ```rust\n\
                 fn greet(name: &str) -> String {\n\
                     format!(\"Hello, {name}!\")\n\
                 }\n\
                 ```\n\
                 \n\
                 ```python\n\
                 def greet(name):\n\
                     return f\"Hello, {name}!\"\n\
                 ```\n\
                 \n\
                 ```ts\n\
                 const greet = (name: string) => `Hello, ${name}!`;\n\
                 ```\n\
                 \n\
                 Comments like %% this %% hide on focus-away.\n\
                 \n\
                 ---\n\
                 \n\
                 [^1]: Footnote definitions live at the bottom of the file.\n\
                 \n\
                 Markers stay visible while your caret is on the span — move away and they fade out.",
            )
        });
        EditorState::new(seed)
    });

    // Minimal demo keymap. The browser already handles
    // Backspace/Delete/Enter for the textarea — these bindings
    // intercept and route them through commands instead, so we
    // can see them flow through the State → Transaction loop in
    // the debug panel.
    // Enter is handled by the view's beforeinput bridge (which
    // routes it through `enter_continue_list` Rust-side) rather
    // than the keymap, so the browser's default
    // `insertParagraph` never sneaks a stray `\n` in alongside
    // our authored change.
    let keymap = Keymap::new()
        .with("Mod-a", commands::select_all as _)
        .with("Mod-b", commands::toggle_bold as _)
        .with("Mod-i", commands::toggle_italic as _)
        .with("Mod-k", commands::toggle_link as _)
        .with("Mod-Shift-k", |s: &_| commands::add_block_id(s).map(|(t, _)| t))
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
        .with("Tab", |s: &_| {
            commands::tab_list_indent(s, false).or_else(|| commands::indent_more(s))
        })
        .with("Shift-Tab", |s: &_| {
            commands::tab_list_indent(s, true).or_else(|| commands::indent_less(s))
        })
        .with("Backspace", commands::delete_backward as _)
        .with("Delete", commands::delete_forward as _)
        // Word-wise deletes — same shared commands the native path
        // wires as its Ctrl-Backspace/Delete default action.
        .with("Mod-Backspace", commands::delete_word_backward as _)
        .with("Mod-Delete", commands::delete_word_forward as _);

    // Vim modal state. Default-on per user preference — toggle
    // with `?novim=1` in the URL to fall back to plain editing.
    let vim = use_signal(editor::editor_vim::VimState::new);
    let vim_enabled = !read_query_flag("novim");

    // ── LSP (desktop/native only) ────────────────────────────
    //
    // Set `EDITOR_LSP_CMD` to enable, e.g.
    //   EDITOR_LSP_CMD="python3 tools/demo_ls.py" cargo run -p playground
    // Diagnostics arrive as decorations in `lsp_decos`; edits are
    // forwarded to the client thread via the `on_transaction` sink.
    #[cfg(not(target_arch = "wasm32"))]
    let mut lsp_decos = use_signal(Vec::<DecoratedRange>::new);
    #[cfg(not(target_arch = "wasm32"))]
    let lsp_bridge = use_hook(|| {
        std::rc::Rc::new(std::cell::RefCell::new(lsp::start(
            state.peek().doc.clone(),
        )))
    });
    #[cfg(not(target_arch = "wasm32"))]
    {
        let bridge = lsp_bridge.clone();
        use_future(move || {
            let rx = bridge
                .borrow_mut()
                .as_mut()
                .and_then(|b| b.decorations.take());
            async move {
                let Some(mut rx) = rx else { return };
                while let Some(d) = rx.recv().await {
                    let ranges: Vec<(usize, usize)> = d.iter().map(|r| (r.from, r.to)).collect();
                    tracing::debug!(count = d.len(), ?ranges, "lsp: decorations received on UI");
                    lsp_decos.set(d);
                }
            }
        });
    }

    // Transaction sink: handles vim's `:w`/`:q` events and feeds
    // the LSP thread. (`:w` writes to target/playground-saved.md —
    // the playground has no real file; an app host saves to its
    // vault instead.)
    #[cfg(not(target_arch = "wasm32"))]
    let on_tx = {
        let events = lsp_bridge.borrow().as_ref().map(|b| b.events.clone());
        Callback::new(move |ev: editor::TransactionEvent| {
            match ev.user_event.as_deref() {
                Some("save") | Some("save-quit") => {
                    let path = std::path::Path::new("target/playground-saved.md");
                    match std::fs::write(path, ev.doc_after.to_string()) {
                        Ok(()) => tracing::info!("vim :w — saved to {}", path.display()),
                        Err(e) => tracing::error!("vim :w failed: {e}"),
                    }
                    if ev.user_event.as_deref() == Some("save-quit") {
                        std::process::exit(0);
                    }
                }
                Some("quit") => std::process::exit(0),
                _ => {}
            }
            if let Some(tx) = &events {
                let _ = tx.send(ev);
            }
        })
    };
    #[cfg(target_arch = "wasm32")]
    let on_tx = Callback::new(move |ev: editor::TransactionEvent| {
        if let Some(kind) = ev.user_event.as_deref() {
            tracing::info!("vim ex event: {kind} (no filesystem on web)");
        }
    });

    // Decoration source — created once (Rc identity is the
    // prop-diff contract for stateful sources). Splices the LSP
    // squiggles in after the markdown/bracket decorations.
    let deco_source = use_hook(|| {
        #[cfg(not(target_arch = "wasm32"))]
        {
            editor_view::DecorationSource::new(move |s: &EditorState| {
                let mut v = combined_decorations(s);
                v.extend(lsp_decos.read().iter().cloned());
                v
            })
        }
        #[cfg(target_arch = "wasm32")]
        {
            editor_view::DecorationSource::ptr(combined_decorations)
        }
    });

    // Slash-command palette. The Editor refreshes this on every
    // doc change via `detect_slash`; the `SlashMenu` component
    // renders the popup directly inside the editor frame.
    let slash = use_signal(|| None::<editor::editor_view::slash::SlashState>);

    // Stylesheet node. Web/desktop link the bundled asset (served by `dx`).
    // Native inlines the CSS: a raw Blitz binary (run outside `dx`) can't
    // resolve the bundled `asset!` URL, so a link 404s and the doc renders as
    // unstyled HTML — `include_str!` bakes the CSS into the binary instead.
    // (rsx can't `#[cfg]` a child node, so the variant is chosen here.)
    #[cfg(not(feature = "native"))]
    let style_node = rsx! { document::Link { rel: "stylesheet", href: STYLE } };
    #[cfg(feature = "native")]
    let style_node =
        rsx! { style { dangerous_inner_html: include_str!("../assets/playground.css") } };

    rsx! {
        {style_node}
        div { class: "page",
            header { class: "page-header",
                h1 { "Editor" }
                p { class: "subtitle", "Text playground" }
            }
            div { class: "split",
                section { class: "editor-pane",
                    h2 { "Editor" }
                    div { class: "editor-frame",
                        if read_query_flag("nodeco") {
                            Editor {
                                state,
                                keymap: keymap.clone(),
                                vim: if vim_enabled { Some(vim) } else { None },
                                slash: Some(slash),
                                on_transaction: on_tx,
                            }
                        } else {
                            Editor {
                                state,
                                keymap: keymap.clone(),
                                decorations: deco_source.clone(),
                                vim: if vim_enabled { Some(vim) } else { None },
                                slash: Some(slash),
                                completion: editor_view::trigger::CompletionSource::ptr(
                                    demo_completion,
                                ),
                                on_transaction: on_tx,
                            }
                        }
                        editor::editor_view::slash::SlashMenu { state, slash }
                    }
                }
                section { class: "debug-pane",
                    h2 { "State" }
                    if vim_enabled {
                        VimStatus { vim }
                    }
                    DebugPanel { state }
                }
            }
        }
    }
}

/// Vim mode badge + pending-command preview. Mirrors the
/// vim-status strip an Obsidian / Neovim user would see in the
/// status bar.
#[component]
fn VimStatus(vim: Signal<editor::editor_vim::VimState>) -> Element {
    let v = vim.read();
    let (mode_label, mode_class) = match v.mode {
        editor::editor_vim::Mode::Normal => ("NORMAL", "mode-normal"),
        editor::editor_vim::Mode::Insert => ("INSERT", "mode-insert"),
        editor::editor_vim::Mode::VisualChar => ("VISUAL", "mode-visual"),
        editor::editor_vim::Mode::VisualLine => ("V-LINE", "mode-visual"),
        editor::editor_vim::Mode::VisualBlock => ("V-BLOCK", "mode-visual"),
        editor::editor_vim::Mode::Replace => ("REPLACE", "mode-replace"),
        editor::editor_vim::Mode::Command => ("COMMAND", "mode-command"),
    };
    let pending = format!(
        "{}{}{}",
        v.pending_count.map(|n| n.to_string()).unwrap_or_default(),
        v.pending_register.map(|r| format!("\"{r:?}")).unwrap_or_default(),
        v.pending_operator
            .map(|op| format!("{op:?}").chars().next().unwrap().to_string())
            .unwrap_or_default(),
    );
    // Live `:`/`/`/`?` buffer — what a vim user sees at the
    // bottom of the screen while typing an ex command or search.
    let cmdline = v
        .command_line
        .as_ref()
        .map(|c| format!("{}{}", c.kind.prompt(), c.buffer));
    rsx! {
        div { class: "vim-status",
            span { class: "vim-mode {mode_class}", "{mode_label}" }
            if let Some(c) = cmdline {
                span { class: "vim-cmdline", "{c}" }
            }
            span { class: "vim-pending", "{pending}" }
        }
    }
}

/// Shows the live document text + selection so we can verify
/// that transactions are actually flowing through state.
#[component]
fn DebugPanel(state: Signal<EditorState>) -> Element {
    let s = state.read();
    let text = s.doc.to_string();
    let len = s.doc.len();
    let primary = s.selection.primary();
    let ranges = s.selection.ranges().len();

    rsx! {
        dl { class: "debug-grid",
            dt { "doc length" }
            dd { id: "dbg-len", "{len}" }
            dt { "ranges" }
            dd { id: "dbg-ranges", "{ranges}" }
            dt { "primary anchor" }
            dd { id: "dbg-anchor", "{primary.anchor}" }
            dt { "primary head" }
            dd { id: "dbg-head", "{primary.head}" }
        }
        h3 { "doc.to_string()" }
        pre { id: "dbg-text", class: "debug-text", "{text}" }
    }
}
