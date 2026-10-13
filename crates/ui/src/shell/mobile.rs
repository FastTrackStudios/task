//! Mobile chrome — the primary-platform shell below `md`.
//!
//! Three pieces, all hidden at `md:` and up (where the desktop
//! sidebar takes over):
//!
//! - No top bar: the page starts at the top of the screen.
//! - [`MobileTimerDock`] — the timer, beside the capture button: a live
//!   clock pill while a timer runs (opening the timer sheet), nothing
//!   while none does (a timer starts from the capture box).
//! - [`BottomTabBar`] — fixed bottom bar (safe-area padded): four
//!   primary destinations + a "More" tab, drawn as your avatar, opening
//!   your account (the desktop [`crate::auth::AccountSwitcher`]'s content
//!   via [`crate::auth::AccountSheetBody`]), the full nav, the org
//!   switcher and the presence roster.
//! - [`BottomSheet`] — the shared mobile sheet primitive (architect-ui's
//!   `Sheet` only slides from the sides; mobile wants bottom sheets).
//!
//! Signal hygiene: every `use_signal` here is owned by a non-keyed,
//! shell-lifetime component (see `crate::collab` docs for the keyed-
//! child rule).

use architect_ui::lucide_dioxus::{ChevronsUpDown, Search, SlidersHorizontal, SquarePen};
use architect_ui::prelude::{Button, ButtonVariant, Text, TextVariant};
use chrono::Utc;
use dioxus::prelude::*;

use crate::auth::{AccountSheetBody, AuthCtx, Avatar};
use crate::chrome::{fmt_hms, owner_id, resolve_org, use_second_tick};
use crate::nav::{NavTab, nav_tabs_for, tabs_match, use_active_plugins};
use crate::orgs::{OrgMeta, OrgSelection};
use crate::routes::Route;
use crate::shell::org_switcher::OrgSwitcher;
use crate::stores;

// ── bottom sheet primitive ──────────────────────────────────────────

pub use task_ui_core::sheet::BottomSheet;

// ── sticky page action bar ──────────────────────────────────────────

/// Sticky mobile action bar — a page's primary actions, pinned just
/// above the [`BottomTabBar`] so they're always in thumb reach.
/// Fixed to the viewport (the tab bar is 56px + safe area), hidden at
/// `md:` and up where the desktop chrome takes over. Pages that render
/// one should add `pb-14 md:pb-0` to their container so the bar never
/// covers the last row of content.
#[component]
pub fn MobileActionBar(children: Element) -> Element {
    // Counted while mounted, so the floating capture controls lift
    // clear of the bar instead of covering its primary action.
    let mut bars = use_context::<crate::chrome::MobileActionBars>().0;
    use_hook(move || *bars.write() += 1);
    use_drop(move || {
        let n = *bars.peek();
        bars.set(n.saturating_sub(1));
    });
    rsx! {
        div {
            class: "fixed inset-x-0 z-30 flex items-center gap-2 border-t border-border bg-background/95 px-3 py-2 backdrop-blur md:hidden",
            // Above the floating tab bar (3.75rem pill + its margin).
            style: "bottom: calc(3.375rem + 0.75rem + max(0.75rem, calc(env(safe-area-inset-bottom, 0px) - 4px)));",
            {children}
        }
    }
}

// ── the timer, beside the capture button ────────────────────────────

/// A phone has no top bar: the page starts at the top of the screen.
/// The timer lives with the capture button instead — nothing while no
/// timer runs (start one from the capture box), and a live clock pill
/// beside the button while one does, which opens the timer sheet.
#[component]
pub fn MobileTimerDock() -> Element {
    let open = use_signal(|| false);
    rsx! {
        MobileTimerPill { open }
        MobileTimerSheet { open }
    }
}

