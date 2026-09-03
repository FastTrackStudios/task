//! Ctrl+P command palette + Ctrl+O note omni-picker.
//!
//! [`CommandPalette`] is the one app-long overlay host: it mounts the
//! shared shortcut engine ([`crate::shortcuts::use_app_shortcuts`]) and
//! renders the which-key overlay, the note omni-picker, and the command
//! palette modal. All three are toggled by context signals the app shell
//! provides ([`PaletteOpen`] / [`OmniOpen`], same shape as
//! `chrome::FleetingOpen`).
//!
//! The palette mixes three nucleo-ranked sections:
//!
//! - **Commands** — every action from [`crate::actions::task_action_defs`]
//!   (create note, start timer, go to …). Selecting one executes it
//!   through the shared [`actions_standalone::StandaloneActions`]
//!   registry — the exact path a keyboard shortcut takes.
//! - **Pages** — every app destination from [`crate::nav::nav_tabs`].
//! - **Notes** — every vault note from the active org's folder index
//!   (the org the switcher is scoped to, else the home org).
//!
//! The omni-picker ([`OmniPicker`], Ctrl+O) is the Notes section on its
//! own: uncapped, fuzzy over the whole vault.

use architect_ui::lucide_dioxus::{FileText, Search};
use dioxus::prelude::*;
use nucleo_matcher::{
    Matcher, Utf32Str,
    pattern::{CaseMatching, Normalization, Pattern},
};

use crate::actions::{ActionsCtx, task_action_defs};
use crate::nav::nav_tabs_for;
use crate::orgs::OrgMeta;
use crate::routes::Route;

// ── shared context ──────────────────────────────────────────────────

/// Visibility of the global command palette. Provided once by the app
/// shell; flipped by the Ctrl/Cmd+P hotkey.
#[derive(Clone, Copy)]
pub struct PaletteOpen(pub Signal<bool>);

/// Visibility of the note omni-picker. Provided once by the app shell;
/// opened by the `fts.picker.notes` action (Ctrl+O).
#[derive(Clone, Copy)]
pub struct OmniOpen(pub Signal<bool>);

/// Install the palette + omni contexts. Call once in the app shell.
pub fn provide_palette_context() {
    use_context_provider(|| PaletteOpen(Signal::new(false)));
    use_context_provider(|| OmniOpen(Signal::new(false)));
}

pub(crate) fn use_palette_open() -> Signal<bool> {
    use_context::<PaletteOpen>().0
}

// ── data ────────────────────────────────────────────────────────────

/// Rows rendered at once in the palette — keeps ranking + render cheap.
const MAX_RESULTS: usize = 14;

/// What selecting a palette row does.
#[derive(Clone, PartialEq)]
enum Target {
    /// Navigate to a route (pages + notes).
    Route(Route),
    /// Execute a registry action by id.
    Action(String),
}

/// Which section a row belongs to — drives display grouping + order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Command,
    Page,
    Note,
}

/// One selectable row.
#[derive(Clone, PartialEq)]
#[allow(unpredictable_function_pointer_comparisons)]
struct PaletteEntry {
    label: String,
    /// Secondary match/display text — the vault-relative path for
    /// notes, the shortcut hint for commands, empty for pages.
    detail: String,
    target: Target,
    /// Nav-tab icon for pages; commands + notes render a glyph.
    icon: Option<fn() -> Element>,
    kind: Kind,
}

