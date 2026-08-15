//! App-wide keyboard shortcuts, driven by the `input` crate's
//! trie-based [`InputProcessor`] (multi-key sequences, leader keys,
//! modal-ready) and the shared actions registry ([`crate::actions`]).
//!
//! ## Architecture
//!
//! ONE canonical keymap ([`BINDINGS`]) is shared across desktop, web and
//! mobile builds. Each entry names its chord/sequence, the **action id**
//! it fires (an id the [`crate::actions`] registry knows), a which-key
//! label, and an optional **web alternate** for combos the browser
//! refuses to yield. [`keymap_config`] compiles the table into an
//! `input::KeymapConfig`; the processor resolves sequences — including
//! leader sequences like `<space> g h` — through its prefix trie.
//!
//! Matched actions are executed through
//! [`actions_standalone::StandaloneActions::execute`]: the same code
//! path the command palette uses, so a shortcut and a palette click run
//! identical logic. The registry's handler resolves the id to an
//! [`crate::actions::Intent`] and forwards it to the **effect loop** in
//! [`use_app_shortcuts`], which owns the Dioxus signals + the router's
//! `Navigator` and performs the effect inside the runtime.
//!
//! The wasm build installs **capture-phase** `keydown`/`keyup`
//! listeners on the document:
//!
//! - the raw DOM closure runs the processor *synchronously* so a match
//!   can `preventDefault()` + `stopPropagation()` in the same tick
//!   (that's what lets app shortcuts win over browser natives, and
//!   stops `<space>` from scrolling while a leader sequence is pending);
//! - matched ids / which-key updates cross back into the Dioxus runtime
//!   over a channel a spawned task drains;
//! - `keyup` feeds [`InputProcessor::notify_key_release`] so held-key /
//!   sticky-prefix bookkeeping stays correct.
//!
//! ## Which-key
//!
//! While a leader sequence is pending the DOM closure derives a
//! which-key view from [`BINDINGS`] — every entry whose sequence extends
//! the pending prefix by one chord — and renders [`WhichKeyOverlay`].
//! Leaf continuations show the action label; deeper prefixes render as a
//! named group (see [`group_label`]).
//!
//! **Typing guard**: when the focused element is a text control or a
//! contenteditable region (the vault editor), plain-key shortcuts are
//! skipped — only chords carrying Ctrl/Cmd pass, so `g h` (or the
//! `<space>` leader) typed into an input inserts text instead of firing.
//!
//! ## Browser-reserved combos
//!
//! `Ctrl+N/T/W` (+ shifted forms) are handled by Chromium before the
//! page sees the keydown; entries using one carry a web alternate.
//! Combos that shadow an interceptable browser default (`Ctrl+P` print,
//! `Ctrl+O` open-file, `Ctrl+K` search, `Ctrl+Shift+B` bookmarks,
//! `Ctrl+Shift+Z` redo) are flagged [`Binding::overrides_browser`]:
//! active while the "prioritize app shortcuts" pref is on.

use std::cell::RefCell;
use std::rc::Rc;

use dioxus::prelude::*;
use input::{InputProcessor, KeymapConfig};

use crate::actions;

// ── binding table ───────────────────────────────────────────────────

/// One canonical binding: the single source of truth for the compiled
/// keymap, the which-key labels, and (via [`crate::actions`]) the
/// Settings table.
#[derive(Debug, PartialEq, Eq)]
pub struct Binding {
    /// Key sequence (`input::parse_key_sequence` syntax: chords
    /// space-separated, `ctrl+`/`alt+`/`shift+`/`cmd+` modifiers, and
    /// tokens like `space`, `comma`). Leader sequences begin with
    /// `space`.
    pub seq: &'static str,
    /// Web alternate when `seq` is browser-RESERVED (undeliverable).
    /// `None` = works everywhere.
    pub web: Option<&'static str>,
    /// Action id fired on match — resolved through the actions registry.
    pub action: &'static str,
    /// Which-key label for the final chord of `seq`.
    pub label: &'static str,
    /// Shadows an interceptable browser default — dropped when the
    /// "prioritize app shortcuts" pref is off.
    pub overrides_browser: bool,
}