/// The running timer as a small pill left of the capture button: a
/// pulsing dot and the live clock. Tapping it opens the timer sheet
/// (stop, what it is on, recent sessions). Absent when nothing runs.
///
/// Org-scoped off the shared [`OrgSelection`] and the optimistic session
/// store — the same source the desktop [`crate::chrome::TimerWidget`]
/// and the `/timer` page read, so all three stay in lockstep.
#[component]
fn MobileTimerPill(mut open: Signal<bool>) -> Element {
    let active = crate::chrome::use_active_timer();

    // Live clock — re-render once a second so the elapsed advances.
    let tick = use_signal(|| 0u64);
    use_second_tick(tick);
    let _ = tick();
    // A hook: read before the early return so the hook order holds.
    let lift = crate::chrome::action_bar_lift();

    let Some(at) = active else {
        return rsx! {};
    };
    let elapsed = (Utc::now() - at.session.start_time).num_seconds();
    rsx! {
        button {
            r#type: "button",
            // Level with the capture button (bottom-24, 48px tall), just
            // to its left.
            class: "fixed bottom-[6.375rem] right-[4.5rem] z-30 flex h-9 items-center gap-2 rounded-full border border-sky-500/40 bg-background/95 px-3 shadow-lg backdrop-blur active:bg-accent md:hidden",
            style: "margin-bottom: calc({lift} + env(safe-area-inset-bottom, 0px));",
            aria_label: "Timer running — open",
            onclick: move |_| open.set(true),
            span { class: "relative flex size-2 shrink-0",
                span { class: "absolute inline-flex size-full animate-ping rounded-full bg-sky-400/70" }
                span { class: "relative inline-flex size-2 rounded-full bg-sky-400" }
            }
            span { class: "font-mono text-sm font-semibold tabular-nums text-sky-400", "{fmt_hms(elapsed)}" }
        }
    }
}