/// Rank `entries` against `query`, keep the top [`MAX_RESULTS`], and
/// group by [`Kind`] (Commands, Pages, Notes) preserving rank within
/// each group.
fn rank(query: &str, entries: Vec<PaletteEntry>) -> Vec<PaletteEntry> {
    let mut survivors: Vec<PaletteEntry> = if query.is_empty() {
        entries.into_iter().take(MAX_RESULTS).collect()
    } else {
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, PaletteEntry)> = entries
            .into_iter()
            .filter_map(|e| {
                let hay = if e.detail.is_empty() {
                    e.label.clone()
                } else {
                    format!("{} {}", e.label, e.detail)
                };
                let score = pattern.score(Utf32Str::new(&hay, &mut buf), &mut matcher)?;
                Some((score, e))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.truncate(MAX_RESULTS);
        scored.into_iter().map(|(_, e)| e).collect()
    };
    // Stable group by kind (Command < Page < Note), rank order kept
    // within each group.
    survivors.sort_by_key(|e| e.kind);
    survivors
}

// ── overlay host ────────────────────────────────────────────────────

/// The app-long overlay host: mounts the shortcut engine and renders
/// the which-key overlay, the note omni-picker, and the palette modal.
/// Render once (in the app shell).
#[component]
pub fn CommandPalette() -> Element {
    // The app-wide shortcut engine (Ctrl+P/Ctrl+O, leader keys, `g …`
    // nav) + the actions registry mount here; it returns the which-key
    // overlay signal.
    let whichkey = crate::shortcuts::use_app_shortcuts();
    rsx! {
        crate::shortcuts::WhichKeyOverlay { state: whichkey }
        crate::search::SearchOverlay {}
        OmniPicker {}
        PaletteModal {}
    }
}

/// The command-palette modal: Commands + Pages + Notes, fuzzy-ranked.
/// Centered top-third modal over a scrim; type to filter, Up/Down +
/// Enter (or click) to run, Escape / click outside closes.
#[component]
fn PaletteModal() -> Element {
    let mut open = use_palette_open();
    let actions = use_context::<ActionsCtx>();

    let nav = use_navigator();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let slugs = use_memo(move || crate::orgs::selected_slugs(&selection.read(), &org_list.read()));

    let mut query = use_signal(String::new);
    let mut cursor = use_signal(|| 0usize);

    let notes = use_resource(move || {
        let want = open();
        let slugs = slugs();
        async move {
            if !want {
                return Vec::new();
            }
            // Fan out across the selected orgs (every hosted org under
            // "All") so search spans all vaults; tag each hit with its
            // org so opening routes to the right one.
            let mut out = Vec::new();
            for slug in slugs {
                if let Ok(pages) = crate::pages::vault::fetch_folder_index(
                    slug.clone(),
                    crate::document_session::VAULT_ID.to_owned(),
                )
                .await
                {
                    for p in pages {
                        out.push((p, slug.clone()));
                    }
                }
            }
            out
        }
    });

    if !open() {
        return rsx! {};
    }

    let mut close = move || {
        open.set(false);
        query.set(String::new());
        cursor.set(0);
    };

    // Active org's enabled plugins — disabled plugins' pages don't
    // list. Signal reads (not hooks), so this is fine after the early
    // return above.
    let plugins = crate::orgs::active_plugin_set(&selection.read(), &org_list.read());

    // Commands (registry) + Pages (nav set) + Notes (vault index).
    let mut entries: Vec<PaletteEntry> = task_action_defs()
        .into_iter()
        .map(|def| PaletteEntry {
            label: def.name.clone(),
            detail: def.shortcut_hint.clone().unwrap_or_default(),
            target: Target::Action(def.id.as_str().to_string()),
            icon: None,
            kind: Kind::Command,
        })
        .collect();
    for tab in nav_tabs_for(&plugins) {
        entries.push(PaletteEntry {
            label: tab.label.to_string(),
            detail: String::new(),
            target: Target::Route(tab.route),
            icon: Some(tab.icon),
            kind: Kind::Page,
        });
    }
    if let Some(pages) = &*notes.read() {
        for (meta, org) in pages {
            let label = if meta.title.is_empty() {
                meta.basename.clone()
            } else {
                meta.title.clone()
            };
            entries.push(PaletteEntry {
                label,
                detail: meta.path.clone(),
                target: Target::Route(Route::VaultRoute {
                    path: meta.path.clone(),
                    org: org.clone(),
                }),
                icon: None,
                kind: Kind::Note,
            });
        }
    }
    let ranked = rank(&query(), entries);
    let count = ranked.len();
    let sel = cursor().min(count.saturating_sub(1));

    let go = {
        let ranked = ranked.clone();
        let registry = actions.registry.clone();
        move |i: usize| {
            if let Some(entry) = ranked.get(i) {
                match &entry.target {
                    Target::Route(r) => {
                        nav.push(r.clone());
                    }
                    Target::Action(id) => {
                        let registry = registry.clone();
                        let id = id.clone();
                        spawn(async move {
                            registry.execute(id).await;
                        });
                    }
                }
            }
            close();
        }
    };
    let mut go_key = go.clone();

    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-start justify-center bg-black/40 p-4 pt-[12vh]",
            onclick: move |_| close(),
            div {
                class: "flex w-full max-w-xl flex-col overflow-hidden rounded-xl border border-border bg-background shadow-xl",
                onclick: move |e| e.stop_propagation(),
                div { class: "flex items-center gap-2 border-b border-border px-3",
                    span { class: "text-muted-foreground", Search { size: 15 } }
                    input {
                        class: "h-11 w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground",
                        placeholder: "Run a command, jump to a page or note…",
                        onmounted: move |e: Event<MountedData>| {
                            spawn(async move {
                                let _ = e.data().set_focus(true).await;
                                architect::sleep(architect::Duration::from_millis(80)).await;
                                let _ = e.data().set_focus(true).await;
                            });
                        },
                        value: "{query}",
                        oninput: move |e| {
                            query.set(e.value());
                            cursor.set(0);
                        },
                        onkeydown: move |e| match e.key() {
                            Key::ArrowDown => {
                                e.prevent_default();
                                if count > 0 {
                                    cursor.set((sel + 1).min(count - 1));
                                }
                            }
                            Key::ArrowUp => {
                                e.prevent_default();
                                cursor.set(sel.saturating_sub(1));
                            }
                            Key::Enter => {
                                e.prevent_default();
                                go_key(sel);
                            }
                            Key::Escape => close(),
                            _ => {}
                        },
                    }
                    span { class: "shrink-0 text-xs text-muted-foreground", "↵ to run · esc to close" }
                }
                div { class: "max-h-[50vh] overflow-y-auto py-1",
                    if count == 0 {
                        div { class: "px-3 py-4 text-center text-sm text-muted-foreground", "No matches." }
                    }
                    {render_sections(&ranked, sel, cursor, go.clone())}
                }
            }
        }
    }
}

