//! The vault's own bottom bar on a phone — Obsidian's: back, forward,
//! search, a new note, the open tabs (a count you tap to switch), and ☰
//! for everything else — a row just above the app's tab bar, which stays,
//! so writing has its own controls in reach.

use architect_ui::lucide_dioxus::{ChevronLeft, ChevronRight, Menu, Plus, Search, X};
use dioxus::prelude::*;
use task_ui_core::sheet::BottomSheet;

use crate::nav::{nav_tabs_for, tabs_match, use_active_plugins};
use crate::routes::Route;

/// One open tab, as the switcher lists it.
#[derive(Clone, PartialEq)]
pub(crate) struct BarTab {
    pub(crate) title: String,
    pub(crate) active: bool,
}

#[component]
pub(crate) fn VaultBar(
    tabs: Vec<BarTab>,
    on_search: EventHandler<()>,
    on_new: EventHandler<()>,
    on_focus: EventHandler<usize>,
    on_close: EventHandler<usize>,
) -> Element {
    let nav = use_navigator();
    let mut tabs_open = use_signal(|| false);
    let mut menu_open = use_signal(|| false);
    let route = use_route::<Route>();
    let sections = nav_tabs_for(&use_active_plugins());
    let n = tabs.len();
    let btn = "flex h-10 flex-1 items-center justify-center rounded-full text-foreground/85 active:bg-accent/60 disabled:opacity-30";

    rsx! {
        nav {
            "data-testid": "vault-bar",
            // Just above the app's tab bar (3.375rem + its 0.5rem gap and
            // the safe-area padding the tab bar uses).
            class: "fixed inset-x-0 z-30 flex px-7 md:hidden [.kb-open_&]:hidden",
            style: "bottom: calc(3.375rem + 0.5rem + max(0.75rem, calc(env(safe-area-inset-bottom, 0px) - 4px)));",
            div { class: "flex h-11 flex-1 items-center gap-0.5 rounded-full border border-border/70 bg-popover/95 p-0.5 shadow-[0_8px_30px_rgb(0_0_0/0.35)] backdrop-blur",
                button {
                    r#type: "button",
                    class: btn,
                    aria_label: "Back",
                    disabled: !nav.can_go_back(),
                    onclick: move |_| nav.go_back(),
                    ChevronLeft { size: 22 }
                }
                button {
                    r#type: "button",
                    class: btn,
                    aria_label: "Forward",
                    disabled: !nav.can_go_forward(),
                    onclick: move |_| nav.go_forward(),
                    ChevronRight { size: 22 }
                }
                button {
                    r#type: "button",
                    class: btn,
                    aria_label: "Search notes and wikis",
                    onclick: move |_| on_search.call(()),
                    Search { size: 21 }
                }
                button {
                    r#type: "button",
                    class: btn,
                    "data-testid": "vault-bar-new",
                    aria_label: "New note",
                    onclick: move |_| on_new.call(()),
                    Plus { size: 23 }
                }
                button {
                    r#type: "button",
                    class: btn,
                    "data-testid": "vault-bar-tabs",
                    aria_label: "Open tabs: {n}",
                    onclick: move |_| tabs_open.set(true),
                    span { class: "flex h-[22px] min-w-[26px] items-center justify-center rounded-[6px] border-[1.75px] border-current px-1 text-[12px] font-semibold tabular-nums",
                        "{n}"
                    }
                }
                button {
                    r#type: "button",
                    class: btn,
                    aria_label: "More",
                    onclick: move |_| menu_open.set(true),
                    Menu { size: 22 }
                }
            }
        }
        BottomSheet {
            open: tabs_open(),
            title: "Tabs".to_string(),
            on_close: move |()| tabs_open.set(false),
            div { class: "flex flex-col pb-2",
                if tabs.is_empty() {
                    p { class: "px-4 py-6 text-center text-[15px] text-muted-foreground", "No notes open." }
                }
                for (i, t) in tabs.iter().cloned().enumerate() {
                    div { key: "{i}", class: if t.active { "flex min-h-12 items-center gap-2 rounded-xl bg-accent px-3" } else { "flex min-h-12 items-center gap-2 rounded-xl px-3 active:bg-accent/60" },
                        button {
                            r#type: "button",
                            class: "min-w-0 flex-1 truncate text-left text-[16px] text-foreground",
                            onclick: move |_| {
                                on_focus.call(i);
                                tabs_open.set(false);
                            },
                            "{t.title}"
                        }
                        button {
                            r#type: "button",
                            class: "flex size-9 shrink-0 items-center justify-center rounded-full text-muted-foreground active:bg-foreground/10",
                            aria_label: "Close {t.title}",
                            onclick: move |_| on_close.call(i),
                            X { size: 16 }
                        }
                    }
                }
                button {
                    r#type: "button",
                    class: "mt-1 flex min-h-12 items-center gap-2 rounded-xl px-3 text-[16px] text-primary active:bg-accent/60",
                    onclick: move |_| {
                        tabs_open.set(false);
                        on_new.call(());
                    },
                    Plus { size: 18 }
                    "New note"
                }
            }
        }
        // ☰ — the rest of the app, since the vault's bar replaces the
        // app's own tab bar here.
        BottomSheet {
            open: menu_open(),
            title: "Go to".to_string(),
            on_close: move |()| menu_open.set(false),
            div { class: "grid grid-cols-2 gap-1 pb-2",
                for tab in sections.into_iter() {
                    Link {
                        key: "{tab.label}",
                        to: tab.route.clone(),
                        class: if tabs_match(&route, &tab) { "flex min-h-12 items-center gap-3 rounded-xl bg-accent px-3 text-[16px] text-foreground" } else { "flex min-h-12 items-center gap-3 rounded-xl px-3 text-[16px] text-foreground active:bg-accent/60" },
                        onclick: move |_| menu_open.set(false),
                        span { class: "flex size-6 shrink-0 items-center justify-center text-muted-foreground [&_svg]:size-5", {(tab.icon)()} }
                        span { class: "truncate", "{tab.label}" }
                    }
                }
            }
        }
    }
}