/// The canonical app bindings. Leaf entries only — intermediate prefixes
/// (`space`, `space g`, `g`) are implied and drive the which-key groups.
pub const BINDINGS: &[Binding] = &[
    // ── direct chords ──
    Binding {
        seq: "ctrl+p",
        web: None,
        action: actions::PALETTE_TOGGLE,
        label: "Command palette",
        overrides_browser: true,
    },
    Binding {
        seq: "ctrl+k",
        web: None,
        action: actions::PALETTE_TOGGLE,
        label: "Command palette",
        overrides_browser: true,
    },
    Binding {
        seq: "ctrl+o",
        web: None,
        action: actions::PICKER_NOTES,
        label: "Find note",
        overrides_browser: true,
    },
    Binding {
        seq: "ctrl+\\",
        web: None,
        action: actions::TOGGLE_SIDEBAR,
        label: "Toggle sidebar",
        overrides_browser: false,
    },
    Binding {
        seq: "ctrl+shift+b",
        web: None,
        action: actions::TOGGLE_PANEL,
        label: "Toggle panel",
        overrides_browser: true,
    },
    Binding {
        seq: "ctrl+shift+z",
        web: None,
        action: actions::TOGGLE_ZEN,
        label: "Toggle zen",
        overrides_browser: true,
    },
    // Ctrl+N is browser-RESERVED on web → Ctrl+Alt+N there.
    Binding {
        seq: "ctrl+n",
        web: Some("ctrl+alt+n"),
        action: actions::CAPTURE_FLEETING,
        label: "Capture",
        overrides_browser: false,
    },
    Binding {
        seq: "ctrl+shift+f",
        web: None,
        action: actions::CAPTURE_FLEETING,
        label: "Capture",
        overrides_browser: false,
    },
    Binding {
        seq: "ctrl+comma",
        web: None,
        action: actions::NAV_SETTINGS,
        label: "Settings",
        overrides_browser: false,
    },
    // ── `g` nav (vim-style) ──
    Binding {
        seq: "g h",
        web: None,
        action: actions::NAV_HOME,
        label: "Home",
        overrides_browser: false,
    },
    Binding {
        seq: "g i",
        web: None,
        action: actions::NAV_INBOX,
        label: "Inbox",
        overrides_browser: false,
    },
    Binding {
        seq: "g t",
        web: None,
        action: actions::NAV_TASKS,
        label: "Tasks",
        overrides_browser: false,
    },
    // ── `<space>` leader ──
    Binding {
        seq: "space space",
        web: None,
        action: actions::SEARCH_ALL,
        label: "Search everything",
        overrides_browser: false,
    },
    Binding {
        seq: "space n",
        web: None,
        action: actions::NEW_NOTE,
        label: "New note",
        overrides_browser: false,
    },
    Binding {
        seq: "space t",
        web: None,
        action: actions::NAV_TASKS,
        label: "Tasks",
        overrides_browser: false,
    },
    Binding {
        seq: "space c",
        web: None,
        action: actions::CAPTURE_FLEETING,
        label: "Capture",
        overrides_browser: false,
    },
    Binding {
        seq: "space p",
        web: None,
        action: actions::INBOX_PROCESS,
        label: "Process inbox",
        overrides_browser: false,
    },
    // ── `<space> g` go-to group ──
    Binding {
        seq: "space g h",
        web: None,
        action: actions::NAV_HOME,
        label: "Home",
        overrides_browser: false,
    },
    Binding {
        seq: "space g i",
        web: None,
        action: actions::NAV_INBOX,
        label: "Inbox",
        overrides_browser: false,
    },
    Binding {
        seq: "space g t",
        web: None,
        action: actions::NAV_TASKS,
        label: "Tasks",
        overrides_browser: false,
    },
    Binding {
        seq: "space g p",
        web: None,
        action: actions::NAV_PROJECTS,
        label: "Projects",
        overrides_browser: false,
    },
    Binding {
        seq: "space g m",
        web: None,
        action: actions::NAV_TIMER,
        label: "Timer",
        overrides_browser: false,
    },
    Binding {
        seq: "space g v",
        web: None,
        action: actions::NAV_VAULT,
        label: "Vault",
        overrides_browser: false,
    },
];

/// The sequence this build actually binds: the web alternate on wasm,
/// the canonical everywhere else.
#[must_use]
pub fn effective_sequence(b: &Binding) -> &'static str {
    if cfg!(target_arch = "wasm32") {
        b.web.unwrap_or(b.seq)
    } else {
        b.seq
    }
}