/// Render the ranked rows split into their [`Kind`] sections. `ranked`
/// is already grouped by [`Kind`], so a row starts a new section iff its
/// kind differs from the previous row's.
fn render_sections(
    ranked: &[PaletteEntry],
    sel: usize,
    mut cursor: Signal<usize>,
    go: impl FnMut(usize) + Clone + 'static,
) -> Element {
    rsx! {
        for (i, entry) in ranked.iter().enumerate() {
            if i == 0 || ranked[i - 1].kind != entry.kind {
                SectionLabel { text: section_label(entry.kind) }
            }
            PaletteRow {
                entry: entry.clone(),
                selected: i == sel,
                on_hover: move |_| cursor.set(i),
                on_pick: {
                    let mut go = go.clone();
                    move |_| go(i)
                },
            }
        }
    }
}

fn section_label(kind: Kind) -> &'static str {
    match kind {
        Kind::Command => "Commands",
        Kind::Page => "Pages",
        Kind::Note => "Notes",
    }
}

// ── omni-picker ─────────────────────────────────────────────────────

/// The Ctrl+O note omni-picker: the Notes section on its own, fuzzy over
/// the whole vault (uncapped, scrollable). Enter opens the note.
#[component]
pub fn OmniPicker() -> Element {
    let mut open = use_context::<OmniOpen>().0;
    let nav = use_navigator();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let slugs = use_memo(move || crate::orgs::selected_slugs(&selection.read(), &org_list.read()));

    let mut query = use_signal(String::new);
    let mut cursor = use_signal(|| 0usize);

    let notes = use_resource(move || {
        let want = open();
        let slugs = slugs();
        async move {
            if !want {
                return Vec::new();
            }
            // Fan out across the selected orgs (every hosted org under
            // "All") so search spans all vaults; tag each hit with its
            // org so opening routes to the right one.
            let mut out = Vec::new();
            for slug in slugs {
                if let Ok(pages) = crate::pages::vault::fetch_folder_index(
                    slug.clone(),
                    crate::document_session::VAULT_ID.to_owned(),
                )
                .await
                {
                    for p in pages {
                        out.push((p, slug.clone()));
                    }
                }
            }
            out
        }
    });

    if !open() {
        return rsx! {};
    }

    let mut close = move || {
        open.set(false);
        query.set(String::new());
        cursor.set(0);
    };

    // All notes, fuzzy-ranked (title + path), uncapped.
    let mut entries: Vec<(String, String, String)> = Vec::new();
    if let Some(pages) = &*notes.read() {
        for (meta, org) in pages {
            let label = if meta.title.is_empty() {
                meta.basename.clone()
            } else {
                meta.title.clone()
            };
            entries.push((label, meta.path.clone(), org.clone()));
        }
    }
    let ranked: Vec<(String, String, String)> = if query().is_empty() {
        entries
    } else {
        let mut matcher = Matcher::new(nucleo_matcher::Config::DEFAULT);
        let pattern = Pattern::parse(&query(), CaseMatching::Ignore, Normalization::Smart);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, (String, String, String))> = entries
            .into_iter()
            .filter_map(|(label, path, org)| {
                let hay = format!("{label} {path}");
                let score = pattern.score(Utf32Str::new(&hay, &mut buf), &mut matcher)?;
                Some((score, (label, path, org)))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0));
        scored.into_iter().map(|(_, e)| e).collect()
    };
    let count = ranked.len();
    let sel = cursor().min(count.saturating_sub(1));

    let go = {
        let ranked = ranked.clone();
        move |i: usize| {
            if let Some((_, path, org)) = ranked.get(i) {
                nav.push(Route::VaultRoute {
                    path: path.clone(),
                    org: org.clone(),
                });
            }
            close();
        }
    };
    let mut go_key = go.clone();

    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-start justify-center bg-black/40 p-4 pt-[12vh]",
            onclick: move |_| close(),
            div {
                class: "flex w-full max-w-xl flex-col overflow-hidden rounded-xl border border-border bg-background shadow-xl",
                onclick: move |e| e.stop_propagation(),
                div { class: "flex items-center gap-2 border-b border-border px-3",
                    span { class: "text-muted-foreground", FileText { size: 15 } }
                    input {
                        class: "h-11 w-full bg-transparent text-sm outline-none placeholder:text-muted-foreground",
                        placeholder: "Open a note…",
                        onmounted: move |e: Event<MountedData>| {
                            spawn(async move {
                                let _ = e.data().set_focus(true).await;
                                architect::sleep(architect::Duration::from_millis(80)).await;
                                let _ = e.data().set_focus(true).await;
                            });
                        },
                        value: "{query}",
                        oninput: move |e| {
                            query.set(e.value());
                            cursor.set(0);
                        },
                        onkeydown: move |e| match e.key() {
                            Key::ArrowDown => {
                                e.prevent_default();
                                if count > 0 {
                                    cursor.set((sel + 1).min(count - 1));
                                }
                            }
                            Key::ArrowUp => {
                                e.prevent_default();
                                cursor.set(sel.saturating_sub(1));
                            }
                            Key::Enter => {
                                e.prevent_default();
                                go_key(sel);
                            }
                            Key::Escape => close(),
                            _ => {}
                        },
                    }
                    span { class: "shrink-0 text-xs text-muted-foreground", "↵ to open · esc to close" }
                }
                div { class: "max-h-[60vh] overflow-y-auto py-1",
                    if count == 0 {
                        div { class: "px-3 py-4 text-center text-sm text-muted-foreground", "No notes." }
                    }
                    for (i, (label, path, _org)) in ranked.iter().enumerate() {
                        button {
                            key: "{path}",
                            r#type: "button",
                            class: if i == sel {
                                "flex w-full items-center gap-2.5 bg-accent/60 px-3 py-2 text-left text-sm text-foreground"
                            } else {
                                "flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm text-muted-foreground hover:text-foreground"
                            },
                            onmouseenter: move |_| cursor.set(i),
                            onclick: {
                                let mut go = go.clone();
                                move |_| go(i)
                            },
                            span { class: "flex h-4 w-4 shrink-0 items-center justify-center", FileText { size: 15 } }
                            span { class: "truncate", "{label}" }
                            span { class: "ml-auto max-w-[40%] truncate text-xs text-muted-foreground/70", "{path}" }
                        }
                    }
                }
            }
        }
    }
}

