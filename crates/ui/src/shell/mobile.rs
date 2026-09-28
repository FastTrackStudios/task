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


use architect_ui::prelude::{Button, ButtonVariant, Text, TextVariant};
use chrono::Utc;
use dioxus::prelude::*;

use crate::auth::{AccountSheetBody, AuthCtx, Avatar};
use crate::chrome::{fmt_hms, owner_id, resolve_org, use_second_tick};
use crate::nav::{NavTab, nav_tabs_for, primary_mobile_tabs, tabs_match, use_active_plugins};
use crate::orgs::{OrgMeta, OrgSelection};
use crate::presence::PresenceLocal;
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
    rsx! {
        div {
            class: "fixed inset-x-0 z-30 flex items-center gap-2 border-t border-border bg-background/95 px-3 py-2 backdrop-blur md:hidden",
            style: "bottom: calc(3.5rem + env(safe-area-inset-bottom, 0px));",
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
            style: "margin-bottom: env(safe-area-inset-bottom, 0px);",
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

// ── bottom tab bar + "More" sheet ───────────────────────────────────

#[component]
pub fn BottomTabBar(current: Route) -> Element {
    let mut more_open = use_signal(|| false);
    let mut account_open = use_signal(|| false);
    let primary = primary_mobile_tabs();

    // "More" is you: your avatar and presence dot, opening every section
    // and, at the top of that sheet, your account.
    let ctx = use_context::<AuthCtx>();
    let local = use_context::<PresenceLocal>();
    let account = ctx.active.read().clone();
    let (name, email) = account.as_ref().map_or_else(
        || ("Signing in…".to_owned(), String::new()),
        |a| (a.name.clone(), a.email.clone()),
    );
    let effective = local.effective_status();
    let dot = effective.dot_class();
    rsx! {
        nav {
            class: "fixed inset-x-0 bottom-0 z-30 border-t border-border bg-background/95 backdrop-blur md:hidden",
            style: "padding-bottom: env(safe-area-inset-bottom, 0px);",
            ul { class: "mx-auto grid max-w-md grid-cols-5",
                for tab in primary.iter() {
                    li { key: "{tab.label}",
                        TabBarItem { tab: tab.clone(), active: tabs_match(&current, tab) }
                    }
                }
                li {
                    button {
                        r#type: "button",
                        class: "flex min-h-[56px] w-full flex-col items-center justify-center gap-1 py-2 text-muted-foreground active:text-foreground",
                        aria_label: "More sections and your account",
                        onclick: move |_| more_open.set(true),
                        span { class: "relative flex h-5 w-5 items-center justify-center",
                            Avatar { name: name.clone(), email: email.clone(), size: 22 }
                            span { class: "absolute -bottom-0.5 -right-0.5 h-2 w-2 rounded-full border-2 border-background {dot}",
                                title: "{effective.label()}",
                            }
                        }
                        span { class: "text-[10px] font-semibold uppercase tracking-widest", "More" }
                    }
                }
            }
        }
        BottomSheet {
            open: more_open(),
            title: "All sections".to_string(),
            on_close: move |()| more_open.set(false),
            // Your account — the sheet the phone's top bar used to open.
            button {
                r#type: "button",
                class: "mb-3 flex min-h-[52px] w-full items-center gap-3 rounded-xl border border-border/70 bg-card/60 px-3 py-2 text-left active:bg-accent",
                onclick: move |_| {
                    more_open.set(false);
                    account_open.set(true);
                },
                span { class: "relative shrink-0",
                    Avatar { name: name.clone(), email: email.clone(), size: 32 }
                    span { class: "absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-background {dot}" }
                }
                span { class: "flex min-w-0 flex-col leading-tight",
                    span { class: "truncate text-sm font-medium text-foreground", "{name}" }
                    span { class: "truncate text-xs text-muted-foreground", "{effective.label()} · Account & status" }
                }
            }
            MoreSheetBody { on_navigate: move |()| more_open.set(false) }
        }
        BottomSheet {
            open: account_open(),
            title: "Account & status".to_string(),
            on_close: move |()| account_open.set(false),
            AccountSheetBody { on_done: move |()| account_open.set(false) }
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
fn TabBarItem(tab: NavTab, active: bool) -> Element {
    let class = if active {
        "flex min-h-[56px] w-full flex-col items-center justify-center gap-1 py-2 text-primary"
    } else {
        "flex min-h-[56px] w-full flex-col items-center justify-center gap-1 py-2 text-muted-foreground active:text-foreground"
    };
    rsx! {
        Link { to: tab.route.clone(), class,
            span { class: "flex h-5 w-5 items-center justify-center", {(tab.icon)()} }
            span { class: "text-[10px] font-semibold uppercase tracking-widest", "{tab.label}" }
        }
    }
}
