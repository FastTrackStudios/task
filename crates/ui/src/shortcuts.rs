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
//! The processor is owned by `architect_input::dioxus::use_input_processor`;
//! its handle arms the sequence timeout and mirrors the pending prefix
//! into a signal itself. The wasm build installs **capture-phase**
//! `keydown`/`keyup` listeners on the document that call the handle
//! directly, outside the Dioxus runtime:
//!
//! - `keydown` runs the processor *synchronously* (`key_down`) so a
//!   match can `preventDefault()` + `stopPropagation()` in the same tick
//!   (that's what lets app shortcuts win over browser natives, and stops
//!   `<space>` from scrolling while a leader sequence is pending);
//!   matched ids cross into the Dioxus runtime over a channel a spawned
//!   task drains, since running one awaits the registry;
//! - `keyup` reports the release (`key_up`) so held-key / sticky-prefix
//!   bookkeeping stays correct (without it a second `<space>` reads as
//!   auto-repeat and `space space` never fires).
//!
//! Vim's macro keys are off ([`keymap_processor`]): `q` and `@` are
//! ordinary keys.
//!
//! ## Which-key
//!
//! While a sequence is pending, [`InputProcessor::which_key`] lists what
//! can follow it, named by [`which_key_labels`]: leaf continuations show
//! the binding's label, deeper prefixes a group name ([`GROUPS`]). A memo
//! over the handle's pending prefix keeps the view current, timeouts
//! included; [`WhichKeyOverlay`] draws it.
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

use architect_input::dioxus::{InputHandle, use_input_processor};
use architect_input::{
    ConfigError, InputContext, InputProcessor, KeyChord, KeyCode, KeymapConfig, MacroKeys,
    WhichKey, WhichKeyEntry, WhichKeyLabels, parse_key_sequence,
};
#[cfg(any(target_arch = "wasm32", test))]
use architect_input::{InputCommand, InputEvent, KeyEvent};
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