/// Compile the table into the processor's config. `prioritize_app` off
/// drops every binding flagged [`Binding::overrides_browser`].
#[must_use]
pub fn keymap_config(prioritize_app: bool) -> KeymapConfig {
    let mut cfg = KeymapConfig::default();
    let normal = cfg.keymap.entry("normal".to_string()).or_default();
    for b in BINDINGS {
        if b.overrides_browser && !prioritize_app {
            continue;
        }
        normal.insert(effective_sequence(b).to_string(), b.action.to_string());
    }
    cfg
}

// ── which-key derivation ────────────────────────────────────────────

/// A which-key overlay snapshot: the pending prefix + its continuations.
#[derive(Clone, PartialEq, Debug)]
pub struct WhichKeyView {
    /// Display of the pending prefix (`SPC`, `SPC g`, `g`).
    pub prefix: String,
    /// The next chord at each continuation, sorted by key.
    pub entries: Vec<WkEntry>,
}

/// One which-key row: the key to press next + what it does.
#[derive(Clone, PartialEq, Debug)]
pub struct WkEntry {
    /// The next key to press (`n`, `g`, `SPC`).
    pub key: String,
    /// Action label (leaf) or group name (branch).
    pub label: String,
    /// `true` when pressing this key descends into a deeper group.
    pub is_branch: bool,
}

/// Human name for an intermediate prefix (a which-key group). `None`
/// falls back to the raw key.
#[must_use]
pub fn group_label(prefix_tokens: &[String]) -> Option<&'static str> {
    match prefix_tokens.join(" ").as_str() {
        "space" => Some("Leader"),
        "space g" => Some("Go to"),
        "g" => Some("Go to"),
        _ => None,
    }
}

/// Pretty single-token display (`space` → `SPC`).
fn token_display(tok: &str) -> String {
    match tok {
        "space" => "SPC".to_string(),
        other => other.to_string(),
    }
}

/// Build the which-key view for a pending prefix (a list of chord
/// tokens in [`BINDINGS`] syntax), or `None` if nothing extends it.
#[must_use]
pub fn derive_whichkey(prefix: &[String]) -> Option<WhichKeyView> {
    use std::collections::BTreeMap;

    if prefix.is_empty() {
        return None;
    }
    // key token -> (label, is_branch). First writer wins; a leaf beats a
    // later branch discovery for the same key (shouldn't collide in a
    // well-formed table).
    let mut next: BTreeMap<String, (String, bool)> = BTreeMap::new();
    for b in BINDINGS {
        let toks: Vec<&str> = effective_sequence(b).split(' ').collect();
        if toks.len() <= prefix.len() {
            continue;
        }
        if !prefix.iter().zip(&toks).all(|(p, t)| p.as_str() == *t) {
            continue;
        }
        let key = toks[prefix.len()].to_string();
        let is_leaf = toks.len() == prefix.len() + 1;
        next.entry(key.clone()).or_insert_with(|| {
            if is_leaf {
                (b.label.to_string(), false)
            } else {
                let mut deeper: Vec<String> = prefix.to_vec();
                deeper.push(key.clone());
                (
                    group_label(&deeper).map_or_else(|| token_display(&key), str::to_string),
                    true,
                )
            }
        });
    }
    if next.is_empty() {
        return None;
    }
    let entries = next
        .into_iter()
        .map(|(key, (label, is_branch))| WkEntry {
            key: token_display(&key),
            label,
            is_branch,
        })
        .collect();
    Some(WhichKeyView {
        prefix: prefix
            .iter()
            .map(|t| token_display(t))
            .collect::<Vec<_>>()
            .join(" "),
        entries,
    })
}

// ── which-key overlay ───────────────────────────────────────────────

