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

use architect_ui::lucide_dioxus::Feather;
use architect_ui::prelude::{Button, ButtonVariant, Text, TextVariant};
use chrono::Utc;
use dioxus::prelude::*;

use crate::chrome::{fmt_hms, owner_id, resolve_org, use_second_tick};
use crate::nav::{NavTab, nav_tabs_for, tabs_match, use_active_plugins};
use crate::orgs::{OrgMeta, OrgSelection};
use crate::routes::Route;
use crate::shell::org_switcher::OrgSwitcher;
use crate::stores;

// ── bottom sheet primitive ──────────────────────────────────────────

pub use task_ui_core::sheet::BottomSheet;

// ── on-screen keyboard height ───────────────────────────────────────

/// Keeps `--kb` on the root element at the on-screen keyboard's height
/// (0 when none). On iOS the keyboard shrinks the *visual* viewport and
/// leaves fixed layout where it was, so anything pinned to the bottom —
/// the phone command palette's field, a composer's chips — sits under
/// the keyboard unless it pads by this.
const KEYBOARD_INSET_JS: &str = r#"
(() => {
  if (window.__taskKb || !window.visualViewport) return;
  window.__taskKb = true;
  const vv = window.visualViewport;
  const set = () => {
    const s = document.documentElement.style;
    const kb = Math.max(0, window.innerHeight - vv.height - vv.offsetTop);
    s.setProperty('--kb', kb + 'px');
    // `kb-open` while the on-screen keyboard is up — what keyboard
    // toolbars show on.
    document.documentElement.classList.toggle('kb-open', kb > 80);
    // The visible rectangle itself: iOS scrolls the layout viewport to
    // keep a focused field in view, which moves anything `fixed` with
    // it. A sheet that must stay on screen pins to these instead.
    s.setProperty('--vv-top', vv.offsetTop + 'px');
    s.setProperty('--vv-h', vv.height + 'px');
  };
  vv.addEventListener('resize', set);
  vv.addEventListener('scroll', set);
  set();
})();
"#;

/// Install [`KEYBOARD_INSET_JS`] once. Call from the app shell.
pub fn use_keyboard_inset() {
    use_hook(|| {
        let _ = document::eval(KEYBOARD_INSET_JS);
    });
}

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

/// How far the capture button travels up to mean "start a timer".
const SWIPE_TIMER: f64 = 36.0;

/// The phone shell's bottom: a floating pill of four sections — Inbox,
/// Tasks, Schedule and Vault by default (the person's own first four,
/// from Customize) — and, apart from it on the right, the capture
/// button: a fleeting thought, into the inbox.
///
/// Every other section is in the left drawer (swipe right), under the
/// work it sits beside; there is no switcher slot, so the bar spends all
/// four places on sections a thumb reaches all day.
#[component]
pub fn BottomTabBar(current: Route) -> Element {
    let to_process = crate::process::use_to_process_count();
    let new_issue = use_context::<crate::new_issue::NewIssue>();
    // The capture button's swipe-up: how far it has lifted, and whether
    // it has gone far enough to mean "start a timer".
    let mut touch_y = use_signal(|| None::<f64>);
    let mut lift = use_signal(|| 0.0f64);
    let mut swiping = use_signal(|| false);
    let mut swallow_click = use_signal(|| false);
    let start_timer = crate::pages::timer_focus::use_start_timer_now();
    let every = nav_tabs_for(&use_active_plugins());
    let layout = crate::nav_layout::use_nav_layout();
    let (all, _hidden) = layout.read().arrange(every);
    let primary: Vec<NavTab> = all.into_iter().take(crate::nav_layout::PRIMARY).collect();

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
            }
            // Quick capture — a fleeting thought, from anywhere; it waits in
            // the inbox to be processed. (New issues are each section's
            // header action.)
            // Tap: capture. Press and swipe up: a timer starts at once and
            // its page opens — for when you are starting before you know
            // what to call it.
            button {
                r#type: "button",
                "data-testid": "capture-button",
                class: if swiping() { "flex size-[3.375rem] shrink-0 items-center justify-center rounded-full border border-primary bg-primary text-primary-foreground shadow-[0_8px_30px_rgb(0_0_0/0.35)] transition-transform" } else { "flex size-[3.375rem] shrink-0 items-center justify-center rounded-full border border-border/70 bg-popover/95 text-foreground shadow-[0_8px_30px_rgb(0_0_0/0.35)] backdrop-blur active:bg-accent" },
                style: if lift() > 0.0 { format!("transform: translateY(-{:.0}px)", lift().min(48.0)) } else { String::new() },
                aria_label: "Capture a fleeting thought — swipe up to start a timer",
                ontouchstart: move |e: TouchEvent| {
                    if let Some(t) = e.touches().first() {
                        touch_y.set(Some(t.client_coordinates().y));
                    }
                },
                ontouchmove: move |e: TouchEvent| {
                    if let (Some(y0), Some(t)) = (touch_y(), e.touches().first()) {
                        let up = (y0 - t.client_coordinates().y).max(0.0);
                        lift.set(up);
                        swiping.set(up >= SWIPE_TIMER);
                    }
                },
                ontouchend: move |_| {
                    let go = swiping();
                    touch_y.set(None);
                    lift.set(0.0);
                    swiping.set(false);
                    if go {
                        swallow_click.set(true);
                        start_timer(String::new());
                    }
                },
                onclick: move |_| {
                    if swallow_click() {
                        swallow_click.set(false);
                        return;
                    }
                    new_issue.capture(String::new());
                },
                if swiping() {
                    architect_ui::lucide_dioxus::Timer { size: 22 }
                } else {
                    Feather { size: 22 }
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
                OrgSwitcher { compact: false, up: true }
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
