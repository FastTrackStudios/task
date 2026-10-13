//! `/settings` — the settings window.
//!
//! One list of panels — Account, Appearance, General, Sections,
//! Keyboard — drawn the way iOS and Linear draw settings: on a phone, an
//! account card and grouped rows, each opening its panel with a back
//! button; from `md` up, the panels down a sidebar beside the open one.
//!
//! Per-user choices write through `PrefsCtx::update` (the server-side
//! `UserPrefs` entity, so they follow the account to any device); what
//! is kept per device says so where it is set.

use architect_ui::lucide_dioxus::{
    ChevronLeft, ChevronRight, Keyboard, LayoutGrid, Palette, SlidersHorizontal, UserRound,
};
use dioxus::prelude::*;
use task_ui_core::sheet::{SheetGroup, SheetSection, SheetSelect, SheetToggle};

use crate::auth::{AccountSheetBody, AuthCtx, Avatar};
use crate::prefs::PrefsCtx;
use crate::shortcuts::{BINDINGS, Binding, effective_sequence};
use crate::theming::use_org_theme_switcher_state;

/// The routes worth landing on. `""` = the app default.
const PAGE_CHOICES: &[(&str, &str)] = &[
    ("", "Inbox (default)"),
    ("/tasks", "Tasks"),
    ("/projects", "Projects"),
    ("/schedule", "Schedule"),
    ("/vault", "Vault"),
];

/// The settings panels, in the order they are listed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Panel {
    Account,
    Appearance,
    General,
    Sections,
    Keyboard,
}

impl Panel {
    const ALL: [Self; 5] = [
        Self::Account,
        Self::Appearance,
        Self::General,
        Self::Sections,
        Self::Keyboard,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Account => "Account",
            Self::Appearance => "Appearance",
            Self::General => "General",
            Self::Sections => "Sections",
            Self::Keyboard => "Keyboard",
        }
    }

    fn icon(self) -> Element {
        match self {
            Self::Account => rsx! { UserRound { size: 17 } },
            Self::Appearance => rsx! { Palette { size: 17 } },
            Self::General => rsx! { SlidersHorizontal { size: 17 } },
            Self::Sections => rsx! { LayoutGrid { size: 17 } },
            Self::Keyboard => rsx! { Keyboard { size: 17 } },
        }
    }

    /// The tile colour behind its icon, iOS-settings style.
    fn tint(self) -> &'static str {
        match self {
            Self::Account => "bg-sky-500",
            Self::Appearance => "bg-violet-500",
            Self::General => "bg-zinc-500",
            Self::Sections => "bg-emerald-500",
            Self::Keyboard => "bg-amber-500",
        }
    }
}

#[component]
pub fn SettingsView() -> Element {
    // Phone: `None` is the list. Desktop shows a panel always.
    let mut open = use_signal(|| None::<Panel>);
    let shown = open().unwrap_or(Panel::Appearance);

    rsx! {
        div { class: "mx-auto flex w-full max-w-5xl flex-col md:flex-row md:gap-8 md:p-8",
            // ── The list (phone: the page; desktop: the sidebar) ──────
            nav {
                class: if open().is_some() { "hidden w-full shrink-0 flex-col md:flex md:w-60" } else { "flex w-full shrink-0 flex-col md:w-60" },
                h1 { class: "px-5 pb-3 text-[28px] font-bold leading-[34px] tracking-[-0.02em] text-foreground md:px-2 md:text-xl",
                    "Settings"
                }
                div { class: "px-3 md:px-0",
                    AccountCard { on_open: move |()| open.set(Some(Panel::Account)) }
                    SheetGroup {
                        for p in Panel::ALL.into_iter().skip(1) {
                            PanelRow {
                                key: "{p.label()}",
                                panel: p,
                                active: shown == p,
                                on_open: move |p| open.set(Some(p)),
                            }
                        }
                    }
                }
            }
            // ── The open panel ────────────────────────────────────────
            section {
                class: if open().is_some() { "flex min-w-0 flex-1 flex-col" } else { "hidden min-w-0 flex-1 flex-col md:flex" },
                // Phone: back to the list, and the panel's name.
                div { class: "relative flex h-11 items-center justify-center px-3 md:hidden",
                    button {
                        r#type: "button",
                        class: "absolute left-3 flex size-11 items-center justify-center rounded-full bg-foreground/[0.07] text-foreground active:scale-95",
                        aria_label: "Back to Settings",
                        onclick: move |_| open.set(None),
                        ChevronLeft { size: 20 }
                    }
                    span { class: "text-[17px] font-semibold text-foreground", "{shown.label()}" }
                }
                h2 { class: "hidden pb-4 text-xl font-semibold text-foreground md:block", "{shown.label()}" }
                div { class: "px-3 pt-3 md:px-0 md:pt-0",
                    match shown {
                        Panel::Account => rsx! { AccountPanel {} },
                        Panel::Appearance => rsx! { AppearancePanel {} },
                        Panel::General => rsx! { GeneralPanel {} },
                        Panel::Sections => rsx! { SectionsPanel {} },
                        Panel::Keyboard => rsx! { KeyboardPanel {} },
                    }
                }
            }
        }
    }
}

