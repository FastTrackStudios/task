//! Two-column desktop layout / single-column mobile layout.
//! Wraps the active route via `Outlet`.

use dioxus::prelude::*;

use crate::chrome::{FleetingFab, FleetingModal, TopBar, provide_chrome_contexts};
use crate::routes::Route;
use crate::shell::mobile::{BottomTabBar, MobileHeader};

#[component]
pub fn AppShell() -> Element {
    let current = use_route::<Route>();

    // Quick-capture + data-refresh signals for the persistent chrome.
    provide_chrome_contexts();
    // Ctrl+P command-palette visibility (same pattern as FleetingOpen).
    crate::palette::provide_palette_context();
    // Obsidian-style route tabs (the strip lives in the TopBar) —
    // restores the persisted strip and seeds it with the boot route.
    crate::tabs::provide_tabs(current.clone());
    // Shell panel state: the vault explorer + the right (backlinks)
    // panel, toggled from the top bar (Obsidian-style).
    let explorer = use_context_provider(|| Signal::new(crate::chrome::ExplorerOpen(true)));
    // The Files page's selection (which root / Drive) — provided at
    // the shell so the sidebar column (FilesSidebar) and the pane
    // drive one state.
    use_context_provider(|| Signal::new(files_ui::Selection::default()));
    let _ = use_context_provider(|| Signal::new(crate::chrome::RightPanelOpen(true)));
    let agent_panel = use_context_provider(|| Signal::new(crate::chrome::AgentPanelOpen(false)));
    let _ = use_context_provider(|| Signal::new(crate::chrome::AgentPanelSelected(String::new())));
    let mut agent_width =
        use_context_provider(|| Signal::new(crate::chrome::AgentPanelWidth(416.0)));
    // Restore the persisted panel width once per shell mount.
    use_future(move || async move {
        let mut eval = dioxus::document::eval(
            "dioxus.send(localStorage.getItem('task.agent-panel.width') || '');",
        );
        if let Ok(v) = eval.recv::<String>().await {
            if let Ok(px) = v.parse::<f64>() {
                agent_width.set(crate::chrome::AgentPanelWidth(px.clamp(320.0, 720.0)));
            }
        }
    });
    // Drag state: Some((pointer x at drag start, width at drag start)).
    let mut agent_drag = use_signal(|| None::<(f64, f64)>);
    // Zen mode (Ctrl+Shift+Z): render NO desktop chrome — the open
    // view gets the whole viewport. Only the rendering is gated; the
    // explorer/right-panel signals keep their values, so exiting zen
    // restores exactly what was showing. Desktop-only: everything zen
    // hides is `md:`-gated already, so mobile is unaffected.
    let zen = use_context::<crate::chrome::ZenMode>().0;
    // Share-link visitors get NO chrome at all (and no presence entry,
    // no start-page redirect — they must land exactly on the shared view).
    let share = use_context::<crate::chrome::ShareMode>().0;
    let chromeless = move || zen() || share;

    rsx! {
        // Publishes this client's presence entry (route activity, idle,
        // manual status) on the org channel joined at the app root.
        // Renders nothing; lives here because it needs `use_route`.
        if !share {
            crate::presence::PresencePublisher {}
            // One-shot start-page redirect from the user's prefs entity.
            StartPageRedirect {}
        }
        // Mobile is the primary platform: below `md` the chrome is the
        // top app bar + bottom tab bar. At `md`+ the desktop shell is
        // Obsidian-shaped (plans/vault-views.md): one full-width top
        // bar over everything (sidebar toggles, timer, capture — and
        // where tabs will live), then icon rail → vault explorer →
        // the open view.
        div { class: "min-h-screen bg-background text-foreground md:flex md:h-screen md:flex-col md:overflow-hidden",
            if !chromeless() {
                TopBar {}
            }
            div { class: "md:flex md:min-h-0 md:flex-1",
            if !chromeless() {
                div { class: "hidden md:block",
                    crate::shell::rail::IconRail { current: current.clone() }
                }
            }
            // Everything right of the icon rail: the [vault explorer |
            // open view] row, with the IDE status line spanning BENEATH
            // it. The rail runs full-height beside the status bar, but the
            // vault explorer stops just above it (VS Code-style).
            div { class: "flex flex-col md:min-h-0 md:flex-1 md:overflow-hidden",
                div { class: "flex flex-col md:min-h-0 md:flex-1 md:flex-row",
                    if explorer.read().0 && !chromeless() {
                        div { class: "hidden w-[17rem] shrink-0 border-r border-border/60 md:flex md:min-h-0 md:flex-col md:overflow-hidden",
                            // On the Files page the sidebar column IS
                            // the file sidebar — the whole screen is
                            // the file manager.
                            if matches!(current, Route::FilesRoute {}) {
                                files_ui::FilesSidebar {}
                            } else {
                                crate::shell::explorer::VaultExplorer {}
                            }
                        }
                    }
                    div { class: "flex min-h-screen flex-col md:min-h-0 md:flex-1 md:overflow-hidden",
                        if !share {
                            MobileHeader {}
                        }
                        // Bottom padding keeps content clear of the fixed
                        // tab bar (56px + safe area). On desktop `main` is
                        // the scroll container.
                        main { class: "flex-1 pb-[calc(6rem+env(safe-area-inset-bottom,0px))] md:min-h-0 md:overflow-y-auto md:pb-0",
                            SuspenseBoundary {
                                fallback: |_| rsx! { RouteFallback {} },
                                Outlet::<Route> {}
                            }
                        }
                        if !share {
                            BottomTabBar { current }
                            FleetingFab {}
                        }
                    }
                    // The right agent sidebar: conversations + chat,
                    // alongside whatever the center view shows. Left
                    // edge is a drag handle (width persisted).
                    if agent_panel.read().0 && !chromeless() {
                        div {
                            class: "relative hidden shrink-0 border-l border-border/60 md:flex md:min-h-0 md:flex-col md:overflow-hidden",
                            style: "width: {agent_width.read().0}px;",
                            div {
                                class: "absolute left-0 top-0 z-30 h-full w-1.5 cursor-col-resize hover:bg-primary/40",
                                onpointerdown: move |e| {
                                    agent_drag.set(Some((
                                        e.client_coordinates().x,
                                        agent_width.peek().0,
                                    )));
                                },
                            }
                            crate::shell::agent_panel::AgentPanel {}
                        }
                    }
                }
                // IDE status line — spans the explorer + view (right of the
                // full-height rail), pinned to the base.
                if !zen() {
                    crate::chrome::StatusBar {}
                }
            }
            }
        }
        // Full-screen overlay while resizing the agent panel — it owns
        // the pointer so the drag never drops into iframes/textareas.
        if let Some((start_x, start_w)) = agent_drag() {
            div {
                class: "fixed inset-0 z-50 cursor-col-resize",
                onpointermove: move |e| {
                    // Panel is on the right: dragging left grows it.
                    let w = (start_w + (start_x - e.client_coordinates().x)).clamp(320.0, 720.0);
                    agent_width.set(crate::chrome::AgentPanelWidth(w));
                },
                onpointerup: move |_| {
                    agent_drag.set(None);
                    let w = agent_width.peek().0;
                    let _ = dioxus::document::eval(&format!(
                        "localStorage.setItem('task.agent-panel.width', '{w:.0}');"
                    ));
                },
                onpointercancel: move |_| agent_drag.set(None),
            }
        }
        // Zen's only chrome: the hover-revealed exit button in the
        // top-left corner.
        if zen() {
            crate::chrome::ZenExitOverlay {}
        }
        // Global Now Playing engine — headless, mounted here (outside the
        // route Outlet) so playback survives navigation. The UI is the
        // status-bar tab (desktop) / the floating tab below (mobile).
        task_player_ui::GlobalNowPlayer {}
        // Marks the playing setlist row + feeds its artwork waveform.
        task_player_ui::NowPlayingStripHighlighter {}
        // Mobile: no desktop status bar, so float the same tab above the
        // bottom tab bar. Renders nothing until something plays.
        if !share {
            div { class: "md:hidden fixed bottom-[calc(3.5rem+env(safe-area-inset-bottom,0px))] right-2 z-40",
                task_player_ui::NowPlayingTab {}
            }
        }
        // Single global capture modal, toggled from any fleeting button.
        FleetingModal {}
        // Ctrl+P command palette — pages + vault notes, fuzzy-ranked.
        // Mounts its own document-level hotkey listener.
        crate::palette::CommandPalette {}
        // App-wide notice queue (architect::Notifications, provided by
        // `use_app_reactive` at the app root). Mutations + the vault
        // DocumentSession report failures here so they outlive the
        // screen that caused them.
        NotificationTray {}
        // Slim connection pill: visible only while the supervised vox
        // connection is down or re-establishing.
        ConnectionBanner {}
    }
}

