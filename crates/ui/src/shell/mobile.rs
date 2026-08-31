//! Mobile chrome — the primary-platform shell below `md`.
//!
//! Three pieces, all hidden at `md:` and up (where the desktop
//! sidebar takes over):
//!
//! - [`MobileHeader`] — sticky top app bar: the [`MobileTimerBar`]
//!   (current tracked task + live clock + start/stop, opening a timer
//!   management sheet) and an avatar button that opens the **account &
//!   status** bottom sheet (the desktop [`crate::auth::AccountSwitcher`]'s
//!   content via [`crate::auth::AccountSheetBody`] — same `AuthCtx` /
//!   `PresenceLocal` logic, touch-sized presentation).
//! - [`BottomTabBar`] — fixed bottom bar (safe-area padded): four
//!   primary destinations + a "More" tab opening the full nav as a
//!   bottom sheet, with the org switcher and the presence roster.
//! - [`BottomSheet`] — the shared mobile sheet primitive (architect-ui's
//!   `Sheet` only slides from the sides; mobile wants bottom sheets).
//!
//! Signal hygiene: every `use_signal` here is owned by a non-keyed,
//! shell-lifetime component (see `crate::collab` docs for the keyed-
//! child rule).

use architect_ui::lucide_dioxus::{Menu, Play, Square};
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

// ── top app bar ─────────────────────────────────────────────────────

#[component]
pub fn MobileHeader() -> Element {
    let mut account_open = use_signal(|| false);
    // Timer sheet open state — shared by the in-header bar and the sheet,
    // which is rendered *outside* the header (see below).
    let timer_open = use_signal(|| false);

    // Avatar trigger state — same identity the desktop switcher shows.
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
        header {
            class: "sticky top-0 z-20 flex items-center gap-2 border-b border-border bg-background/95 px-3 py-2 backdrop-blur md:hidden",
            style: "padding-top: max(0.5rem, env(safe-area-inset-top, 0px));",
            // The timer carries the bar now — current task + live clock,
            // tap to manage (start/stop, history).
            MobileTimerBar { open: timer_open }
            div { class: "ml-auto flex shrink-0 items-center",
                // Account & status — opens the bottom sheet.
                button {
                    r#type: "button",
                    class: "flex h-11 w-11 items-center justify-center rounded-full active:bg-accent",
                    aria_label: "Account & status",
                    onclick: move |_| account_open.set(true),
                    span { class: "relative",
                        Avatar { name: name.clone(), email: email.clone(), size: 32 }
                        span { class: "absolute -bottom-0.5 -right-0.5 h-2.5 w-2.5 rounded-full border-2 border-background {dot}",
                            title: "{effective.label()}",
                        }
                    }
                }
            }
        }
        // Rendered OUTSIDE `header`: the header's `backdrop-blur`
        // establishes a containing block for `fixed` descendants, which
        // would anchor the sheet to the header box instead of the
        // viewport. Keeping it a sibling (like the account sheet) lets
        // `fixed inset-x-0 bottom-0` resolve against the viewport.
        MobileTimerSheet { open: timer_open }
        BottomSheet {
            open: account_open(),
            title: "Account & status".to_string(),
            on_close: move |()| account_open.set(false),
            AccountSheetBody { on_done: move |()| account_open.set(false) }
        }
    }
}

// ── mobile timer bar ────────────────────────────────────────────────

/// The header's primary affordance on mobile: a live view of the
/// session being tracked right now (pulsing dot + description + elapsed
/// clock) with an inline Stop, or a "Start a timer" prompt when nothing
/// runs. Tapping the bar opens a bottom sheet to start / stop and jump
/// to the full `/timer` history.
///
/// Org-scoped off the shared [`OrgSelection`] and the optimistic session
/// store — the same source the desktop [`crate::chrome::TimerWidget`]
/// and the `/timer` page read, so all three stay in lockstep.
#[component]
fn MobileTimerBar(mut open: Signal<bool>) -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));

    let session_rows = stores::use_session_list();
    let muts = stores::use_timer_mutations();

    // Running session = the open row for the active org + owner.
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

    // Live clock — re-render once a second so the elapsed advances.
    let tick = use_signal(|| 0u64);
    use_second_tick(tick);
    let _ = tick();

    // Stop the running session (optimistic). `active` isn't `Copy`, so
    // the handler takes a clone; `target`/`muts` are `Copy`.
    let running = active.clone();
    let stop_bar = move || {
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

    rsx! {
        div { class: "flex min-w-0 flex-1 items-center gap-1",
            if tracking {
                // Stop sits on the left, in the same slot the Play badge
                // occupies when idle.
                button {
                    r#type: "button",
                    class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-destructive/10 text-destructive active:bg-destructive/20",
                    aria_label: "Stop timer",
                    onclick: move |_| stop_bar(),
                    Square { size: 16 }
                }
                button {
                    r#type: "button",
                    class: "flex min-w-0 flex-1 items-center gap-2 rounded-lg py-1 pl-1 pr-2 text-left active:bg-accent",
                    onclick: move |_| open.set(true),
                    span { class: "flex min-w-0 flex-col leading-tight",
                        span { class: "truncate text-sm font-medium text-foreground", "{title}" }
                        span { class: "font-mono text-xs tabular-nums text-sky-400", "{fmt_hms(elapsed)}" }
                    }
                }
            } else {
                button {
                    r#type: "button",
                    class: "flex min-w-0 flex-1 items-center gap-2 rounded-lg py-1 pl-1 pr-2 text-left text-muted-foreground active:bg-accent",
                    onclick: move |_| open.set(true),
                    span { class: "flex h-8 w-8 shrink-0 items-center justify-center rounded-full bg-primary/10 text-primary",
                        Play { size: 16 }
                    }
                    span { class: "truncate text-sm", "Start a timer" }
                }
            }
        }
    }
}

/// The timer management sheet — rendered **outside** the mobile header so
/// its `fixed inset-x-0 bottom-0` panel resolves against the viewport.
/// (The header's `backdrop-blur` establishes a containing block for
/// `fixed` descendants, which would otherwise pin the sheet to the
/// header.) Start / stop, today's total, recent sessions, and a link to
/// the full `/timer` page. Derives the same org-scoped session state the
/// bar does, so the two never disagree.
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
    let primary = primary_mobile_tabs();
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
                        aria_label: "More sections",
                        onclick: move |_| more_open.set(true),
                        Menu { size: 20 }
                        span { class: "text-[10px] font-semibold uppercase tracking-widest", "More" }
                    }
                }
            }
        }
        BottomSheet {
            open: more_open(),
            title: "All sections".to_string(),
            on_close: move |()| more_open.set(false),
            MoreSheetBody { on_navigate: move |()| more_open.set(false) }
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
