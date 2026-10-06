//! Two-column desktop layout / single-column mobile layout.
//! Wraps the active route via `Outlet`.

use dioxus::prelude::*;

use crate::chrome::{FleetingFab, FleetingModal, TopBar, provide_chrome_contexts};
use crate::routes::Route;
use crate::shell::mobile::{BottomTabBar, MobileTimerDock};

#[component]
pub fn AppShell() -> Element {
    let current = use_route::<Route>();
    // Boot: the shell has painted (effects run after the first render).
    use_effect(|| task_ui_core::boot_trace::mark("shell"));

    // Quick-capture + data-refresh signals for the persistent chrome.
    provide_chrome_contexts();
    // The docked player a citation's timestamp plays in.
    crate::source_dock::provide_source_dock();
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
    // What the enabled apps put in the right dock. Read here rather
    // than at the mount site because the dock's chrome — the border,
    // the drag handle, the whole column — should not render at all
    // when nothing is going to fill it.
    let enabled = crate::nav::use_active_plugins();
    let docked: Vec<fn() -> Element> = task_plugin_ui::registered()
        .iter()
        .filter(|a| enabled.contains(a.id))
        .filter_map(|a| a.panel)
        .collect();
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
        // Obsidian-shaped: one full-width top
        // bar over everything (sidebar toggles, timer, capture — and
        // where tabs will live), then icon rail → vault explorer →
        // the open view.
        div { class: "min-h-screen bg-background text-foreground md:flex md:h-screen md:flex-col md:overflow-hidden",
            if !chromeless() {
                TopBar {}
            }
            div { class: "md:flex md:min-h-0 md:min-w-0 md:flex-1",
            if !chromeless() {
                div { class: "hidden md:block",
                    crate::shell::rail::IconRail { current: current.clone() }
                }
            }
            // Everything right of the icon rail: the [vault explorer |
            // open view] row, with the IDE status line spanning BENEATH
            // it. The rail runs full-height beside the status bar, but the
            // vault explorer stops just above it (VS Code-style).
            div { class: "flex min-w-0 flex-col md:min-h-0 md:flex-1 md:overflow-hidden",
                div { class: "flex min-w-0 flex-col md:min-h-0 md:flex-1 md:flex-row",
                    if explorer.read().0 && !chromeless() && side_panel_for(&current) != SidePanel::None {
                        div { class: "hidden w-[17rem] shrink-0 border-r border-border/60 md:flex md:min-h-0 md:flex-col md:overflow-hidden",
                            // The column shows the best companion for
                            // where the person is ([`side_panel_for`]).
                            match side_panel_for(&current) {
                                SidePanel::Files => rsx! { files_ui::FilesSidebar {} },
                                SidePanel::Project(id) => rsx! {
                                    crate::shell::project_sidebar::ProjectSidebar { id }
                                },
                                // Inside a wiki the column is THAT wiki's
                                // pages, not the vault's folders — the same
                                // explorer, over the wiki's vault id.
                                SidePanel::Wiki { org, wiki } => rsx! {
                                    crate::shell::explorer::VaultExplorer { org, wiki }
                                },
                                SidePanel::Vault => rsx! { crate::shell::explorer::VaultExplorer {} },
                                SidePanel::Work => rsx! {
                                    crate::shell::work_sidebar::WorkSidebar { current: current.clone() }
                                },
                                SidePanel::None => rsx! {},
                            }
                        }
                    }
                    div { class: "flex min-h-screen min-w-0 flex-col md:min-h-0 md:flex-1 md:overflow-hidden",
                        // Bottom padding keeps content clear of the fixed
                        // tab bar (56px + safe area). On desktop `main` is
                        // the scroll container.
                        main { class: "flex-1 pt-[env(safe-area-inset-top,0px)] pb-[calc(6rem+env(safe-area-inset-bottom,0px))] md:min-h-0 md:overflow-y-auto md:pt-0 md:pb-0",
                            SuspenseBoundary {
                                fallback: |_| rsx! { RouteFallback {} },
                                Outlet::<Route> {}
                            }
                        }
                        if !share {
                            BottomTabBar { current }
                            FleetingFab {}
                            MobileTimerDock {}
                        }
                    }
                    // The right dock: whatever an app puts beside the
                    // center view. Left edge is a drag handle (width
                    // persisted). The shell owns the dock — the toggle,
                    // the width, the resize — and the apps fill it, so
                    // an org with every panel-contributing app turned
                    // off gets no dock rather than an empty one.
                    if agent_panel.read().0 && !chromeless() && !docked.is_empty() {
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
                            for render in docked.clone() {
                                {render()}
                            }
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
        // Nothing mounts (and on the web, nothing downloads) until the
        // first play request; the setlist-row highlighter rides with it.
        task_player_ui::GlobalNowPlayer {}
        // The unified media host: the persistent review player (dock
        // strip ⇄ zoomed review screen, one element) and the
        // one-audible-source rule. Outside the Outlet for the same
        // reason as the engine above — playback survives navigation.
        crate::media_session::MediaHost {}
        // Mobile: no desktop status bar, so float the same tab above the
        // bottom tab bar. Renders nothing until something plays.
        if !share {
            div { class: "md:hidden fixed bottom-[calc(3.5rem+env(safe-area-inset-bottom,0px))] right-2 z-40",
                task_player_ui::NowPlayingTab {}
            }
        }
        // Single global capture modal, toggled from any fleeting button.
        FleetingModal {}
        crate::source_dock::SourceDockView {}
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

/// What the desktop side column holds.
#[derive(Clone, PartialEq, Debug)]
enum SidePanel {
    /// The file manager's own tree — on Files the column IS the app.
    Files,
    /// One project's map (parts, neighbours) on its own page.
    Project(String),
    /// One wiki's pages, inside that wiki.
    Wiki { org: String, wiki: String },
    /// The org's vault tree, where notes are the subject.
    Vault,
    /// Smart lists and active projects, wherever work is planned.
    Work,
    /// Nothing worth a column (settings, people, connections, plugin
    /// apps with their own layout): the view gets the width.
    None,
}

/// The best companion for `route` — the column follows what the person
/// is doing instead of always being the vault.
fn side_panel_for(route: &Route) -> SidePanel {
    match route {
        Route::FilesRoute {} => SidePanel::Files,
        Route::ProjectDetailRoute { id } => SidePanel::Project(id.clone()),
        Route::WikiHomeRoute { org, wiki }
        | Route::WikiDocRoute { org, wiki, .. }
        | Route::WikiRequestRoute { org, wiki, .. }
        | Route::WikiScopedSourcesRoute { org, wiki }
        | Route::WikiScopedSourceRoute { org, wiki, .. } => SidePanel::Wiki {
            org: org.clone(),
            wiki: wiki.clone(),
        },
        Route::VaultRoute { .. }
        | Route::BasesRoute {}
        | Route::GraphRoute { .. }
        | Route::WikiRoute {}
        | Route::WikiPageRoute { .. }
        | Route::WikiSourcesRoute {}
        | Route::WikiSourceRoute { .. } => SidePanel::Vault,
        Route::HomeRoute {}
        | Route::DashboardRoute {}
        | Route::InboxRoute {}
        | Route::ProjectsRoute {}
        | Route::GoalsRoute {}
        | Route::TasksRoute {}
        | Route::TaskDetailRoute { .. }
        | Route::MilestonesRoute {}
        | Route::ScheduleRoute {}
        | Route::GanttRoute {}
        | Route::TimerRoute {} => SidePanel::Work,
        _ => SidePanel::None,
    }
}

/// Where a phone opens when the person has not picked a start page:
/// the task list, whose first section is Today. On a phone the app is
/// for doing the next thing, and the dashboard is a tab away.
const PHONE_START: &str = "/tasks";

/// Redirect `/` to the user's preferred start page, once per session,
/// when their prefs load (renders nothing). With no preference set, a
/// phone-sized screen opens on [`PHONE_START`]; wider screens stay on
/// Home. Deep links and manual navigation are never hijacked: the
/// redirect only fires while the current route is still the root and
/// no redirect has happened yet.
#[component]
fn StartPageRedirect() -> Element {
    let prefs = use_context::<crate::prefs::PrefsCtx>().prefs;
    let nav = use_navigator();
    let route = use_route::<Route>();
    let mut done = use_signal(|| false);

    use_effect(move || {
        let target = prefs.read().default_page.clone();
        // `use_route` in an effect: read once via the captured value —
        // only fire from the root route.
        if *done.peek() || !matches!(route, Route::HomeRoute {}) {
            return;
        }
        if !target.is_empty() {
            done.set(true);
            nav.replace(target.as_str());
            return;
        }
        // The same breakpoint the phone layout (bottom tab bar) uses.
        spawn(async move {
            let phone =
                dioxus::document::eval("return window.matchMedia('(max-width: 640px)').matches;")
                    .await
                    .ok()
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
            if phone && !*done.peek() {
                done.set(true);
                nav.replace(PHONE_START);
            }
        });
    });
    rsx! {}
}

#[cfg(test)]
mod tests {
    use super::{SidePanel, side_panel_for};
    use crate::routes::Route;

    /// The vault tree is for notes; planning screens get the work
    /// column, and screens with no companion give the view the width.
    #[test]
    fn the_side_column_follows_the_screen() {
        assert_eq!(side_panel_for(&Route::TasksRoute {}), SidePanel::Work);
        assert_eq!(side_panel_for(&Route::HomeRoute {}), SidePanel::Work);
        assert_eq!(side_panel_for(&Route::ProjectsRoute {}), SidePanel::Work);
        assert_eq!(
            side_panel_for(&Route::VaultRoute {
                path: String::new(),
                org: String::new()
            }),
            SidePanel::Vault
        );
        assert_eq!(
            side_panel_for(&Route::WikiHomeRoute {
                org: "o".into(),
                wiki: "w".into()
            }),
            SidePanel::Wiki {
                org: "o".into(),
                wiki: "w".into()
            }
        );
        assert_eq!(side_panel_for(&Route::SettingsRoute {}), SidePanel::None);
        assert_eq!(side_panel_for(&Route::FilesRoute {}), SidePanel::Files);
    }
}