/// The app's processor for [`keymap_config`]. Vim's macro keys are off:
/// Task is not a vim app, so an unbound `q` or `@` is an ordinary key and
/// never swallows the key after it as a register name.
///
/// # Errors
///
/// When the compiled keymap does not build (a test pins that it does).
pub fn keymap_processor(prioritize_app: bool) -> Result<InputProcessor, ConfigError> {
    Ok(InputProcessor::from_config(keymap_config(prioritize_app))?.with_macro_keys(MacroKeys::Off))
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

/// The overlay's view of `keys`' pending sequence; `None` when nothing is
/// pending.
#[must_use]
fn which_key_view(keys: &InputProcessor, labels: &WhichKeyLabels) -> Option<WhichKeyView> {
    keys.which_key(&InputContext::new(), labels)
        .as_ref()
        .map(WhichKeyView::from_model)
}

// ── one keydown ─────────────────────────────────────────────────────

/// What one keydown resolved to.
#[cfg(any(target_arch = "wasm32", test))]
#[derive(Debug, PartialEq)]
struct KeyPress {
    /// Action ids to run, in order.
    actions: Vec<String>,
    /// A sequence is still pending after the key.
    pending: bool,
}

#[cfg(any(target_arch = "wasm32", test))]
impl KeyPress {
    /// The app owns the event when it ran an action or holds a pending
    /// prefix (so `<space>` doesn't scroll mid-leader); misses are left
    /// to the browser.
    const fn claims_event(&self) -> bool {
        !self.actions.is_empty() || self.pending
    }
}

/// Feed one keydown to the shortcut processor. Callable outside the
/// Dioxus runtime (the capture-phase listener): the handle re-enters it
/// to update the which-key view and arm the sequence timeout.
#[cfg(any(target_arch = "wasm32", test))]
fn key_down(keys: &InputHandle, key: KeyEvent) -> KeyPress {
    let commands = keys.process_event(InputEvent::Key(key), &InputContext::new());
    KeyPress {
        actions: commands.into_iter().filter_map(fired_action).collect(),
        pending: keys.processor().needs_timeout(),
    }
}

/// The action id a command runs, if it runs one.
#[cfg(any(target_arch = "wasm32", test))]
fn fired_action(command: InputCommand) -> Option<String> {
    match command {
        InputCommand::Action(id) | InputCommand::ActionWithArgs { action: id, .. } => Some(id),
        _ => None,
    }
}

/// Report a keyup. Without releases a second `<space>` reads as OS
/// auto-repeat and `space space` never fires. Whether to hide the
/// overlay needs no answer here: a release that ends a held prefix
/// clears the pending sequence, and the which-key view follows it.
#[cfg(any(target_arch = "wasm32", test))]
fn key_up(keys: &InputHandle, chord: &KeyChord) {
    let _overlay_follows_pending = keys.notify_key_release(chord);
}

/// The app's shortcut processor, and the which-key view of its pending
/// sequence (recomputed whenever the handle's pending prefix changes).
fn use_keymap() -> (InputHandle, Memo<Option<WhichKeyView>>) {
    let keys = use_input_processor(|| {
        keymap_processor(true)
            .unwrap_or_else(|_| InputProcessor::new().with_macro_keys(MacroKeys::Off))
    });
    let labels = use_hook(|| Rc::new(which_key_labels()));
    let which_key = use_memo({
        let keys = keys.clone();
        move || {
            // Subscribe: the handle's mirror changes with every key.
            let _prefix = keys.pending_display();
            which_key_view(&keys.processor(), &labels)
        }
    });
    (keys, which_key)
}

// ── which-key overlay ───────────────────────────────────────────────

/// Bottom-right which-key panel, shown while a leader sequence is
/// pending — a compact column of key → action rows, lazygit-style.
/// Renders nothing when `state` is `None`.
#[component]
pub fn WhichKeyOverlay(state: Memo<Option<WhichKeyView>>) -> Element {
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
/// an `Intent`, and awaits, so it cannot run in the listener);
/// `Intent` is performed directly.
// `Exec` is only built by the wasm DOM listeners.
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
enum Msg {
    Exec(String),
    Intent(actions::Intent),
}

/// Mount the app-wide shortcut engine + the actions registry. Call once
/// from an app-long component under `AppShell` (today:
/// [`crate::palette::CommandPalette`]). Returns the which-key overlay
/// view the caller renders. Also provides [`crate::actions::ActionsCtx`]
/// via context so the palette executes through the same registry.
pub fn use_app_shortcuts() -> Memo<Option<WhichKeyView>> {
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

    let (keys, which_key) = use_keymap();

    // Single channel: DOM/palette → the effect loop. `msg_tx` is shared
    // (registry handlers enqueue intents, the DOM closure enqueues
    // Exec); a spawned task drains it inside the runtime. The
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

    // Recompile the keymap when the "prioritize app shortcuts" pref
    // changes (boot load included).
    {
        let keys = keys.clone();
        use_effect(move || {
            let prioritize = prefs.read().shortcuts_priority;
            if let Err(e) = keys.reload_config(keymap_config(prioritize)) {
                tracing::warn!(%e, "shortcut keymap reload failed");
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
            spawn(async move {
                use futures_util::StreamExt;
                while let Some(msg) = rx.next().await {
                    match msg {
                        Msg::Exec(id) => {
                            registry.execute(id).await;
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
    install_dom_listeners(keys, msg_tx);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (keys, msg_tx);

    which_key
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
/// They call the handle directly, outside the Dioxus runtime, so a match
/// can `preventDefault()` in the same tick; the handle re-enters the
/// runtime for the which-key view and the sequence timeout.
#[cfg(target_arch = "wasm32")]
fn install_dom_listeners(keys: InputHandle, msg_tx: futures_channel::mpsc::UnboundedSender<Msg>) {
    use wasm_bindgen::JsCast;
    use wasm_bindgen::prelude::Closure;

    use_hook(move || {
        let keydown = {
            let keys = keys.clone();
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                // OS auto-repeat never fires app actions.
                if e.repeat() || !typing_guard_passes(&e, &keys) {
                    return;
                }
                let Some(key) = key_event(&e) else {
                    return;
                };
                let outcome = key_down(&keys, key);
                if outcome.claims_event() {
                    e.prevent_default();
                    e.stop_propagation();
                }
                for id in outcome.actions {
                    let _ = msg_tx.unbounded_send(Msg::Exec(id));
                }
            })
        };

        let keyup =
            Closure::<dyn FnMut(web_sys::KeyboardEvent)>::new(move |e: web_sys::KeyboardEvent| {
                if let Some(k) = key_event(&e) {
                    key_up(&keys, &KeyChord::new(k.key, k.modifiers));
                }
            });

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

/// Typing guard: plain keys belong to the focused text control; only
/// Ctrl/Cmd chords are shortcuts there. Exception: the note editor in vim
/// NORMAL mode may start a leader sequence with `<space>` (and continue a
/// pending one); plain letters stay vim motions, so `g`/`gg` never leave
/// the editor.
#[cfg(target_arch = "wasm32")]
fn typing_guard_passes(e: &web_sys::KeyboardEvent, keys: &InputHandle) -> bool {
    if !text_entry_focused() || e.ctrl_key() || e.meta_key() {
        return true;
    }
    editor_vim_normal()
        && editor_root_focused()
        && (e.key() == " " || keys.processor().needs_timeout())
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
    use dioxus::core::VirtualDom;

    use super::*;

    thread_local! {
        static MOUNTED: RefCell<Option<(InputHandle, Memo<Option<WhichKeyView>>)>> =
            const { RefCell::new(None) };
    }

    #[allow(non_snake_case)]
    fn Keys() -> Element {
        let mounted = use_keymap();
        use_hook(move || MOUNTED.with(|slot| *slot.borrow_mut() = Some(mounted)));
        VNode::empty()
    }

    /// The app's shortcut engine, mounted and then driven from outside
    /// the Dioxus runtime, the way the capture-phase listeners drive it.
    struct Engine {
        dom: VirtualDom,
        keys: InputHandle,
        which_key: Memo<Option<WhichKeyView>>,
    }

    /// What typing a sequence left behind.
    struct Typed {
        /// Every action fired, across all of the sequence's keydowns.
        actions: Vec<String>,
        /// The last keydown claimed the event.
        claims: bool,
        /// The overlay's view after the last keyup.
        which_key: Option<WhichKeyView>,
    }

    fn engine(prioritize_app: bool) -> Engine {
        let mut dom = VirtualDom::new(Keys);
        dom.rebuild_in_place();
        let (keys, which_key) = MOUNTED
            .with(|slot| slot.borrow_mut().take())
            .expect("mounted");
        // As the "prioritize app shortcuts" pref effect does.
        keys.reload_config(keymap_config(prioritize_app))
            .expect("keymap compiles");
        Engine {
            dom,
            keys,
            which_key,
        }
    }

    /// Press then release every chord of `seq` the way the DOM does.
    fn type_seq(engine: &Engine, seq: &str) -> Typed {
        let mut actions = Vec::new();
        let mut claims = false;
        for token in seq.split(' ') {
            let chord = parse_key_chord(token).expect("chord parses");
            let key = KeyEvent {
                key: chord.key.clone(),
                modifiers: chord.modifiers,
            };
            let press = key_down(&engine.keys, key);
            claims = press.claims_event();
            actions.extend(press.actions);
            key_up(&engine.keys, &chord);
        }
        let which_key = engine.dom.in_runtime(|| engine.which_key.read().clone());
        Typed {
            actions,
            claims,
            which_key,
        }
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
        assert!(keymap_processor(true).is_ok());
        assert!(keymap_processor(false).is_ok());
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
            let got = type_seq(&engine(true), seq);
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
        let keys = engine(false);
        let got = type_seq(&keys, "ctrl+p");
        assert!(got.actions.is_empty());
        assert!(!got.claims);
        assert_eq!(
            type_seq(&keys, "ctrl+\\").actions,
            vec![actions::TOGGLE_SIDEBAR.to_owned()]
        );
    }

    #[test]
    fn a_leader_prefix_claims_the_key_and_shows_which_key() {
        let got = type_seq(&engine(true), "space");
        assert!(got.actions.is_empty());
        assert!(got.claims, "space must not scroll mid-leader");
        assert_eq!(got.which_key.expect("pending").prefix, "SPC");
    }

    #[test]
    fn space_space_searches_everything() {
        assert_eq!(
            type_seq(&engine(true), "space space").actions,
            vec![actions::SEARCH_ALL.to_owned()]
        );
    }

    #[test]
    fn space_space_with_keyups_fires_once_and_clears_the_overlay() {
        let keys = engine(true);
        let leader = type_seq(&keys, "space");
        assert!(leader.claims);
        assert_eq!(leader.which_key.expect("leader pending").prefix, "SPC");
        let got = type_seq(&keys, "space");
        assert_eq!(got.actions, vec![actions::SEARCH_ALL.to_owned()]);
        assert!(got.which_key.is_none(), "nothing pending after the leaf");
        assert!(keys.keys.processor().sequence_timeout().is_none());
    }

    #[test]
    fn macro_keys_are_ordinary_keys() {
        let keys = engine(true);
        let q = type_seq(&keys, "q");
        assert!(q.actions.is_empty());
        assert!(!q.claims, "an unbound `q` is left to the browser");
        assert!(!keys.keys.is_recording());
        assert_eq!(
            type_seq(&keys, "g h").actions,
            vec![actions::NAV_HOME.to_owned()],
            "`g` after `q` is a key, not a register name"
        );
    }

    #[test]
    fn a_miss_ends_the_sequence() {
        let keys = engine(true);
        let got = type_seq(&keys, "g x");
        assert!(got.actions.is_empty());
        assert!(got.which_key.is_none());
        assert_eq!(
            type_seq(&keys, "g h").actions,
            vec![actions::NAV_HOME.to_owned()]
        );
    }

    #[test]
    fn whichkey_lists_leader_group() {
        let view = type_seq(&engine(true), "space")
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
        let view = type_seq(&engine(true), "space g")
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