/// The signed-in person at the top of the list: avatar, name, email.
#[component]
fn AccountCard(on_open: EventHandler<()>) -> Element {
    let account = use_context::<AuthCtx>().active.read().clone();
    let (name, email) = account.as_ref().map_or_else(
        || ("Not signed in".to_owned(), String::new()),
        |a| (a.name.clone(), a.email.clone()),
    );
    rsx! {
        button {
            r#type: "button",
            class: "mb-3 flex w-full items-center gap-3.5 rounded-[20px] bg-foreground/[0.05] px-4 py-3.5 text-left active:bg-foreground/[0.08]",
            onclick: move |_| on_open.call(()),
            Avatar { name: name.clone(), email: email.clone(), size: 48 }
            div { class: "flex min-w-0 flex-1 flex-col",
                span { class: "truncate text-[17px] font-semibold text-foreground", "{name}" }
                span { class: "truncate text-[13px] text-muted-foreground", "{email}" }
            }
            span { class: "text-muted-foreground/60", ChevronRight { size: 18 } }
        }
    }
}

/// One panel in the list: tinted icon tile, name, chevron.
#[component]
fn PanelRow(panel: Panel, active: bool, on_open: EventHandler<Panel>) -> Element {
    let tint = panel.tint();
    let row = if active {
        "flex min-h-[52px] w-full items-center gap-3.5 px-4 text-left text-[16px] text-foreground active:bg-foreground/[0.06] md:bg-foreground/[0.06]"
    } else {
        "flex min-h-[52px] w-full items-center gap-3.5 px-4 text-left text-[16px] text-foreground active:bg-foreground/[0.06]"
    };
    rsx! {
        button { r#type: "button", class: row, onclick: move |_| on_open.call(panel),
            span { class: "flex size-7 shrink-0 items-center justify-center rounded-lg text-white {tint}",
                {panel.icon()}
            }
            span { class: "min-w-0 flex-1 truncate", "{panel.label()}" }
            span { class: "text-muted-foreground/60 md:hidden", ChevronRight { size: 18 } }
        }
    }
}

#[component]
fn AccountPanel() -> Element {
    rsx! {
        // Status, other accounts, sign out — the account sheet's rows.
        AccountSheetBody { on_done: move |()| {} }
        // Linked accounts live with the account, on the issuer: one
        // place to link GitHub and Google for every app.
        if let Some(issuer) = task_ui_core::central_auth::issuer() {
            SheetSection { label: "Linked accounts".to_owned() }
            SheetGroup {
                a {
                    class: "flex min-h-[52px] items-center justify-between px-4 text-[16px] text-foreground active:bg-foreground/[0.06]",
                    href: "{issuer.trim_end_matches('/')}/account",
                    target: "_blank",
                    rel: "noopener",
                    "GitHub and Google"
                    span { class: "text-muted-foreground", "Manage ↗" }
                }
            }
            p { class: "px-2 text-[13px] text-muted-foreground",
                "Changes you accept on a repository-backed wiki are pushed and proposed as your GitHub account."
            }
        }
    }
}

#[component]
fn AppearancePanel() -> Element {
    // Same active-org ↔ overrides bridge as the shell's theme pickers;
    // persistence rides the App-root `use_theme_prefs_sync` effect.
    let theme_state = use_org_theme_switcher_state();
    rsx! {
        p { class: "px-2 pb-3 text-[13px] text-muted-foreground",
            "Theme and mode follow your account on every device. Text size, density, corners, contrast and font are kept per device."
        }
        div { class: "rounded-[20px] bg-foreground/[0.05] p-4",
            crate::appearance::AppearancePanel { state: theme_state, class: "max-w-md" }
        }
    }
}

#[component]
fn GeneralPanel() -> Element {
    let prefs_ctx = use_context::<PrefsCtx>();
    let prefs = prefs_ctx.prefs.read().clone();
    rsx! {
        SheetSection { label: "Opening the app".to_owned() }
        SheetGroup {
            SheetSelect {
                label: "Start page",
                value: prefs.default_page.clone(),
                options: PAGE_CHOICES
                    .iter()
                    .map(|(v, l)| ((*v).to_owned(), (*l).to_owned()))
                    .collect::<Vec<_>>(),
                on_change: move |v: String| prefs_ctx.update(|p| p.default_page = v.clone()),
            }
        }
        SheetSection { label: "Tasks board".to_owned() }
        SheetGroup {
            SheetToggle {
                label: "Show completed",
                on: !prefs.tasks_active,
                on_toggle: move |on: bool| prefs_ctx.update(|p| p.tasks_active = !on),
            }
            SheetToggle {
                label: "Only what's relevant now",
                hint: "Routines in their windows, deadlines always",
                on: prefs.tasks_relevant,
                on_toggle: move |on| prefs_ctx.update(|p| p.tasks_relevant = on),
            }
        }
        p { class: "px-2 text-[13px] text-muted-foreground",
            "Follows your account on every device. The board's Display options change these too."
        }
    }
}

