//! App-wide keyboard shortcuts, driven by architect's keymap engine
//! (`architect-input`: a key trie, a modal [`InputProcessor`] with
//! multi-key sequences and leader keys, and the which-key model) and the
//! shared actions registry ([`crate::actions`]).
//!
//! ## Architecture
//!
//! ONE canonical keymap ([`BINDINGS`]) is shared across desktop, web and
//! mobile builds. Each entry names its chord/sequence, the **action id**
//! it fires (an id the [`crate::actions`] registry knows), a which-key
//! label, and an optional **web alternate** for combos the browser
//! refuses to yield. [`keymap_config`] compiles the table into an
//! `architect_input::KeymapConfig`; the processor resolves sequences,
//! including leader sequences like `<space> g h`, through its prefix
//! trie.
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
//! - the raw DOM closure runs the processor *synchronously* (`press`)
//!   so a match can `preventDefault()` + `stopPropagation()` in the same
//!   tick (that's what lets app shortcuts win over browser natives, and
//!   stops `<space>` from scrolling while a leader sequence is pending);
//! - matched ids / which-key updates cross back into the Dioxus runtime
//!   over a channel a spawned task drains;
//! - `keyup` feeds [`InputProcessor::notify_key_release`] so held-key /
//!   sticky-prefix bookkeeping stays correct (without it a second
//!   `<space>` reads as auto-repeat and `space space` never fires).
//!
//! ## Which-key
//!
//! While a sequence is pending, [`InputProcessor::which_key`] lists what
//! can follow it, named by [`which_key_labels`]: leaf continuations show
//! the binding's label, deeper prefixes a group name ([`GROUPS`]).
//! [`WhichKeyOverlay`] draws it.
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

#[cfg(any(target_arch = "wasm32", test))]
use architect_input::{InputCommand, InputContext, InputEvent, KeyEvent};
use architect_input::{
    InputProcessor, KeyChord, KeyCode, KeymapConfig, WhichKey, WhichKeyEntry, WhichKeyLabels,
    parse_key_sequence,
};
use dioxus::prelude::*;

use crate::actions;

// ── binding table ───────────────────────────────────────────────────