/// Bottom-right which-key panel, shown while a leader sequence is
/// pending — a compact column of key → action rows, lazygit-style.
/// Renders nothing when `state` is `None`.
#[component]
pub fn WhichKeyOverlay(state: Signal<Option<WhichKeyView>>) -> Element {
    let view = state.read().clone();
    let Some(view) = view else {
        return rsx! {};
    };
    rsx! {
        div { class: "pointer-events-none fixed bottom-4 right-4 z-50 flex justify-end",
            div { class: "pointer-events-auto w-64 max-w-[calc(100vw-2rem)] overflow-hidden rounded-lg border border-border bg-popover/95 shadow-2xl backdrop-blur",
                div { class: "flex items-center gap-2 border-b border-border/60 bg-muted/30 px-3 py-2",
                    kbd { class: "rounded border border-border bg-background px-1.5 py-0.5 font-mono text-[11px] font-semibold text-foreground shadow-sm",
                        "{view.prefix}"
                    }
                    span { class: "text-[11px] text-muted-foreground", "which key…" }
                }
                div { class: "flex max-h-72 flex-col overflow-y-auto py-1",
                    for entry in view.entries.iter() {
                        div { key: "{entry.key}", class: "flex items-center gap-2.5 px-3 py-1",
                            kbd { class: "min-w-[1.75rem] rounded border border-border/70 bg-muted/50 px-1.5 py-0.5 text-center font-mono text-[11px] text-foreground",
                                "{entry.key}"
                            }
                            span {
                                class: if entry.is_branch {
                                    "truncate text-[13px] font-medium text-primary"
                                } else {
                                    "truncate text-[13px] text-foreground/90"
                                },
                                if entry.is_branch {
                                    "+ {entry.label}"
                                } else {
                                    "{entry.label}"
                                }
                            }
                            if entry.is_branch {
                                span { class: "ml-auto text-[11px] text-muted-foreground", "›" }
                            }
                        }
                    }
                }
            }
        }
    }
}

// ── engine hook ─────────────────────────────────────────────────────

/// Messages crossing from the raw DOM closure / palette into the Dioxus
/// runtime. `Exec` runs an action through the registry (which enqueues
/// an `Intent`); `Intent` is performed directly; `WhichKey` updates the
/// overlay.
// `Exec` / `WhichKey` have no construction site yet (the palette
// performs Intents directly and which-key renders from its own
// signal); the variants document the crossing contract and stay.
#[allow(dead_code)]
enum Msg {
    Exec(String),
    Intent(actions::Intent),
    WhichKey(Option<WhichKeyView>),
}

/// Mount the app-wide shortcut engine + the actions registry. Call once
/// from an app-long component under `AppShell` (today:
/// [`crate::palette::CommandPalette`]). Returns the which-key overlay
/// signal the caller renders. Also provides [`crate::actions::ActionsCtx`]
/// via context so the palette executes through the same registry.
pub fn use_app_shortcuts() -> Signal<Option<WhichKeyView>> {
    use futures_channel::mpsc;

    let palette = use_context::<crate::palette::PaletteOpen>().0;
    let omni = use_context::<crate::palette::OmniOpen>().0;
    let fleeting = use_context::<crate::chrome::FleetingOpen>().0;
    let search = use_context::<crate::chrome::SearchOpen>().0;
    let pending_title = use_context::<crate::chrome::PendingTitleEdit>().0;
    let explorer = use_context::<Signal<crate::chrome::ExplorerOpen>>();
    let right_panel = use_context::<Signal<crate::chrome::RightPanelOpen>>();
    let zen = use_context::<crate::chrome::ZenMode>().0;
    let nav = use_navigator();
    let prefs = use_context::<crate::prefs::PrefsCtx>().prefs;
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let timer_muts = crate::stores::use_timer_mutations();

    let whichkey = use_signal(|| None::<WhichKeyView>);

    // Single channel: DOM/palette → the effect loop. `msg_tx` is shared
    // (registry handlers enqueue intents, the DOM closure enqueues
    // Exec/WhichKey); a spawned task drains it inside the runtime. The
    // receiver rides an `Rc` (use_hook needs `Clone`, `UnboundedReceiver`
    // isn't) and is `take`n once by the effect loop.
    let (msg_tx, msg_rx) = use_hook(|| {
        let (tx, rx) = mpsc::unbounded::<Msg>();
        (tx, Rc::new(RefCell::new(Some(rx))))
    });

    // The standalone actions registry, shared with the palette.
    let registry = use_context_provider(|| actions::ActionsCtx {
        registry: actions_standalone::StandaloneActions::new(),
    })
    .registry;

    // Register every action; each handler forwards an intent over the
    // channel. (register is async — the RwLock write is awaited.)
    use_hook({
        let registry = registry.clone();
        let tx = msg_tx.clone();
        move || {
            spawn(async move {
                let sink = move |intent: actions::Intent| {
                    let _ = tx.unbounded_send(Msg::Intent(intent));
                };
                actions::register_task_actions(&registry, sink).await;
            });
        }
    });

    // The processor, shared between the raw DOM closures (sync match +
    // preventDefault) and config reloads. Single-threaded on wasm.
    let engine: Rc<RefCell<InputProcessor>> = use_hook(|| {
        Rc::new(RefCell::new(
            InputProcessor::from_config(keymap_config(true)).unwrap_or_default(),
        ))
    });

    // Recompile the keymap when the "prioritize app shortcuts" pref
    // changes (boot load included).
    {
        let engine = engine.clone();
        use_effect(move || {
            let prioritize = prefs.read().shortcuts_priority;
            if engine
                .borrow_mut()
                .reload_config(keymap_config(prioritize))
                .is_err()
            {
                tracing::warn!("shortcut keymap reload failed");
            }
        });
    }

    // The effect loop: drains `msg_rx` and performs effects inside the
    // runtime (Dioxus signals + Navigator are !Send, so the registry
    // handlers can't touch them — they enqueue Intents this loop runs).
    use_hook({
        let registry = registry.clone();
        move || {
            let Some(mut rx) = msg_rx.borrow_mut().take() else {
                return;
            };
            let mut palette = palette;
            let mut omni = omni;
            let mut fleeting = fleeting;
            let mut search = search;
            let mut explorer = explorer;
            let mut right_panel = right_panel;
            let mut zen = zen;
            let mut whichkey = whichkey;
            spawn(async move {
                use futures_util::StreamExt;
                while let Some(msg) = rx.next().await {
                    match msg {
                        Msg::Exec(id) => {
                            registry.execute(id).await;
                        }
                        Msg::WhichKey(view) => whichkey.set(view),
                        Msg::Intent(intent) => perform_intent(
                            intent,
                            &mut palette,
                            &mut omni,
                            &mut fleeting,
                            &mut search,
                            pending_title,
                            &mut explorer,
                            &mut right_panel,
                            &mut zen,
                            nav,
                            org_list,
                            selection,
                            timer_muts,
                        ),
                    }
                }
            });
        }
    });

    #[cfg(target_arch = "wasm32")]
    install_dom_listeners(engine, msg_tx);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (engine, msg_tx);

    whichkey
}