/// Fixed bottom-right toast stack over the notification queue —
/// severity accent + icon, ×N dedupe badge, and per-notice TTL expiry
/// (the queue carries `ttl_ms`; this tray arms one dismiss task per
/// `(id, count)` so a re-pushed notice restarts its clock).
#[component]
fn NotificationTray() -> Element {
    use architect_ui::lucide_dioxus::{CircleCheck, Info, TriangleAlert};

    let notices = architect::use_notifications();
    // (id, count) pairs that already have a dismiss timer in flight.
    let mut armed = use_signal(std::collections::HashSet::<(u64, u32)>::new);
    use_effect(move || {
        let list = notices.list();
        for n in &list {
            let Some(ttl) = n.ttl_ms else { continue };
            let key = (n.id, n.count);
            if armed.peek().contains(&key) {
                continue;
            }
            armed.write().insert(key);
            let (id, count) = key;
            spawn(async move {
                architect::platform::sleep(std::time::Duration::from_millis(u64::from(ttl))).await;
                notices.dismiss_if(id, count);
            });
        }
        // Drop bookkeeping for notices that left the queue.
        let live: std::collections::HashSet<u64> = list.iter().map(|n| n.id).collect();
        armed.write().retain(|(id, _)| live.contains(id));
    });

    let list = notices.list();
    if list.is_empty() {
        return rsx! {};
    }
    rsx! {
        div { class: "pointer-events-none fixed bottom-20 right-4 z-50 flex w-80 max-w-[calc(100vw-2rem)] flex-col gap-2 md:bottom-4",
            for n in list {
                {
                    let (accent, icon_cls) = match n.level {
                        architect::NoticeLevel::Error => ("border-l-destructive", "text-destructive"),
                        architect::NoticeLevel::Warning => ("border-l-amber-400", "text-amber-400"),
                        architect::NoticeLevel::Success => ("border-l-emerald-500", "text-emerald-500"),
                        architect::NoticeLevel::Info => ("border-l-border", "text-muted-foreground"),
                    };
                    let count = n.count;
                    rsx! {
                        div {
                            key: "{n.id}",
                            class: "pointer-events-auto flex items-start gap-2.5 rounded-lg border border-border border-l-4 {accent} bg-popover/95 px-3 py-2.5 text-sm text-popover-foreground shadow-lg backdrop-blur",
                            span { class: "mt-0.5 shrink-0 {icon_cls}",
                                match n.level {
                                    architect::NoticeLevel::Error
                                    | architect::NoticeLevel::Warning => rsx! { TriangleAlert { size: 15 } },
                                    architect::NoticeLevel::Success => rsx! { CircleCheck { size: 15 } },
                                    architect::NoticeLevel::Info => rsx! { Info { size: 15 } },
                                }
                            }
                            span { class: "min-w-0 flex-1 break-words", "{n.message}" }
                            if count > 1 {
                                span { class: "shrink-0 rounded-full bg-muted/60 px-1.5 py-0.5 text-[10px] tabular-nums text-muted-foreground",
                                    "×{count}"
                                }
                            }
                            button {
                                class: "shrink-0 text-muted-foreground transition-colors hover:text-foreground",
                                aria_label: "Dismiss",
                                onclick: move |_| notices.dismiss(n.id),
                                "×"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Slim top-center pill shown while the supervised vox connection is
/// down: amber while re-establishing, red once an attempt has failed
/// (hover for the raw error). Boot-time connecting shows nothing —
/// pages render their own skeletons. Flipping back to Ready after an
/// outage drops a success toast.
#[component]
fn ConnectionBanner() -> Element {
    let conn = architect::use_connection::<vox_core::Caller>();
    let notices = architect::use_notifications();
    let mut was_down = use_signal(|| false);
    // Sticky worst-state: the supervisor alternates Failed ↔ Connecting
    // on every retry, which would flip the pill red ↔ amber. Once an
    // attempt has failed during THIS outage, hold the red variant (and
    // its last error, for the hover) until the connection is Ready
    // again.
    let mut last_failure = use_signal(|| None::<String>);

    use_effect(move || {
        let generation = conn.generation();
        match conn.state() {
            architect::ConnectionState::Ready(_) => {
                last_failure.set(None);
                if *was_down.peek() {
                    was_down.set(false);
                    notices.success("Reconnected to your workspace");
                }
            }
            architect::ConnectionState::Connecting if generation == 0 => {}
            architect::ConnectionState::Failed(e) => {
                last_failure.set(Some(e));
                if !*was_down.peek() {
                    was_down.set(true);
                }
            }
            architect::ConnectionState::Connecting => {
                if !*was_down.peek() {
                    was_down.set(true);
                }
            }
        }
    });

    let generation = conn.generation();
    match conn.state() {
        architect::ConnectionState::Ready(_) => return rsx! {},
        architect::ConnectionState::Connecting if generation == 0 => return rsx! {},
        _ => {}
    }
    let (cls, label, title) = match last_failure.read().clone() {
        Some(e) => (
            "border-destructive/40 bg-destructive/15 text-destructive",
            if generation == 0 {
                "Can't reach the server — retrying"
            } else {
                "Connection lost — retrying"
            },
            e,
        ),
        None => (
            "border-amber-400/40 bg-amber-500/15 text-amber-200",
            "Reconnecting to your workspace…",
            "Connection lost — re-establishing".to_string(),
        ),
    };
    rsx! {
        div { class: "pointer-events-none fixed left-1/2 top-2 z-50 -translate-x-1/2",
            div {
                class: "pointer-events-auto flex items-center gap-2 rounded-full border {cls} px-3 py-1.5 text-xs font-medium shadow-lg backdrop-blur",
                title: "{title}",
                span { class: "size-3 shrink-0 animate-spin rounded-full border-2 border-current border-t-transparent" }
                "{label}"
            }
        }
    }
}

#[component]
fn RouteFallback() -> Element {
    rsx! {
        div { class: "flex h-64 items-center justify-center text-sm text-muted-foreground",
            "Loading…"
        }
    }
}

/// Redirect `/` to the user's preferred start page, once per session,
/// when their prefs load (renders nothing). Deep links and manual
/// navigation are never hijacked: the redirect only fires while the
/// current route is still the root and no redirect has happened yet.
#[component]
fn StartPageRedirect() -> Element {
    let prefs = use_context::<crate::prefs::PrefsCtx>().prefs;
    let nav = use_navigator();
    let route = use_route::<Route>();
    let mut done = use_signal(|| false);

    use_effect(move || {
        let target = prefs.read().default_page.clone();
        if *done.peek() || target.is_empty() {
            return;
        }
        // `use_route` in an effect: read once via the captured value —
        // only fire from the root route.
        if matches!(route, Route::HomeRoute {}) {
            done.set(true);
            nav.replace(target.as_str());
        }
    });
    rsx! {}
}