/// One canonical binding: the single source of truth for the compiled
/// keymap, the which-key labels, and (via [`crate::actions`]) the
/// Settings table.
#[derive(Debug, PartialEq, Eq)]
pub struct Binding {
    /// Key sequence (`architect_input::parse_key_sequence` syntax: chords
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

// ── which-key ───────────────────────────────────────────────────────

/// Names for the intermediate prefixes (the which-key groups), in
/// [`BINDINGS`] syntax. A prefix not listed shows its key.
pub const GROUPS: &[(&str, &str)] = &[("space", "Leader"), ("space g", "Go to"), ("g", "Go to")];

/// The which-key labels: each binding's label at the sequence this build
/// binds, each [`GROUPS`] name at its prefix.
#[must_use]
pub fn which_key_labels() -> WhichKeyLabels {
    let leaves = BINDINGS.iter().map(|b| (effective_sequence(b), b.label));
    GROUPS
        .iter()
        .copied()
        .chain(leaves)
        .filter_map(|(seq, label)| Some((parse_key_sequence(seq).ok()?, label.to_owned())))
        .collect()
}

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

impl WhichKeyView {
    /// The overlay's rows for the processor's which-key model.
    #[must_use]
    pub fn from_model(model: &WhichKey) -> Self {
        Self {
            prefix: model
                .typed
                .iter()
                .map(chord_display)
                .collect::<Vec<_>>()
                .join(" "),
            entries: model.entries.iter().map(WkEntry::from_model).collect(),
        }
    }
}

impl WkEntry {
    fn from_model(entry: &WhichKeyEntry) -> Self {
        let key = chord_display(&entry.chord);
        Self {
            label: entry.label.clone().unwrap_or_else(|| key.clone()),
            is_branch: entry.is_group(),
            key,
        }
    }
}

/// How the overlay shows one chord: `SPC` for the leader, otherwise the
/// keymap notation (`n`, `Ctrl+p`).
fn chord_display(chord: &KeyChord) -> String {
    if *chord == KeyChord::plain(KeyCode::char(' ')) {
        "SPC".to_owned()
    } else {
        chord.to_string()
    }
}

// ── one keydown ─────────────────────────────────────────────────────

/// What one keydown resolved to.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, PartialEq)]
struct KeyPress {
    /// Action ids to run, in order.
    actions: Vec<String>,
    /// The which-key view while a sequence is pending; `None` once it
    /// ends.
    which_key: Option<WhichKeyView>,
}

#[cfg(any(target_arch = "wasm32", test))]
impl KeyPress {
    /// The app owns the event when it ran an action or holds a pending
    /// prefix (so `<space>` doesn't scroll mid-leader); misses are left
    /// to the browser.
    const fn claims_event(&self) -> bool {
        !self.actions.is_empty() || self.which_key.is_some()
    }
}

/// Feed one key to the processor: the actions it fires and the
/// which-key view left behind.
#[cfg(any(target_arch = "wasm32", test))]
fn press(engine: &mut InputProcessor, key: KeyEvent, labels: &WhichKeyLabels) -> KeyPress {
    let ctx = InputContext::new();
    let commands = engine.process(InputEvent::Key(key), &ctx);
    KeyPress {
        actions: commands
            .into_iter()
            .filter_map(|command| match command {
                InputCommand::Action(id) | InputCommand::ActionWithArgs { action: id, .. } => {
                    Some(id)
                }
                _ => None,
            })
            .collect(),
        which_key: engine
            .which_key(&ctx, labels)
            .as_ref()
            .map(WhichKeyView::from_model),
    }
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
// `Exec` / `WhichKey` are only built by the wasm DOM listeners.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
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
                        Msg::WhichKey(view) => {
                            if *whichkey.peek() != view {
                                whichkey.set(view);
                            }
                        }
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
        let index = crate::pages::vault::fetch_folder_index(
            slug.clone(),
            crate::document_session::VAULT_ID.to_owned(),
        )
        .await;
        let Ok(pages) = index else {
            // No server: the note is made on this device, under a path no
            // other device will pick, and syncs (and gets its file) when
            // the connection is back — `offline.doc.create`.
            let name = crate::doc_replica::device_note_path("Untitled");
            match crate::doc_replica::create_local_note(
                &slug,
                crate::document_session::VAULT_ID,
                &name,
            )
            .await
            {
                Ok(()) => {
                    pending_title.set(Some(name.clone()));
                    nav.push(crate::routes::Route::VaultRoute {
                        path: name,
                        org: String::new(),
                    });
                }
                Err(e) => tracing::warn!(%e, "new note (offline) failed"),
            }
            return;
        };
        let taken: std::collections::HashSet<String> =
            pages.into_iter().map(|p| p.path.to_lowercase()).collect();
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
        match crate::pages::vault::create_new_file(
            slug,
            crate::document_session::VAULT_ID.to_owned(),
            name.clone(),
        )
        .await
        {
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
///
/// The processor runs here, outside the Dioxus runtime, so a match can
/// `preventDefault()` in the same tick. That is also why this does not
/// use `architect_input::dioxus::use_input_processor`: its handle arms
/// the timeout with `spawn` (needs the runtime) and has no way to report
/// a key release.
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
        // only fires if no further key arrived (the processor owns no
        // clock).
        let epoch = Rc::new(Cell::new(0_u64));
        let labels = Rc::new(which_key_labels());

        let keydown = {
            let engine = engine.clone();
            let epoch = epoch.clone();
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
                        && (e.key() == " " || !engine.borrow().pending_keys().is_empty());
                    if !leader_ok {
                        return;
                    }
                }
                // OS auto-repeat never fires app actions.
                if e.repeat() {
                    return;
                }
                let Some(key) = key_event(&e) else {
                    return;
                };
                epoch.set(epoch.get().wrapping_add(1));
                let outcome = press(&mut engine.borrow_mut(), key, &labels);
                if engine.borrow().needs_timeout() {
                    schedule_sequence_timeout(engine.clone(), epoch.clone(), msg_tx.clone());
                }
                if outcome.claims_event() {
                    e.prevent_default();
                    e.stop_propagation();
                }
                let KeyPress { actions, which_key } = outcome;
                for id in actions {
                    let _ = msg_tx.unbounded_send(Msg::Exec(id));
                }
                let _ = msg_tx.unbounded_send(Msg::WhichKey(which_key));
            })
        };

        let keyup = {
            let engine = engine.clone();
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                if let Some(k) = key_event(&e) {
                    let _ = engine
                        .borrow_mut()
                        .notify_key_release(&KeyChord::new(k.key, k.modifiers));
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
/// key arrived first. Also clears the which-key overlay.
#[cfg(target_arch = "wasm32")]
fn schedule_sequence_timeout(
    engine: Rc<RefCell<InputProcessor>>,
    epoch: Rc<std::cell::Cell<u64>>,
    msg_tx: futures_channel::mpsc::UnboundedSender<Msg>,
) {
    let want = epoch.get();
    let ms = u32::try_from(engine.borrow().timeout_duration().as_millis()).unwrap_or(1000);
    gloo_timers::callback::Timeout::new(ms, move || {
        if epoch.get() == want {
            let _ = engine.borrow_mut().timeout_expired();
            let _ = msg_tx.unbounded_send(Msg::WhichKey(None));
        }
    })
    .forget();
}

/// `web_sys::KeyboardEvent` → an input key event, through architect's
/// key mapping. `None` for keys no binding can name (bare modifiers,
/// media keys).
#[cfg(target_arch = "wasm32")]
fn key_event(e: &web_sys::KeyboardEvent) -> Option<KeyEvent> {
    let key = e.key().parse::<Key>().ok()?;
    Some(KeyEvent {
        key: architect_input::dioxus::convert_key(&key)?,
        modifiers: architect_input::Modifiers {
            ctrl: e.ctrl_key(),
            alt: e.alt_key(),
            shift: e.shift_key(),
            meta: e.meta_key(),
        },
    })
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

#[cfg(test)]
mod tests {
    use architect_input::{Modifiers, parse_key_chord};

    use super::*;

    fn engine(prioritize_app: bool) -> InputProcessor {
        InputProcessor::from_config(keymap_config(prioritize_app)).expect("keymap compiles")
    }

    /// Press then release every chord of `seq` the way the DOM does,
    /// returning the last keydown's outcome.
    fn type_seq(engine: &mut InputProcessor, seq: &str) -> KeyPress {
        let labels = which_key_labels();
        let mut last = None;
        for token in seq.split(' ') {
            let chord = parse_key_chord(token).expect("chord parses");
            let key = KeyEvent {
                key: chord.key.clone(),
                modifiers: chord.modifiers,
            };
            last = Some(press(engine, key, &labels));
            let _ = engine.notify_key_release(&chord);
        }
        last.expect("a non-empty sequence")
    }

    #[test]
    fn every_sequence_parses() {
        for b in BINDINGS {
            parse_key_sequence(b.seq).unwrap_or_else(|e| panic!("bad seq {:?}: {e}", b.seq));
            if let Some(web) = b.web {
                parse_key_sequence(web).unwrap_or_else(|e| panic!("bad web alt {:?}: {e}", web));
            }
        }
    }

    #[test]
    fn full_keymap_compiles_into_a_processor() {
        assert!(InputProcessor::<String>::from_config(keymap_config(true)).is_ok());
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
    fn every_binding_fires_its_action() {
        for b in BINDINGS {
            let seq = effective_sequence(b);
            let got = type_seq(&mut engine(true), seq);
            assert_eq!(got.actions, vec![b.action.to_owned()], "{seq}");
            assert!(got.which_key.is_none(), "{seq} leaves nothing pending");
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
    fn priority_off_leaves_browser_combos_to_the_browser() {
        let mut keys = engine(false);
        let got = type_seq(&mut keys, "ctrl+p");
        assert!(got.actions.is_empty());
        assert!(!got.claims_event());
        assert_eq!(
            type_seq(&mut keys, "ctrl+\\").actions,
            vec![actions::TOGGLE_SIDEBAR.to_owned()]
        );
    }

    #[test]
    fn a_leader_prefix_claims_the_key_and_shows_which_key() {
        let got = type_seq(&mut engine(true), "space");
        assert!(got.actions.is_empty());
        assert!(got.claims_event(), "space must not scroll mid-leader");
        assert_eq!(got.which_key.expect("pending").prefix, "SPC");
    }

    #[test]
    fn space_space_searches_everything() {
        assert_eq!(
            type_seq(&mut engine(true), "space space").actions,
            vec![actions::SEARCH_ALL.to_owned()]
        );
    }

    #[test]
    fn a_miss_ends_the_sequence() {
        let mut keys = engine(true);
        let got = type_seq(&mut keys, "g x");
        assert!(got.actions.is_empty());
        assert!(got.which_key.is_none());
        assert_eq!(
            type_seq(&mut keys, "g h").actions,
            vec![actions::NAV_HOME.to_owned()]
        );
    }

    #[test]
    fn whichkey_lists_leader_group() {
        let view = type_seq(&mut engine(true), "space")
            .which_key
            .expect("leader has entries");
        assert_eq!(view.prefix, "SPC");
        // `g` under the leader is a branch (the go-to group).
        let g = view
            .entries
            .iter()
            .find(|e| e.key == "g")
            .expect("g present");
        assert!(g.is_branch);
        assert_eq!(g.label, "Go to");
        // `n` is a leaf (new note); `SPC` is the search leaf.
        let n = view
            .entries
            .iter()
            .find(|e| e.key == "n")
            .expect("n present");
        assert!(!n.is_branch);
        assert_eq!(n.label, "New note");
        assert!(
            view.entries
                .iter()
                .any(|e| e.key == "SPC" && e.label == "Search everything")
        );
    }

    #[test]
    fn whichkey_descends_into_go_group() {
        let view = type_seq(&mut engine(true), "space g")
            .which_key
            .expect("go group entries");
        assert_eq!(view.prefix, "SPC g");
        assert!(view.entries.iter().all(|e| !e.is_branch));
        assert!(
            view.entries
                .iter()
                .any(|e| e.key == "h" && e.label == "Home")
        );
    }

    #[test]
    fn the_overlay_shows_modified_chords_in_keymap_notation() {
        let chord = KeyChord::new(KeyCode::char('p'), Modifiers::CTRL);
        assert_eq!(chord_display(&chord), "Ctrl+p");
    }
}