/// Perform one resolved [`actions::Intent`]. Runs inside the Dioxus
/// runtime (called from the effect loop), so it may write signals,
/// navigate, and spawn async work.
#[allow(clippy::too_many_arguments)]
fn perform_intent(
    intent: actions::Intent,
    palette: &mut Signal<bool>,
    omni: &mut Signal<bool>,
    fleeting: &mut Signal<bool>,
    search: &mut Signal<bool>,
    pending_title: Signal<Option<String>>,
    explorer: &mut Signal<crate::chrome::ExplorerOpen>,
    right_panel: &mut Signal<crate::chrome::RightPanelOpen>,
    zen: &mut Signal<bool>,
    nav: dioxus_router::Navigator,
    org_list: Signal<Vec<crate::orgs::OrgMeta>>,
    selection: Signal<crate::orgs::OrgSelection>,
    timer_muts: crate::stores::TimerMutations,
) {
    use crate::actions::Intent;
    match intent {
        Intent::Nav(route) => {
            nav.push(route);
        }
        Intent::TogglePalette => {
            let cur = *palette.peek();
            palette.set(!cur);
        }
        Intent::OpenOmni => omni.set(true),
        Intent::OpenSearch => search.set(true),
        Intent::OpenFleeting => fleeting.set(true),
        Intent::ToggleSidebar => {
            let cur = explorer.peek().0;
            explorer.set(crate::chrome::ExplorerOpen(!cur));
        }
        Intent::TogglePanel => {
            let cur = right_panel.peek().0;
            right_panel.set(crate::chrome::RightPanelOpen(!cur));
        }
        Intent::ToggleZen => {
            let cur = *zen.peek();
            zen.set(!cur);
        }
        Intent::NewNote => {
            let slug = crate::orgs::active_slug(&selection.read(), &org_list.read());
            if slug.is_empty() {
                return;
            }
            spawn_new_note(slug, nav, pending_title);
        }
        Intent::StartTimer => {
            let Some((slug, org_id)) =
                crate::chrome::resolve_org(&selection.read(), &org_list.read())
            else {
                return;
            };
            timer_muts.start(
                slug,
                timer_proto::StartTimerRequest {
                    user_id: crate::chrome::owner_id(org_id),
                    org_id,
                    project_id: None,
                    project_path: String::new(),
                    task_note_path: String::new(),
                    description: String::new(),
                },
            );
        }
    }
}