/// The timer management sheet: start / stop, today's total, recent
/// sessions, and a link to the full `/timer` page. Derives the same
/// org-scoped session state the pill does, so the two never disagree.
#[component]
fn MobileTimerSheet(mut open: Signal<bool>) -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));

    let session_rows = stores::use_session_list();
    let muts = stores::use_timer_mutations();

    let active: Option<stores::OrgSession> = target().and_then(|(slug, org_id)| {
        let owner = owner_id(org_id);
        session_rows.value().and_then(|rows| {
            rows.iter()
                .map(|(_, r)| r)
                .find(|r| {
                    r.slug == slug && r.session.user_id == owner && r.session.end_time.is_none()
                })
                .cloned()
        })
    });

    // Live clock so the big elapsed advances while the sheet is open.
    let tick = use_signal(|| 0u64);
    use_second_tick(tick);
    let _ = tick();

    let mut draft = use_signal(String::new);

    // Optimistic start/stop. `target`/`muts`/`draft` are `Copy`, so each
    // handler gets its own closure; `active` isn't, so stop takes a clone.
    let make_start = move || {
        move || {
            let Some((slug, org_id)) = target() else {
                return;
            };
            let desc = draft.peek().trim().to_string();
            draft.set(String::new());
            muts.start(
                slug,
                timer_proto::StartTimerRequest {
                    user_id: owner_id(org_id),
                    org_id,
                    project_id: None,
                    project_path: String::new(),
                    task_note_path: String::new(),
                    description: desc,
                },
            );
        }
    };
    let mut start_input = make_start();
    let mut start_btn = make_start();
    let running = active.clone();
    let stop_sheet = move || {
        let Some((slug, org_id)) = target() else {
            return;
        };
        let Some(sess) = running.as_ref() else {
            return;
        };
        muts.stop(slug, owner_id(org_id), sess.session.id);
    };

    let elapsed = active
        .as_ref()
        .map_or(0, |r| (Utc::now() - r.session.start_time).num_seconds());
    let title = active.as_ref().map_or_else(
        || "(no description)".to_string(),
        |r| {
            if r.session.description.trim().is_empty() {
                "(no description)".to_string()
            } else {
                r.session.description.clone()
            }
        },
    );
    let tracking = active.is_some();

    // Recent + today totals, scoped to the active org + owner (newest
    // first, as the store yields them).
    let today = Utc::now().date_naive();
    let mine: Vec<stores::OrgSession> = match target() {
        Some((slug, org_id)) => {
            let owner = owner_id(org_id);
            session_rows.value().map_or_else(Vec::new, |rows| {
                rows.iter()
                    .map(|(_, r)| r)
                    .filter(|r| r.slug == slug && r.session.user_id == owner)
                    .cloned()
                    .collect()
            })
        }
        None => Vec::new(),
    };
    let today_secs: i64 = mine
        .iter()
        .filter(|r| r.session.start_time.date_naive() == today)
        .filter_map(|r| {
            r.session
                .end_time
                .map(|e| (e - r.session.start_time).num_seconds())
        })
        .sum();
    let recent: Vec<(String, String, String)> = mine
        .iter()
        .filter(|r| r.session.end_time.is_some())
        .take(6)
        .map(|r| {
            let s = &r.session;
            let dur = s.end_time.map_or(0, |e| (e - s.start_time).num_seconds());
            let desc = if s.description.trim().is_empty() {
                "(no description)".to_string()
            } else {
                s.description.clone()
            };
            (
                desc,
                fmt_hms(dur),
                s.start_time.format("%-I:%M %p").to_string(),
            )
        })
        .collect();

    rsx! {
        BottomSheet {
            open: open(),
            title: "Timer".to_string(),
            on_close: move |()| open.set(false),
            div { class: "flex flex-col gap-4 pb-2",
                if target().is_none() {
                    div { class: "rounded-2xl border border-border/70 bg-card/60 p-5 text-center",
                        Text { variant: TextVariant::Muted, "Connecting to your workspace…" }
                    }
                } else if tracking {
                    // Active session — big live clock + what you're on + Stop.
                    div { class: "flex flex-col gap-4 rounded-2xl border border-sky-500/40 bg-sky-500/10 p-5",
                        div { class: "flex items-center gap-2",
                            span { class: "relative flex size-2.5",
                                span { class: "absolute inline-flex size-full animate-ping rounded-full bg-sky-400/70" }
                                span { class: "relative inline-flex size-2.5 rounded-full bg-sky-400" }
                            }
                            span { class: "text-xs font-semibold uppercase tracking-[0.18em] text-sky-400",
                                "Tracking"
                            }
                        }
                        div { class: "font-mono text-5xl font-semibold leading-none tabular-nums tracking-tight",
                            "{fmt_hms(elapsed)}"
                        }
                        div { class: "text-sm text-foreground", "{title}" }
                        Button {
                            variant: ButtonVariant::Destructive,
                            class: "w-full",
                            on_click: move |_| {
                                stop_sheet();
                                open.set(false);
                            },
                            "Stop timer"
                        }
                    }
                } else {
                    // Idle — start a new session.
                    div { class: "flex flex-col gap-3 rounded-2xl border border-border/70 bg-card/60 p-5",
                        span { class: "text-xs font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                            "Start tracking"
                        }
                        input {
                            class: "w-full rounded-lg border border-border bg-background px-3 py-2.5 text-sm outline-none focus:ring-2 focus:ring-primary/40",
                            r#type: "text",
                            placeholder: "What are you working on?",
                            value: "{draft}",
                            oninput: move |e| draft.set(e.value()),
                            onkeydown: move |e| {
                                if e.key() == Key::Enter {
                                    start_input();
                                    open.set(false);
                                }
                            },
                        }
                        Button {
                            variant: ButtonVariant::Primary,
                            class: "w-full",
                            on_click: move |_| {
                                start_btn();
                                open.set(false);
                            },
                            "Start timer"
                        }
                    }
                }

                // Today total + recent sessions — substance, scoped to this org.
                if target().is_some() {
                    div { class: "flex flex-col gap-2",
                        div { class: "flex items-center justify-between px-1",
                            span { class: "text-xs font-semibold uppercase tracking-[0.18em] text-muted-foreground",
                                "Recent"
                            }
                            span { class: "font-mono text-xs tabular-nums text-muted-foreground",
                                "Today {fmt_hms(today_secs)}"
                            }
                        }
                        if recent.is_empty() {
                            div { class: "rounded-xl border border-dashed border-border/70 px-3 py-4 text-center",
                                Text { variant: TextVariant::Muted, "No sessions logged yet." }
                            }
                        } else {
                            div { class: "flex flex-col divide-y divide-border/50 overflow-hidden rounded-xl border border-border/60 bg-card/40",
                                for (i , (desc , dur , when)) in recent.iter().enumerate() {
                                    div { key: "{i}", class: "flex items-center justify-between gap-3 px-3 py-2.5",
                                        div { class: "flex min-w-0 flex-col",
                                            span { class: "truncate text-sm text-foreground", "{desc}" }
                                            span { class: "text-xs text-muted-foreground", "{when}" }
                                        }
                                        span { class: "shrink-0 font-mono text-xs tabular-nums text-muted-foreground",
                                            "{dur}"
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                Link {
                    to: Route::TimerRoute {},
                    class: "flex items-center justify-center gap-1.5 rounded-lg border border-border px-3 py-2.5 text-sm text-muted-foreground active:bg-accent",
                    onclick: move |_| open.set(false),
                    "View all sessions"
                }
            }
        }
    }
}

// ── the phone's tab bar (Linear's) ──────────────────────────────────

/// Where the switcher slot last pointed, so it keeps showing it.
const SECTION_KEY: &str = "task.nav.section";

/// The phone shell's bottom: a floating pill of four — Inbox, Tasks,
/// Favorites, and a section switcher — and, apart from it on the right,
/// the quick-input button that opens the composer.
///
/// The switcher shows the icon of the section you're in when that is
/// none of the three (Projects, Schedule, Vault…), else the last one you
/// visited (Schedule to start), with ⌃⌄ to say it opens a menu: the org
/// at the top, every section below, your current one highlighted.
#[component]
pub fn BottomTabBar(current: Route) -> Element {
    let mut menu_open = use_signal(|| false);
    let mut account_open = use_signal(|| false);
    let mut customize_open = use_signal(|| false);
    let to_process = crate::process::use_to_process_count();
    let new_issue = use_context::<crate::new_issue::NewIssue>();
    let every = nav_tabs_for(&use_active_plugins());
    // The person's arrangement (Customize): the menu lists what they
    // show, in their order, and the first three are the tab bar.
    let layout = crate::nav_layout::use_nav_layout();
    let (all, _hidden) = layout.read().arrange(every.clone());
    let primary: Vec<NavTab> = all
        .iter()
        .take(crate::nav_layout::PRIMARY)
        .cloned()
        .collect();

    // The section the switcher shows.
    let in_primary = primary.iter().any(|t| tabs_match(&current, t));
    let here = all
        .iter()
        .find(|t| !in_primary && tabs_match(&current, t))
        .cloned();
    if let Some(t) = here.as_ref() {
        crate::local_store::set(SECTION_KEY, t.label);
    }
    let remembered = crate::local_store::get(SECTION_KEY).unwrap_or_else(|| "Schedule".to_owned());
    // Never one of the three already on the bar: the remembered section
    // if it isn't, else the first section after them.
    let off_bar = |t: &&NavTab| !primary.iter().any(|p| p.label == t.label);
    let section = here.clone().or_else(|| {
        all.iter()
            .filter(off_bar)
            .find(|t| t.label == remembered)
            .or_else(|| all.iter().find(off_bar))
            .cloned()
    });

    let slot = |active: bool| {
        if active {
            "relative flex h-[2.875rem] flex-1 items-center justify-center gap-1 rounded-full bg-accent text-foreground"
        } else {
            "relative flex h-[2.875rem] flex-1 items-center justify-center gap-1 rounded-full text-foreground/80 active:bg-accent/60"
        }
    };
    let badge = if to_process > 99 {
        "99+".to_owned()
    } else {
        to_process.to_string()
    };

    rsx! {
        nav {
            class: "fixed inset-x-0 bottom-0 z-30 flex items-end gap-2 px-7 md:hidden",
            style: "padding-bottom: max(0.75rem, calc(env(safe-area-inset-bottom, 0px) - 4px));",
            // The pill.
            div { class: "flex h-[3.375rem] flex-1 items-center gap-0.5 rounded-full border border-border/70 bg-popover/95 p-1 shadow-[0_8px_30px_rgb(0_0_0/0.35)] backdrop-blur",
                for tab in primary.iter() {
                    Link {
                        key: "{tab.label}",
                        to: tab.route.clone(),
                        class: slot(tabs_match(&current, tab)),
                        aria_label: "{tab.label}",
                        span { class: "flex size-6 items-center justify-center [&_svg]:size-[22px]", {(tab.icon)()} }
                        if matches!(tab.route, Route::InboxRoute {}) && to_process > 0 {
                            span { class: "absolute right-[22%] top-1.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-primary px-1 text-[10px] font-semibold leading-none tabular-nums text-primary-foreground",
                                "{badge}"
                            }
                        }
                    }
                }
                // The section switcher.
                button {
                    r#type: "button",
                    class: slot(here.is_some()),
                    aria_label: "Sections",
                    aria_haspopup: "menu",
                    aria_expanded: menu_open().to_string(),
                    onclick: move |_| menu_open.toggle(),
                    if let Some(t) = section.clone() {
                        span { class: "flex size-6 items-center justify-center [&_svg]:size-[22px]", {(t.icon)()} }
                    }
                    span { class: "text-muted-foreground", ChevronsUpDown { size: 12 } }
                }
            }
            // Quick input — the composer, from anywhere.
            button {
                r#type: "button",
                class: "flex size-[3.375rem] shrink-0 items-center justify-center rounded-full border border-border/70 bg-popover/95 text-foreground shadow-[0_8px_30px_rgb(0_0_0/0.35)] backdrop-blur active:bg-accent",
                aria_label: "New issue",
                onclick: move |_| new_issue.open(String::new()),
                SquarePen { size: 22 }
            }
        }
        if menu_open() {
            SectionMenu {
                tabs: all,
                current: current.clone(),
                on_account: move |()| {
                    menu_open.set(false);
                    account_open.set(true);
                },
                on_customize: move |()| {
                    menu_open.set(false);
                    customize_open.set(true);
                },
                on_close: move |()| menu_open.set(false),
            }
        }
        crate::nav_layout::CustomizeSheet {
            open: customize_open(),
            on_close: move |()| customize_open.set(false),
            tabs: every,
            layout,
        }
        BottomSheet {
            open: account_open(),
            title: "Account & status".to_string(),
            on_close: move |()| account_open.set(false),
            AccountSheetBody { on_done: move |()| account_open.set(false) }
        }
    }
}

/// The card the switcher opens, above the bar: the org (switch it from
/// here), your account, and every section — the current one marked.
#[component]
fn SectionMenu(
    tabs: Vec<NavTab>,
    current: Route,
    on_account: EventHandler<()>,
    on_customize: EventHandler<()>,
    on_close: EventHandler<()>,
) -> Element {
    let mut search = use_context::<crate::chrome::SearchOpen>().0;
    let ctx = use_context::<AuthCtx>();
    let account = ctx.active.read().clone();
    let (name, email) = account.as_ref().map_or_else(
        || (String::new(), String::new()),
        |a| (a.name.clone(), a.email.clone()),
    );
    rsx! {
        div { class: "fixed inset-0 z-40 md:hidden", onclick: move |_| on_close.call(()) }
        div {
            class: "fixed inset-x-7 bottom-[calc(3.375rem+0.5rem+max(0.75rem,calc(env(safe-area-inset-bottom)-4px)))] z-50 flex max-h-[65vh] flex-col overflow-hidden rounded-[1.75rem] border border-border/70 bg-popover shadow-2xl md:hidden",
            role: "menu",
            onclick: move |e| e.stop_propagation(),
            div { class: "flex items-center gap-2 border-b border-border/60 px-4 py-2.5",
                div { class: "min-w-0 flex-1", OrgSwitcher { compact: false, theme: false } }
                button {
                    r#type: "button",
                    class: "flex size-10 shrink-0 items-center justify-center rounded-full text-foreground active:bg-accent",
                    aria_label: "Customize",
                    onclick: move |_| on_customize.call(()),
                    SlidersHorizontal { size: 19 }
                }
                button {
                    r#type: "button",
                    class: "flex size-10 shrink-0 items-center justify-center rounded-full active:bg-accent",
                    aria_label: "Account & status",
                    onclick: move |_| on_account.call(()),
                    Avatar { name: name.clone(), email: email.clone(), size: 26 }
                }
            }
            ul { class: "min-h-0 flex-1 overflow-y-auto p-2",
                for tab in tabs.into_iter() {
                    {
                        let on = tabs_match(&current, &tab);
                        rsx! {
                            li { key: "{tab.label}",
                                Link {
                                    to: tab.route.clone(),
                                    class: if on {
                                        "flex min-h-12 items-center gap-3.5 rounded-full bg-accent px-4 text-[17px] text-foreground"
                                    } else {
                                        "flex min-h-12 items-center gap-3.5 rounded-full px-4 text-[17px] text-foreground active:bg-accent/60"
                                    },
                                    onclick: move |_| on_close.call(()),
                                    span { class: "flex size-6 shrink-0 items-center justify-center [&_svg]:size-5", {(tab.icon)()} }
                                    span { class: "truncate", "{tab.label}" }
                                }
                            }
                        }
                    }
                }
                li {
                    button {
                        r#type: "button",
                        class: "flex min-h-12 w-full items-center gap-3.5 rounded-full px-4 text-left text-[17px] text-foreground active:bg-accent/60",
                        onclick: move |_| {
                            on_close.call(());
                            search.set(true);
                        },
                        span { class: "flex size-6 shrink-0 items-center justify-center", Search { size: 20 } }
                        "Search"
                    }
                }
            }
        }
    }
}

/// The "More" sheet content: the full nav (everything the desktop
/// sidebar lists), the org switcher, and the presence roster. Only
/// mounted while the sheet is open, so the roster's polling future
/// runs only then.
#[component]
fn MoreSheetBody(on_navigate: EventHandler<()>) -> Element {
    rsx! {
        div { class: "flex flex-col gap-4 pb-2",
            ul { class: "grid grid-cols-2 gap-x-2",
                for tab in nav_tabs_for(&use_active_plugins()) {
                    li { key: "{tab.label}",
                        Link {
                            to: tab.route.clone(),
                            class: "flex min-h-[44px] items-center gap-3 rounded-lg px-3 py-2.5 text-sm text-foreground active:bg-accent",
                            onclick: move |_| on_navigate.call(()),
                            span { class: "flex h-5 w-5 shrink-0 items-center justify-center", {(tab.icon)()} }
                            span { class: "truncate", "{tab.label}" }
                        }
                    }
                }
            }
            section {
                h3 { class: "px-1 pb-1 text-xs font-semibold uppercase tracking-widest text-muted-foreground",
                    "Organization"
                }
                OrgSwitcher { compact: false }
            }
            // Who's online — the same roster the desktop sidebar shows.
            crate::presence::PresenceRoster {}
        }
    }
}

#[component]
fn TabBarItem(tab: NavTab, active: bool, #[props(default)] badge: usize) -> Element {
    let class = if active {
        "flex min-h-[56px] w-full flex-col items-center justify-center gap-1 py-2 text-primary"
    } else {
        "flex min-h-[56px] w-full flex-col items-center justify-center gap-1 py-2 text-muted-foreground active:text-foreground"
    };
    let label = if badge > 99 {
        "99+".to_owned()
    } else {
        badge.to_string()
    };
    rsx! {
        Link { to: tab.route.clone(), class,
            span { class: "relative flex h-5 w-5 items-center justify-center",
                {(tab.icon)()}
                // What's waiting to be processed — the one number the
                // tab bar shows, because processing is the core loop.
                if badge > 0 {
                    span { class: "absolute -right-2.5 -top-1.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-primary px-1 text-[10px] font-semibold leading-none tabular-nums text-primary-foreground",
                        "{label}"
                    }
                }
            }
            span { class: "text-[11px] font-medium", "{tab.label}" }
        }
    }
}