#[component]
fn SectionsPanel() -> Element {
    let layout = crate::nav_layout::use_nav_layout();
    let mut customize = use_signal(|| false);
    let every = crate::nav::nav_tabs_for(&crate::nav::use_active_plugins());
    let (shown, hidden) = layout.read().arrange(every.clone());
    rsx! {
        p { class: "px-2 pb-3 text-[13px] text-muted-foreground",
            "The order of the section menu on this device. The first three are the phone's tab bar."
        }
        SheetGroup {
            for (i , t) in shown.iter().enumerate() {
                div {
                    key: "{t.label}",
                    class: "flex min-h-[48px] items-center gap-3.5 px-4 text-[16px] text-foreground",
                    span { class: "w-5 text-right text-[13px] tabular-nums text-muted-foreground", "{i + 1}" }
                    span { class: "flex size-5 items-center justify-center [&_svg]:size-[18px]", {(t.icon)()} }
                    span { class: "min-w-0 flex-1 truncate", "{t.label}" }
                    if i < crate::nav_layout::PRIMARY {
                        span { class: "rounded-full bg-primary/15 px-2 py-0.5 text-[11px] font-medium text-primary", "Tab bar" }
                    }
                }
            }
        }
        if !hidden.is_empty() {
            p { class: "px-2 pb-3 text-[13px] text-muted-foreground",
                "{hidden.len()} hidden"
            }
        }
        button {
            r#type: "button",
            class: "flex h-12 w-full items-center justify-center rounded-full bg-foreground/[0.07] text-[15px] font-semibold text-foreground active:scale-[0.98]",
            onclick: move |_| customize.set(true),
            "Customize…"
        }
        crate::nav_layout::CustomizeSheet {
            open: customize(),
            on_close: move |()| customize.set(false),
            tabs: every,
            layout,
        }
    }
}

#[component]
fn KeyboardPanel() -> Element {
    let prefs_ctx = use_context::<PrefsCtx>();
    let prefs = prefs_ctx.prefs.read().clone();
    rsx! {
        p { class: "px-2 pb-3 text-[13px] text-muted-foreground",
            "App-wide shortcuts. One keymap everywhere; combos the browser refuses to yield (Ctrl+N/T/W) are remapped on web."
        }
        SheetGroup {
            SheetToggle {
                label: "Prioritize app shortcuts",
                hint: "Win over browser defaults like Ctrl+P",
                on: prefs.shortcuts_priority,
                on_toggle: move |on| prefs_ctx.update(|p| p.shortcuts_priority = on),
            }
            SheetToggle {
                label: "Vim keys in the note editor",
                hint: "Notes open in NORMAL mode",
                on: prefs.vim_mode,
                on_toggle: move |on| prefs_ctx.update(|p| p.vim_mode = on),
            }
        }
        SheetSection { label: "Shortcuts".to_owned() }
        div { class: "overflow-x-auto rounded-[20px] bg-foreground/[0.05]",
            table { class: "w-full min-w-[28rem] border-collapse text-sm",
                tbody {
                    for def in BINDINGS {
                        ShortcutRow {
                            key: "{def.action}-{def.seq}",
                            def,
                            priority_on: prefs.shortcuts_priority,
                        }
                    }
                }
            }
        }
    }
}

/// One read-only keymap row: the effective chord(s) as keycaps, the
/// action description, and provenance notes (web remap of a reserved
/// combo / overrides a browser default / inactive while the priority
/// toggle is off).
#[component]
fn ShortcutRow(def: &'static Binding, priority_on: bool) -> Element {
    let inactive = def.overrides_browser && !priority_on;
    let seq = effective_sequence(def);
    rsx! {
        tr {
            class: if inactive {
                "border-b border-foreground/[0.06] text-muted-foreground/60 last:border-0"
            } else {
                "border-b border-foreground/[0.06] last:border-0"
            },
            td { class: "whitespace-nowrap px-4 py-2.5",
                for (i, chord) in seq.split(' ').enumerate() {
                    if i > 0 {
                        span { class: "px-1 text-xs text-muted-foreground", "then" }
                    }
                    kbd { class: "rounded-md bg-foreground/[0.08] px-1.5 py-0.5 font-mono text-xs uppercase",
                        "{chord}"
                    }
                }
            }
            td { class: "px-3 py-2.5", "{def.label}" }
            td { class: "px-3 py-2.5 text-xs text-muted-foreground",
                if def.web.is_some() {
                    span { class: "mr-2", "web remap of {def.seq}" }
                }
                if def.overrides_browser {
                    if inactive {
                        span { "inactive — overrides a browser shortcut" }
                    } else {
                        span { "overrides a browser shortcut" }
                    }
                }
            }
        }
    }
}