/// Create a fresh vault note and open it, primed for immediate naming.
///
/// Obsidian-style names: `Untitled`, then `Untitled 2`… — the folder
/// index tells us which are taken. The created path is parked in
/// [`crate::chrome::PendingTitleEdit`] so the note opens straight into
/// title-edit mode (text selected) and typing immediately names it.
/// Shared by the `<space> n` intent and the dashboard's New-note tile.
pub fn spawn_new_note(
    slug: String,
    nav: dioxus_router::Navigator,
    mut pending_title: Signal<Option<String>>,
) {
    spawn(async move {
        let taken: std::collections::HashSet<String> =
            crate::pages::vault::fetch_folder_index(slug.clone())
                .await
                .map(|pages| pages.into_iter().map(|p| p.path.to_lowercase()).collect())
                .unwrap_or_default();
        let name = (1..)
            .map(|i| {
                if i == 1 {
                    "Untitled.md".to_string()
                } else {
                    format!("Untitled {i}.md")
                }
            })
            .find(|n| !taken.contains(&n.to_lowercase()))
            .expect("unbounded name search");
        match crate::pages::vault::create_new_file(slug, name.clone()).await {
            Ok(_) => {
                pending_title.set(Some(name.clone()));
                nav.push(crate::routes::Route::VaultRoute {
                    path: name,
                    org: String::new(),
                });
            }
            Err(e) => tracing::warn!(%e, "new note failed"),
        }
    });
}

// ── wasm wiring ─────────────────────────────────────────────────────

/// Capture-phase document listeners. Installed once; the palette lives
/// as long as the shell, so the listeners are `forget`-leaked.
#[cfg(target_arch = "wasm32")]
fn install_dom_listeners(
    engine: Rc<RefCell<InputProcessor>>,
    msg_tx: futures_channel::mpsc::UnboundedSender<Msg>,
) {
    use std::cell::Cell;

    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    use_hook(move || {
        // Bumped on every processed keydown; a pending-sequence timeout
        // only fires if no further key arrived (epoch dance — wasm has
        // no tokio time).
        let epoch = Rc::new(Cell::new(0_u64));
        // Mirrors the processor's pending sequence in BINDINGS-token
        // syntax, so we can derive the which-key continuations.
        let pending: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));

        let keydown = {
            let engine = engine.clone();
            let epoch = epoch.clone();
            let pending = pending.clone();
            let msg_tx = msg_tx.clone();
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                // Typing guard: plain keys belong to the focused text
                // control; only Ctrl/Cmd chords are shortcuts there.
                // Exception: the note editor in vim NORMAL mode may
                // start a leader sequence with `<space>` (and continue
                // a pending one) — plain letters stay vim motions, so
                // `g`/`gg` never leave the editor.
                if text_entry_focused() && !(e.ctrl_key() || e.meta_key()) {
                    let leader_ok = editor_vim_normal()
                        && editor_root_focused()
                        && (e.key() == " " || !pending.borrow().is_empty());
                    if !leader_ok {
                        return;
                    }
                }
                // OS auto-repeat never fires app actions.
                if e.repeat() {
                    return;
                }
                let Some(event) = convert_key_event(&e) else {
                    return;
                };
                let token = seq_token(&event);
                epoch.set(epoch.get().wrapping_add(1));
                let ctx = input::ActionContext::new();
                let commands = engine
                    .borrow_mut()
                    .process(input::InputEvent::Key(event), &ctx);

                let mut matched = false;
                let mut is_pending = false;
                for cmd in commands {
                    match cmd {
                        input::InputCommand::Action(id) => {
                            matched = true;
                            let _ = msg_tx.unbounded_send(Msg::Exec(id.as_str().to_string()));
                        }
                        input::InputCommand::ActionWithArgs { action, .. } => {
                            matched = true;
                            let _ = msg_tx.unbounded_send(Msg::Exec(action.as_str().to_string()));
                        }
                        input::InputCommand::Pending { .. } => {
                            is_pending = true;
                            schedule_sequence_timeout(
                                engine.clone(),
                                epoch.clone(),
                                pending.clone(),
                                msg_tx.clone(),
                            );
                        }
                        _ => {}
                    }
                }

                if is_pending {
                    if let Some(tok) = token {
                        pending.borrow_mut().push(tok);
                    }
                    let view = derive_whichkey(&pending.borrow());
                    let _ = msg_tx.unbounded_send(Msg::WhichKey(view));
                } else {
                    // Match or miss ends any pending sequence.
                    if !pending.borrow().is_empty() {
                        pending.borrow_mut().clear();
                        let _ = msg_tx.unbounded_send(Msg::WhichKey(None));
                    }
                }

                // A match OR an owned pending prefix claims the event
                // (so `<space>` doesn't scroll mid-leader); misses are
                // left to the browser/native behavior.
                if matched || is_pending {
                    e.prevent_default();
                    e.stop_propagation();
                }
            })
        };

        let keyup = {
            let engine = engine.clone();
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                if let Some(k) = convert_key_event(&e) {
                    let _ = engine
                        .borrow_mut()
                        .notify_key_release(input::KeyChord::new(k.key, k.modifiers));
                }
            })
        };

        if let Some(doc) = web_sys::window().and_then(|w| w.document()) {
            let _ = doc.add_event_listener_with_callback_and_bool(
                "keydown",
                keydown.as_ref().unchecked_ref(),
                true,
            );
            let _ = doc.add_event_listener_with_callback_and_bool(
                "keyup",
                keyup.as_ref().unchecked_ref(),
                true,
            );
        }
        keydown.forget();
        keyup.forget();
    });
}