// ── rows ────────────────────────────────────────────────────────────

#[component]
fn SectionLabel(text: &'static str) -> Element {
    rsx! {
        div { class: "px-3 pb-1 pt-2 text-xs font-medium uppercase tracking-wide text-muted-foreground",
            "{text}"
        }
    }
}

/// One result row: icon/glyph + label (+ muted detail). Hover moves the
/// keyboard cursor so mouse and arrows never fight.
#[component]
fn PaletteRow(
    entry: PaletteEntry,
    selected: bool,
    on_hover: EventHandler<()>,
    on_pick: EventHandler<()>,
) -> Element {
    let icon = entry.icon;
    let kind = entry.kind;
    rsx! {
        button {
            r#type: "button",
            class: if selected {
                "flex w-full items-center gap-2.5 bg-accent/60 px-3 py-2 text-left text-sm text-foreground"
            } else {
                "flex w-full items-center gap-2.5 px-3 py-2 text-left text-sm text-muted-foreground hover:text-foreground"
            },
            onmouseenter: move |_| on_hover.call(()),
            onclick: move |_| on_pick.call(()),
            span { class: "flex h-4 w-4 shrink-0 items-center justify-center",
                if let Some(icon) = icon {
                    {icon()}
                } else if kind == Kind::Command {
                    span { class: "font-mono text-xs text-muted-foreground", "›" }
                } else {
                    FileText { size: 15 }
                }
            }
            span { class: "truncate", "{entry.label}" }
            if !entry.detail.is_empty() {
                span { class: "ml-auto max-w-[45%] shrink-0 truncate text-xs text-muted-foreground/70",
                    "{entry.detail}"
                }
            }
        }
    }
}