/// Expire a pending prefix after the processor's timeout, unless another
/// key arrived first. Also clears the which-key mirror + overlay.
#[cfg(target_arch = "wasm32")]
fn schedule_sequence_timeout(
    engine: Rc<RefCell<InputProcessor>>,
    epoch: Rc<std::cell::Cell<u64>>,
    pending: Rc<RefCell<Vec<String>>>,
    msg_tx: futures_channel::mpsc::UnboundedSender<Msg>,
) {
    let want = epoch.get();
    let ms = u32::try_from(engine.borrow().timeout_duration().as_millis()).unwrap_or(1000);
    gloo_timers::callback::Timeout::new(ms, move || {
        if epoch.get() == want {
            let _ = engine.borrow_mut().timeout_expired();
            pending.borrow_mut().clear();
            let _ = msg_tx.unbounded_send(Msg::WhichKey(None));
        }
    })
    .forget();
}

/// The BINDINGS-syntax token for a key event, or `None` for keys that
/// never start/extend a sequence (non-character keys).
#[cfg(target_arch = "wasm32")]
fn seq_token(ev: &input::KeyEvent) -> Option<String> {
    let base = match &ev.key {
        input::KeyCode::Character(c) if c == " " => "space".to_string(),
        input::KeyCode::Character(c) => c.clone(),
        _ => return None,
    };
    let m = ev.modifiers;
    if m.ctrl || m.alt || m.shift || m.meta {
        let mut s = String::new();
        if m.ctrl {
            s.push_str("ctrl+");
        }
        if m.alt {
            s.push_str("alt+");
        }
        if m.shift {
            s.push_str("shift+");
        }
        if m.meta {
            s.push_str("cmd+");
        }
        s.push_str(&base);
        Some(s)
    } else {
        Some(base)
    }
}

// ── editor vim-mode bridge ──────────────────────────────────────────
//
// The note editor is a contenteditable region, so the typing guard
// would normally swallow every plain key. The focused note view
// publishes its vim mode here (see `note_view.rs`) so the guard can
// let the `<space>` leader through while the editor sits in NORMAL
// mode — where plain keys are motions, not text.
thread_local! {
    static EDITOR_VIM_NORMAL: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Publish whether the focused note editor is in vim NORMAL mode.
/// Called by the note view's status effect; cleared on unmount.
pub fn set_editor_vim_normal(on: bool) {
    EDITOR_VIM_NORMAL.with(|c| c.set(on));
}

#[cfg(target_arch = "wasm32")]
fn editor_vim_normal() -> bool {
    EDITOR_VIM_NORMAL.with(|c| c.get())
}

/// Is the DOM focus inside the note editor's root element?
#[cfg(target_arch = "wasm32")]
fn editor_root_focused() -> bool {
    web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
        .is_some_and(|el| el.closest(".editor-root").ok().flatten().is_some())
}

/// Is the focused element a text-entry surface?
#[cfg(target_arch = "wasm32")]
fn text_entry_focused() -> bool {
    use wasm_bindgen::JsCast;
    let Some(el) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.active_element())
    else {
        return false;
    };
    let tag = el.tag_name().to_ascii_lowercase();
    if matches!(tag.as_str(), "input" | "textarea" | "select") {
        return true;
    }
    el.dyn_ref::<web_sys::HtmlElement>()
        .is_some_and(|h| h.is_content_editable())
}

/// `web_sys::KeyboardEvent` → `input::KeyEvent`.
#[cfg(target_arch = "wasm32")]
fn convert_key_event(e: &web_sys::KeyboardEvent) -> Option<input::KeyEvent> {
    let key = e.key();
    let code = match key.as_str() {
        "ArrowUp" => input::KeyCode::ArrowUp,
        "ArrowDown" => input::KeyCode::ArrowDown,
        "ArrowLeft" => input::KeyCode::ArrowLeft,
        "ArrowRight" => input::KeyCode::ArrowRight,
        "Enter" => input::KeyCode::Enter,
        "Escape" => input::KeyCode::Escape,
        "Tab" => input::KeyCode::Tab,
        "Backspace" => input::KeyCode::Backspace,
        "Delete" => input::KeyCode::Delete,
        f if f.len() >= 2 && f.starts_with('F') => {
            input::KeyCode::F(f[1..].parse::<u8>().ok().filter(|n| (1..=12).contains(n))?)
        }
        c if c.chars().count() == 1 => input::KeyCode::Character(c.to_lowercase()),
        _ => return None,
    };
    Some(input::KeyEvent {
        key: code,
        modifiers: input::Modifiers {
            ctrl: e.ctrl_key(),
            alt: e.alt_key(),
            shift: e.shift_key(),
            meta: e.meta_key(),
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sequence_parses() {
        for b in BINDINGS {
            input::parse_key_sequence(b.seq).unwrap_or_else(|e| panic!("bad seq {:?}: {e}", b.seq));
            if let Some(web) = b.web {
                input::parse_key_sequence(web)
                    .unwrap_or_else(|e| panic!("bad web alt {:?}: {e}", web));
            }
        }
    }

    #[test]
    fn full_keymap_compiles_into_a_processor() {
        assert!(InputProcessor::from_config(keymap_config(true)).is_ok());
    }

    #[test]
    fn every_bound_action_is_registered() {
        // Every keymap action id must resolve to an intent, or a
        // shortcut would fire nothing.
        for b in BINDINGS {
            assert!(
                crate::actions::intent_for(b.action).is_some(),
                "unregistered action {}",
                b.action
            );
        }
    }

    #[test]
    fn priority_off_drops_only_overriding_bindings() {
        let on = keymap_config(true);
        let off = keymap_config(false);
        let overriding = BINDINGS.iter().filter(|b| b.overrides_browser).count();
        assert!(overriding > 0, "table should flag some overrides");
        assert_eq!(
            on.keymap["normal"].len(),
            off.keymap["normal"].len() + overriding
        );
        assert!(off.keymap["normal"].contains_key("g h"));
    }

    #[test]
    fn whichkey_lists_leader_group() {
        let view = derive_whichkey(&["space".to_string()]).expect("leader has entries");
        assert_eq!(view.prefix, "SPC");
        // `g` under the leader is a branch (the go-to group).
        let g = view
            .entries
            .iter()
            .find(|e| e.key == "g")
            .expect("g present");
        assert!(g.is_branch);
        assert_eq!(g.label, "Go to");
        // `n` is a leaf (new note).
        let n = view
            .entries
            .iter()
            .find(|e| e.key == "n")
            .expect("n present");
        assert!(!n.is_branch);
    }

    #[test]
    fn whichkey_descends_into_go_group() {
        let view =
            derive_whichkey(&["space".to_string(), "g".to_string()]).expect("go group entries");
        assert_eq!(view.prefix, "SPC g");
        assert!(view.entries.iter().all(|e| !e.is_branch));
        assert!(
            view.entries
                .iter()
                .any(|e| e.key == "h" && e.label == "Home")
        );
    }
}
