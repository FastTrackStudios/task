//! Persistent app chrome — the desktop top bar plus the quick-capture
//! and timer widgets that live in it (and in the sidebar / bottom bar).
//!
//! Three pieces, all org-scoped off the shared [`OrgSelection`] context:
//!
//! - **Fleeting capture** — a single global modal ([`FleetingModal`])
//!   toggled by a [`FleetingOpen`] context signal, so a lightweight
//!   [`FleetingButton`] anywhere (top bar, sidebar, bottom bar) opens
//!   the same capture form. Captures land in the inbox via
//!   `upsert_inbox_item` (the FLAP "capture" step).
//! - **Timer widget** — [`TimerWidget`]: a compact live clock with
//!   start / stop over the org's `TimerService`, mirroring `/timer`.
//! - **Top bar** — [`TopBar`]: desktop-only sticky header, right
//!   aligned, with at-a-glance stat chips + the two widgets.
//!
//! All three read/write the shared optimistic stores
//! ([`crate::stores`]), so a capture or a timer start made *anywhere*
//! (these widgets, `/inbox`, `/timer`) updates the chips and the clock
//! instantly — no refresh counter, no extra round-trips.

use chrono::Utc;
use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{ChevronDown, Feather, Inbox as InboxIcon, Pause, Play, Square};
use architect_ui::prelude::*;
use uuid::Uuid;

use crate::orgs::{OrgMeta, OrgSelection};
use crate::routes::Route;

// ── shared context ──────────────────────────────────────────────────

/// Visibility of the single global fleeting-capture modal. Provided
/// once by the app shell; flipped by any [`FleetingButton`].
#[derive(Clone, Copy)]
pub struct FleetingOpen(pub Signal<bool>);

/// Visibility of the telescope-style everything-search window
/// (`<space> <space>`, [`crate::search::SearchOverlay`]).
#[derive(Clone, Copy)]
pub struct SearchOpen(pub Signal<bool>);

/// A vault path whose note header should open straight into title-edit
/// mode (text pre-selected) the next time it mounts. Set by the
/// new-note flow so a fresh "Untitled" note can be named by just
/// typing; taken (cleared) by the header when it fires.
#[derive(Clone, Copy)]
pub struct PendingTitleEdit(pub Signal<Option<String>>);

/// Zen mode: hide ALL desktop chrome (top bar, icon rail, explorer,
/// status bar) so the open view gets the full viewport. Toggled by
/// Ctrl+Shift+Z ([`crate::shortcuts`]) and exited from the hover
/// hot-zone ([`ZenExitOverlay`]). Panel open/closed state is left
/// untouched — exiting restores exactly what was showing. Desktop-only
/// by construction: the chrome it hides is `md:`-gated, and mobile has
/// no keyboard to trigger it.
#[derive(Clone, Copy)]
pub struct ZenMode(pub Signal<bool>);

/// Note view mode — Obsidian-shaped: Edit (live editor), Read (reading
/// view; currently renders the same editor — the read design comes with
/// the viewing-experience pass), Raw (the literal text file, MINUS the
/// YAML frontmatter block, which stays in the Properties tab). Toggled
/// from the bottom status bar; app-wide like Obsidian's per-pane default.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ViewMode {
    Edit,
    Read,
    Raw,
}

#[derive(Clone, Copy)]
pub struct NoteViewMode(pub Signal<ViewMode>);

// The player's request channels moved out with the player itself
// (`task_player_ui::context`) — re-exported here so `crate::chrome::NowPlaying`
// and friends resolve at their historical paths.
pub use task_player_ui::{NowPlaying, NowPlayingRequest, SongPlayRequest};

/// Anonymous share-link mode (`?share=1` in the URL — appended by the
/// share landing page's Open button): render NO app chrome at all — no
/// top bar, rail, explorer, tabs, or capture FAB. The visitor gets just
/// the shared view. Read once at boot; guests don't toggle chrome.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct ShareMode(pub bool);

fn detect_share_mode() -> bool {
    #[cfg(target_arch = "wasm32")]
    {
        web_sys::window()
            .and_then(|w| w.location().search().ok())
            .map(|q| q.contains("share=1"))
            .unwrap_or(false)
    }
    #[cfg(not(target_arch = "wasm32"))]
    false
}

/// Install the chrome contexts. Call once in the app shell.
pub fn provide_chrome_contexts() {
    use_context_provider(|| FleetingOpen(Signal::new(false)));
    use_context_provider(|| SearchOpen(Signal::new(false)));
    use_context_provider(|| PendingTitleEdit(Signal::new(None)));
    use_context_provider(|| StatusBarInfo(Signal::new(None)));
    use_context_provider(|| ZenMode(Signal::new(false)));
    use_context_provider(|| ShareMode(detect_share_mode()));
    use_context_provider(|| NoteViewMode(Signal::new(ViewMode::Edit)));
    // Player contexts (SongPlayRequest / NowPlaying / NowPlayingCtl).
    task_player_ui::provide_player_contexts();
    // How an extracted feature page links to a vault note. The router
    // is the shell's, so the shell renders the typed route to a URL and
    // hands the builder down (see `task_ui_core::nav`).
    use_context_provider(|| {
        task_ui_core::nav::NoteHref(Callback::new(|path: String| {
            crate::routes::Route::VaultRoute { path, org: String::new() }.to_string()
        }))
    });
    use_context_provider(|| TimerResumeHint(Signal::new(None)));
}

// ── timer shared state ──────────────────────────────────────────────

/// A just-paused timer's context, held so the idle widget can offer a
/// one-click **Resume**. Set on Pause, cleared on Stop or a fresh
/// Start. Provided at the shell so it survives route navigation.
#[derive(Clone, PartialEq)]
pub struct PausedTimer {
    pub slug: String,
    pub org_id: Uuid,
    pub req: timer_proto::StartTimerRequest,
    pub title: String,
    /// Seconds already banked before the pause — shown frozen so the
    /// paused pill reads the accumulated time, not zero.
    pub banked: i64,
}

/// Shell-level "paused timer" hint. `None` when nothing is paused.
#[derive(Clone, Copy)]
pub struct TimerResumeHint(pub Signal<Option<PausedTimer>>);

pub(crate) fn use_resume_hint() -> Signal<Option<PausedTimer>> {
    use_context::<TimerResumeHint>().0
}

/// The single running work session for the active org + owner, if one
/// is open. Derived from the shared session store, so the top-bar
/// widget, the `/timer` page, and the task-list banner all agree on
/// exactly what's tracking right now.
#[derive(Clone, PartialEq)]
pub struct ActiveTimer {
    pub slug: String,
    pub org_id: Uuid,
    pub session: timer_proto::WorkSession,
}

pub(crate) fn use_active_timer() -> Option<ActiveTimer> {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let session_rows = crate::stores::use_session_list();
    let (slug, org_id) = resolve_org(&selection.read(), &org_list.read())?;
    let owner = owner_id(org_id);
    session_rows.value().and_then(|rows| {
        rows.iter()
            .map(|(_, r)| r)
            .find(|r| r.slug == slug && r.session.user_id == owner && r.session.end_time.is_none())
            .map(|r| ActiveTimer {
                slug: slug.clone(),
                org_id,
                session: r.session.clone(),
            })
    })
}

/// Rebuild a start request from an existing session — used by pause →
/// resume and by switch, which both re-open "the same work".
pub(crate) fn req_from_session(
    org_id: Uuid,
    s: &timer_proto::WorkSession,
) -> timer_proto::StartTimerRequest {
    timer_proto::StartTimerRequest {
        user_id: s.user_id,
        org_id,
        project_id: s.project_id,
        project_path: s.project_path.clone(),
        task_note_path: s.task_note_path.clone(),
        description: s.description.clone(),
    }
}

/// The label a session shows in the timer chrome — its description, or
/// a neutral fallback when it was started bare.
pub(crate) fn session_title(s: &timer_proto::WorkSession) -> String {
    if s.description.trim().is_empty() {
        "Tracking".to_string()
    } else {
        s.description.clone()
    }
}

/// The only chrome visible in zen mode: an invisible ~24px hot-zone
/// fixed to the top-left corner that reveals a small exit button on
/// hover (pure CSS group-hover). Clicking it leaves zen; the tooltip
/// names the keyboard toggle.
#[component]
pub fn ZenExitOverlay() -> Element {
    let mut zen = use_context::<ZenMode>().0;
    rsx! {
        div {
            // The hot-zone is larger than the button so a rough flick
            // into the corner catches; only the button paints.
            class: "group fixed left-0 top-0 z-50 hidden h-10 w-10 md:block",
            button {
                r#type: "button",
                class: "m-1.5 flex h-7 w-7 items-center justify-center rounded-md border border-border bg-card/90 text-muted-foreground opacity-0 shadow-md backdrop-blur transition-opacity hover:text-foreground focus-visible:opacity-100 group-hover:opacity-100",
                title: "Exit zen (Ctrl+Shift+Z)",
                aria_label: "Exit zen mode",
                onclick: move |_| zen.set(false),
                architect_ui::lucide_dioxus::Minimize2 { size: 14 }
            }
        }
    }
}

/// What the open document contributes to the bottom status bar —
/// written by the vault page (an effect keeps it current), cleared on
/// unmount. `None` = no document context (the bar shows only the
/// app-level segments).
#[derive(Clone, PartialEq)]
pub struct DocStatus {
    /// Vault-relative path of the open file.
    pub file: String,
    /// Unsaved changes?
    pub dirty: bool,
    /// Save-cycle message ("Saving…", "Saved", error) — empty = idle.
    pub save: String,
    /// Collab session state ("live" / "connecting…"), when attached.
    pub collab: Option<String>,
    /// Vim mode label ("NORMAL", "INSERT", …) when vim is active.
    pub vim: Option<String>,
    /// Explicit-save hook — renders a Save affordance in the bar.
    pub on_save: Option<Callback<()>>,
}

/// Status-bar context: `Signal<Option<DocStatus>>`, provided app-wide.
#[derive(Clone, Copy)]
pub struct StatusBarInfo(pub Signal<Option<DocStatus>>);

/// IDE-style bottom status line (Zed-shaped): document segments on the
/// left (file · dirty dot · save state · collab · vim mode), app-level
/// state on the right (connection badge). Desktop-only — mobile keeps
/// its action bar + tab bar.
#[component]
pub fn StatusBar() -> Element {
    let info = use_context::<StatusBarInfo>().0;
    let doc = info.read().clone();
    rsx! {
        div { class: "relative z-20 hidden h-6 shrink-0 items-center gap-3 border-t border-border bg-card/60 px-3 text-[11px] text-muted-foreground md:flex",
            if let Some(d) = doc {
                if let Some(vim) = d.vim {
                    span { class: "rounded bg-accent px-1.5 font-semibold uppercase tracking-wide text-foreground",
                        "{vim}"
                    }
                }
                span { class: "min-w-0 truncate", "{d.file}" }
                if d.dirty {
                    span { class: "size-1.5 shrink-0 rounded-full bg-primary", title: "Unsaved changes" }
                }
                if !d.save.is_empty() {
                    span { class: "shrink-0", "{d.save}" }
                }
                if let Some(save) = d.on_save {
                    if d.dirty {
                        button {
                            r#type: "button",
                            class: "shrink-0 rounded px-1 text-foreground hover:bg-accent/50",
                            onclick: move |_| save.call(()),
                            "Save"
                        }
                    }
                }
                if let Some(c) = d.collab {
                    span { class: "shrink-0", "Collab: {c}" }
                }
            }
            div { class: "ml-auto flex items-center gap-3",
                ViewModeToggle {}
                crate::presence::ConnectionBadge {}
            }
            // Now Playing tab — parked ~2/3 into the note view (before the
            // right sidebar), poking up out of the status line. Absolute so
            // it doesn't shift the status segments. Renders nothing until
            // something plays.
            div { class: "absolute bottom-0 left-2/3 z-30",
                task_player_ui::NowPlayingTab {}
            }
        }
    }
}

/// The Edit | Read | Raw segment control in the status bar.
#[component]
fn ViewModeToggle() -> Element {
    let mut mode = use_context::<NoteViewMode>().0;
    let current = mode();
    rsx! {
        div { class: "flex items-center overflow-hidden rounded border border-border",
            for (m, label) in [
                (ViewMode::Edit, "Edit"),
                (ViewMode::Read, "Read"),
                (ViewMode::Raw, "Raw"),
            ] {
                button {
                    r#type: "button",
                    class: if current == m {
                        "bg-accent px-1.5 py-0.5 font-semibold text-foreground"
                    } else {
                        "px-1.5 py-0.5 text-muted-foreground hover:text-foreground"
                    },
                    onclick: move |_| mode.set(m),
                    "{label}"
                }
            }
        }
    }
}

/// Shell panel state: the vault explorer (left) — toggled from the
/// top bar, Obsidian-style.
#[derive(Clone, Copy, PartialEq)]
pub struct ExplorerOpen(pub bool);

/// Shell panel state: the right (backlinks/context) panel.
#[derive(Clone, Copy, PartialEq)]
pub struct RightPanelOpen(pub bool);

/// Shell panel state: the right AGENT sidebar (conversations +
/// embedded chat) — vault on the left, agents on the right.
#[derive(Clone, Copy, PartialEq)]
pub struct AgentPanelOpen(pub bool);

/// The agent panel's open conversation (session id; empty = list
/// only). Lives in chrome state so the selection survives panel
/// toggles and route changes.
#[derive(Clone, PartialEq)]
pub struct AgentPanelSelected(pub String);

/// Agent panel width in px (drag-resizable, persisted to
/// localStorage — CodexMonitor persists its panel sizes too).
#[derive(Clone, Copy, PartialEq)]
pub struct AgentPanelWidth(pub f64);

pub(crate) fn use_fleeting_open() -> Signal<bool> {
    use_context::<FleetingOpen>().0
}

// ── top bar ─────────────────────────────────────────────────────────

/// Desktop-only sticky top bar: right-aligned stat chips + the
/// fleeting-capture button + the timer widget. Hidden on mobile, where
/// the bottom bar carries the fleeting button instead.
#[component]
pub fn TopBar() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();

    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));

    // Both chips derive from the shared stores, so a capture or a
    // stopped timer made anywhere updates them instantly (the widgets
    // below mutate the same stores).
    let inbox_rows = crate::stores::use_inbox_list();
    let session_rows = crate::stores::use_session_list();

    // Open inbox items still to review (excludes processed / archived
    // and not-yet-due snoozes), for the at-a-glance chip.
    let today = Utc::now().date_naive().to_string();
    let inbox_count = inbox_rows.value().map_or(0, |rows| {
        rows.iter()
            .filter(|(_, it)| {
                it.is_open()
                    && it
                        .resurface_on
                        .as_deref()
                        .is_none_or(|d| d <= today.as_str())
            })
            .count()
    });

    // Time logged today (completed sessions only — the running one is
    // shown live in the timer widget), scoped to the active org+owner.
    let today_date = Utc::now().date_naive();
    let logged: i64 = match (target(), session_rows.value()) {
        (Some((slug, org_id)), Some(rows)) => {
            let owner = owner_id(org_id);
            rows.iter()
                .map(|(_, r)| r)
                .filter(|r| {
                    r.slug == slug
                        && r.session.user_id == owner
                        && r.session.start_time.date_naive() == today_date
                })
                .filter_map(|r| {
                    r.session
                        .end_time
                        .map(|e| (e - r.session.start_time).num_seconds())
                })
                .sum()
        }
        _ => 0,
    };

    // Obsidian-shaped window bar: sidebar toggle at the far left,
    // the tab strip in the middle (real route tabs — crate::tabs),
    // actions + the right-panel toggle at the far right.
    let mut explorer = use_context::<Signal<ExplorerOpen>>();
    let mut right_panel = use_context::<Signal<RightPanelOpen>>();
    let mut agent_panel = use_context::<Signal<AgentPanelOpen>>();

    rsx! {
        div {
            class: "z-20 hidden h-10 shrink-0 items-center gap-2 border-b border-border bg-card/60 px-2 md:flex",
            button {
                r#type: "button",
                class: "flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent/50 hover:text-foreground",
                title: "Toggle vault sidebar",
                onclick: move |_| {
                    let cur = explorer.peek().0;
                    explorer.set(ExplorerOpen(!cur));
                },
                architect_ui::lucide_dioxus::PanelLeft { size: 15 }
            }
            // The tab strip — Obsidian-style route tabs.
            crate::tabs::TabStrip {}

            StatChip {
                icon: rsx! { InboxIcon { size: 14 } },
                label: "{inbox_count}",
                route: Route::InboxRoute {},
            }
            StatChip {
                icon: rsx! { architect_ui::lucide_dioxus::Clock { size: 14 } },
                label: "Today {fmt_hms(logged)}",
                route: Route::TimerRoute {},
            }

            div { class: "mx-1 h-5 w-px bg-border" }

            FleetingButton { compact: true }
            TimerWidget {}

            // Who's here — avatar group opening the full roster.
            crate::presence::PresenceAvatarBar {}

            NotificationBell {}

            button {
                r#type: "button",
                class: if agent_panel.read().0 {
                    "ml-1 flex h-7 w-7 items-center justify-center rounded-md bg-accent text-foreground"
                } else {
                    "ml-1 flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                },
                title: "Toggle agent sidebar",
                onclick: move |_| {
                    let cur = agent_panel.peek().0;
                    agent_panel.set(AgentPanelOpen(!cur));
                },
                architect_ui::lucide_dioxus::Bot { size: 15 }
            }
            button {
                r#type: "button",
                class: "ml-1 flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent/50 hover:text-foreground",
                title: "Toggle right panel",
                onclick: move |_| {
                    let cur = right_panel.peek().0;
                    right_panel.set(RightPanelOpen(!cur));
                },
                architect_ui::lucide_dioxus::PanelRight { size: 15 }
            }
        }
    }
}

/// A small clickable stat pill that navigates to `route`.
#[component]
fn StatChip(icon: Element, label: String, route: Route) -> Element {
    rsx! {
        Link {
            to: route,
            class: "flex items-center gap-1.5 rounded-full border border-border bg-card/40 px-2.5 py-1 text-xs text-muted-foreground transition-colors hover:text-foreground",
            span { class: "flex h-3.5 w-3.5 items-center justify-center", {icon} }
            span { "{label}" }
        }
    }
}

// ── fleeting capture ────────────────────────────────────────────────

/// A button that opens the global fleeting-capture modal. `compact`
/// renders icon-only (for the top bar); otherwise icon + label (for
/// the sidebar / bottom bar).
#[component]
pub fn FleetingButton(#[props(default = false)] compact: bool) -> Element {
    let mut open = use_fleeting_open();
    if compact {
        rsx! {
            button {
                class: "flex items-center gap-1.5 rounded-lg bg-primary px-2.5 py-1.5 text-sm font-medium text-primary-foreground transition-colors hover:bg-primary/80",
                title: "Capture a fleeting note",
                onclick: move |_| open.set(true),
                Feather { size: 15 }
                span { class: "hidden xl:inline", "Capture" }
            }
        }
    } else {
        rsx! {
            button {
                class: "flex w-full items-center gap-2 rounded-lg bg-primary/10 px-3 py-2 text-sm font-medium text-primary transition-colors hover:bg-primary/20",
                onclick: move |_| open.set(true),
                span { class: "flex h-4 w-4 items-center justify-center", Feather { size: 16 } }
                span { "Fleeting note" }
            }
        }
    }
}

/// The single global capture modal. Render once (in the app shell). A
/// textarea + Capture; Enter submits, Shift+Enter newlines, Esc / click
/// outside cancels. Captures into the active org's inbox.
#[component]
pub fn FleetingModal() -> Element {
    let mut open = use_fleeting_open();
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));
    let muts = crate::stores::use_inbox_mutations();

    let mut draft = use_signal(String::new);

    if !open() {
        return rsx! {};
    }

    // Optimistic: the capture lands in the shared inbox store
    // instantly (chip + /inbox update), persists in the background,
    // and rolls back + notifies on failure.
    let mut submit = move || {
        let text = draft.peek().trim().to_string();
        if text.is_empty() {
            open.set(false);
            return;
        }
        let Some((slug, _)) = target() else {
            return;
        };
        let id = uuid::Uuid::new_v4().to_string();
        let created = Utc::now().to_rfc3339();
        muts.capture(
            slug,
            inbox_proto::InboxItem::capture(id, text, "ui", created),
        );
        draft.set(String::new());
        open.set(false);
    };

    rsx! {
        div {
            class: "fixed inset-0 z-50 flex items-start justify-center bg-black/40 p-4 pt-[12vh]",
            onclick: move |_| open.set(false),
            div {
                class: "flex w-full max-w-lg flex-col gap-3 rounded-xl border border-border bg-card p-4 shadow-xl",
                onclick: move |e| e.stop_propagation(),
                div { class: "flex items-center gap-2 text-sm text-muted-foreground",
                    Feather { size: 15 }
                    span { "Fleeting note" }
                    span { class: "ml-auto text-xs", "↵ to capture · esc to close" }
                }
                textarea {
                    class: "min-h-[7rem] w-full resize-none rounded-lg border border-input bg-input/30 px-3 py-2 text-sm outline-none focus-visible:border-ring focus-visible:ring-[3px] focus-visible:ring-ring/50 placeholder:text-muted-foreground",
                    placeholder: "Get it out of your head…",
                    // `autofocus` only fires on initial page load, not when
                    // the modal is inserted dynamically — focus on mount.
                    onmounted: move |e: Event<MountedData>| {
                        spawn(async move {
                            let _ = e.data().set_focus(true).await;
                        });
                    },
                    value: "{draft}",
                    oninput: move |e| draft.set(e.value()),
                    onkeydown: move |e| {
                        if e.key() == Key::Enter && !e.modifiers().contains(Modifiers::SHIFT) {
                            e.prevent_default();
                            submit();
                        } else if e.key() == Key::Escape {
                            open.set(false);
                        }
                    },
                }
                div { class: "flex items-center justify-end gap-2",
                    Button {
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        on_click: move |_| open.set(false),
                        "Cancel"
                    }
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        on_click: move |_| submit(),
                        "Capture"
                    }
                }
            }
        }
    }
}

/// Mobile floating action button for fleeting capture — a circular
/// button pinned bottom-right above the tab bar. Hidden on desktop
/// (the top bar + sidebar carry the capture button there).
#[component]
pub fn FleetingFab() -> Element {
    let mut open = use_fleeting_open();
    rsx! {
        button {
            class: "fixed bottom-24 right-4 z-30 flex h-12 w-12 items-center justify-center rounded-full bg-primary text-primary-foreground shadow-lg transition-transform active:scale-95 md:hidden",
            style: "margin-bottom: env(safe-area-inset-bottom, 0px);",
            title: "Capture a fleeting note",
            onclick: move |_| open.set(true),
            Feather { size: 20 }
        }
    }
}

// ── timer widget ────────────────────────────────────────────────────

/// Compact live timer control mirroring `/timer` and the task-list
/// banner (all three read the same [`use_active_timer`] source, so they
/// never disagree). Three states:
///
/// - **Running** — pulsing sky dot, task/description, live clock, and
///   Switch / Pause / Stop controls. Switch opens a task picker and
///   atomically moves the clock to the chosen task ([`TimerMutations::switch`]).
/// - **Paused** — an amber pill with the banked time frozen and a
///   one-click Resume. Pause closes the session (the segment is logged)
///   but stashes its context in [`TimerResumeHint`] so resuming re-opens
///   the same work.
/// - **Idle** — a description input + Start.
#[component]
pub fn TimerWidget() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));

    let muts = crate::stores::use_timer_mutations();
    let active = use_active_timer();
    let mut hint = use_resume_hint();

    // Live clock — re-render once a second so the running elapsed advances.
    let tick = use_signal(|| 0u64);
    use_second_tick(tick);
    let _ = tick();

    let mut draft = use_signal(String::new);
    let mut switching = use_signal(|| false);
    let mut menu_open = use_signal(|| false);

    // Fresh start from the idle input — clears any paused work.
    let mut start = move || {
        let Some((slug, org_id)) = target() else {
            return;
        };
        let desc = draft.peek().trim().to_string();
        draft.set(String::new());
        hint.set(None);
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
    };

    let Some((cur_slug, cur_org)) = target() else {
        return rsx! {};
    };

    match active.as_ref() {
        // ── Running ──────────────────────────────────────────────
        Some(at) => {
            let s = &at.session;
            let elapsed = (Utc::now() - s.start_time).num_seconds();
            let title = session_title(s);
            let slug = at.slug.clone();
            let org_id = at.org_id;
            let sid = s.id;
            // Pause: log the segment (stop) but remember the work so the
            // idle pill can resume it.
            let paused = PausedTimer {
                slug: slug.clone(),
                org_id,
                req: req_from_session(org_id, s),
                title: title.clone(),
                banked: elapsed.max(0),
            };
            let pause = {
                let slug = slug.clone();
                move |_: MouseEvent| {
                    muts.stop(slug.clone(), owner_id(org_id), sid);
                    hint.set(Some(paused.clone()));
                }
            };
            let stop = {
                let slug = slug.clone();
                move |_: MouseEvent| {
                    muts.stop(slug.clone(), owner_id(org_id), sid);
                    hint.set(None);
                }
            };
            let on_switch = {
                let slug = slug.clone();
                move |req: timer_proto::StartTimerRequest| {
                    muts.switch(slug.clone(), sid, req);
                    hint.set(None);
                    switching.set(false);
                }
            };
            rsx! {
                div { class: "relative flex items-center gap-2 rounded-lg border border-sky-500/40 bg-sky-500/10 py-1 pl-2.5 pr-1.5",
                    span { class: "relative flex size-2 shrink-0",
                        span { class: "absolute inline-flex size-full animate-ping rounded-full bg-sky-400/70" }
                        span { class: "relative inline-flex size-2 rounded-full bg-sky-400" }
                    }
                    span { class: "max-w-[10rem] truncate text-xs text-foreground", "{title}" }
                    span { class: "font-mono text-sm font-semibold tabular-nums text-sky-300", "{fmt_hms(elapsed)}" }
                    button {
                        class: "flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-sky-500/20 hover:text-sky-300",
                        title: "Switch task",
                        onclick: move |_| {
                            let cur = *switching.peek();
                            switching.set(!cur);
                        },
                        ChevronDown { size: 14 }
                    }
                    button {
                        class: "flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-amber-500/15 hover:text-amber-400",
                        title: "Pause timer",
                        onclick: pause,
                        Pause { size: 14 }
                    }
                    button {
                        class: "flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive",
                        title: "Stop timer",
                        onclick: stop,
                        Square { size: 14 }
                    }
                    if switching() {
                        TimerMenu {
                            slug,
                            org_id,
                            placeholder: "Switch to task…".to_string(),
                            on_pick: on_switch,
                            on_close: move |_| switching.set(false),
                        }
                    }
                }
            }
        }
        None => match hint.read().clone() {
            // ── Paused ───────────────────────────────────────────
            Some(p) => {
                let resume = {
                    let p = p.clone();
                    move |_: MouseEvent| {
                        muts.start(p.slug.clone(), p.req.clone());
                        hint.set(None);
                    }
                };
                rsx! {
                    div { class: "flex items-center gap-2 rounded-lg border border-amber-500/40 bg-amber-500/10 py-1 pl-2.5 pr-1.5",
                        span { class: "flex shrink-0 items-center text-amber-400", Pause { size: 12 } }
                        span { class: "max-w-[9rem] truncate text-xs text-foreground", "{p.title}" }
                        span { class: "font-mono text-sm font-semibold tabular-nums text-amber-300/90", "{fmt_hms(p.banked)}" }
                        span { class: "text-[10px] uppercase tracking-wide text-muted-foreground", "paused" }
                        button {
                            class: "flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-sky-500/20 hover:text-sky-300",
                            title: "Resume timer",
                            onclick: resume,
                            Play { size: 14 }
                        }
                        button {
                            class: "flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-destructive/10 hover:text-destructive",
                            title: "Discard paused timer",
                            onclick: move |_| hint.set(None),
                            Square { size: 14 }
                        }
                    }
                }
            }
            // ── Idle ─────────────────────────────────────────────
            None => {
                let start_req = {
                    let slug = cur_slug.clone();
                    move |req: timer_proto::StartTimerRequest| {
                        hint.set(None);
                        menu_open.set(false);
                        muts.start(slug.clone(), req);
                    }
                };
                rsx! {
                    div { class: "relative flex items-center gap-1 rounded-lg border border-border bg-card/40 py-1 pl-2.5 pr-1",
                        input {
                            class: "w-32 bg-transparent text-xs outline-none placeholder:text-muted-foreground focus:w-44 xl:w-40 xl:focus:w-56",
                            placeholder: "Start a timer…",
                            value: "{draft}",
                            oninput: move |e| draft.set(e.value()),
                            onkeydown: move |e| {
                                if e.key() == Key::Enter {
                                    start();
                                }
                            },
                        }
                        button {
                            class: "flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-sky-500/15 hover:text-sky-300",
                            title: "Pick a task or recent timer",
                            onclick: move |_| {
                                let cur = *menu_open.peek();
                                menu_open.set(!cur);
                            },
                            ChevronDown { size: 14 }
                        }
                        button {
                            class: "flex h-6 w-6 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-sky-500/15 hover:text-sky-300",
                            title: "Start timer",
                            onclick: move |_| start(),
                            Play { size: 14 }
                        }
                        if menu_open() {
                            TimerMenu {
                                slug: cur_slug.clone(),
                                org_id: cur_org,
                                placeholder: "Start a task…".to_string(),
                                on_pick: start_req,
                                on_close: move |_| menu_open.set(false),
                            }
                        }
                    }
                }
            }
        },
    }
}

/// Task/timer picker popover — the launcher shown from the idle widget
/// and the switch control on a running one. Lists today's **recent**
/// timers (one-click resume) and fuzzy-searches the active org's
/// **open tasks**; picking either emits a fully-formed
/// [`StartTimerRequest`](timer_proto::StartTimerRequest) that the caller
/// starts (idle) or switches to (running).
#[component]
fn TimerMenu(
    slug: String,
    org_id: Uuid,
    #[props(default = "Search tasks…".to_string())] placeholder: String,
    on_pick: EventHandler<timer_proto::StartTimerRequest>,
    on_close: EventHandler<()>,
) -> Element {
    let mut query = use_signal(String::new);
    let tasks = crate::stores::use_task_list();
    let projects = crate::stores::use_project_list();
    let sessions = crate::stores::use_session_list();

    let searching = !query().trim().is_empty();

    // Recent timers today (distinct by description + task), newest first
    // — hidden while searching so the query only filters tasks.
    let recents: Vec<TimerPick> = if searching {
        Vec::new()
    } else {
        let owner = owner_id(org_id);
        let today = Utc::now().date_naive();
        let mut seen = std::collections::HashSet::new();
        sessions
            .value()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(_, r)| r)
            .filter(|r| r.slug == slug && r.session.user_id == owner)
            .filter(|r| r.session.start_time.date_naive() == today)
            .filter_map(|r| {
                let s = r.session;
                let title = session_title(&s);
                if !seen.insert((s.description.clone(), s.task_note_path.clone())) {
                    return None;
                }
                Some(TimerPick {
                    key: format!("recent-{}", s.id),
                    title,
                    task_note_path: s.task_note_path,
                    project_id: s.project_id,
                    project_path: s.project_path,
                    project_title: None,
                })
            })
            .take(5)
            .collect()
    };

    // Open tasks: fuzzy-ranked while searching, else the first few.
    let task_picks: Vec<TimerPick> = {
        let q = query();
        let q = q.trim();
        let project_rows = projects.value().cloned().unwrap_or_default();
        let mut scored: Vec<(i64, crate::stores::OrgTask)> = tasks
            .value()
            .cloned()
            .unwrap_or_default()
            .into_iter()
            .map(|(_, row)| row)
            .filter(|row| row.slug == slug)
            .filter(|row| is_open_status(&row.task.status))
            .filter_map(|row| {
                if q.is_empty() {
                    Some((0, row))
                } else {
                    crate::fuzzy::fuzzy_match(q, &row.task.title).map(|score| (score, row))
                }
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| a.1.task.title.cmp(&b.1.task.title))
        });
        scored.truncate(8);
        scored
            .into_iter()
            .map(|(_, row)| {
                let project = row
                    .task
                    .project_id
                    .and_then(|pid| project_rows.iter().map(|(_, p)| p).find(|p| p.project.id == pid));
                TimerPick {
                    key: format!("task-{}", row.task.id),
                    title: row.task.title.clone(),
                    task_note_path: row.task.path.clone(),
                    project_id: row.task.project_id,
                    project_path: project.map(|p| p.project.path.clone()).unwrap_or_default(),
                    project_title: project.map(|p| p.project.title.clone()),
                }
            })
            .collect()
    };

    let empty = recents.is_empty() && task_picks.is_empty();
    let row = move |p: TimerPick| {
        rsx! {
            button {
                key: "{p.key}",
                r#type: "button",
                class: "flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-xs transition-colors hover:bg-accent",
                onclick: move |_| {
                    on_pick.call(timer_proto::StartTimerRequest {
                        user_id: owner_id(org_id),
                        org_id,
                        project_id: p.project_id,
                        project_path: p.project_path.clone(),
                        task_note_path: p.task_note_path.clone(),
                        description: p.title.clone(),
                    });
                },
                span { class: "min-w-0 truncate text-foreground", "{p.title}" }
                if let Some(project) = p.project_title.as_ref() {
                    span { class: "shrink-0 rounded bg-muted px-1.5 py-px text-[10px] text-muted-foreground",
                        "#{project}"
                    }
                }
            }
        }
    };

    rsx! {
        // Backdrop — click-away closes the menu.
        div {
            class: "fixed inset-0 z-30",
            onclick: move |_| on_close.call(()),
        }
        div { class: "absolute right-0 top-full z-40 mt-1 flex w-72 flex-col overflow-hidden rounded-lg border border-border bg-popover shadow-xl",
            div { class: "border-b border-border/60 p-2",
                input {
                    class: "w-full rounded-md border border-border bg-background px-2.5 py-1.5 text-xs outline-none focus:ring-2 focus:ring-sky-500/40",
                    placeholder: "{placeholder}",
                    autofocus: true,
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                }
            }
            if empty {
                div { class: "px-3 py-3 text-xs text-muted-foreground", "No open tasks match." }
            } else {
                div { class: "flex max-h-72 flex-col overflow-y-auto",
                    if !recents.is_empty() {
                        div { class: "px-3 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground", "Recent" }
                        for p in recents.clone() {
                            {row(p)}
                        }
                    }
                    if !task_picks.is_empty() {
                        div { class: "px-3 pb-1 pt-2 text-[10px] font-semibold uppercase tracking-wide text-muted-foreground", "Tasks" }
                        for p in task_picks.clone() {
                            {row(p)}
                        }
                    }
                }
            }
        }
    }
}

/// One menu row — a recent timer or an open task, ready to (re)start.
#[derive(Clone, PartialEq)]
struct TimerPick {
    key: String,
    title: String,
    task_note_path: String,
    project_id: Option<Uuid>,
    project_path: String,
    project_title: Option<String>,
}

/// Statuses the switch picker treats as "still workable" (mirrors the
/// `/timer` page's open-task filter).
fn is_open_status(status: &str) -> bool {
    !matches!(
        status,
        "done" | "complete" | "completed" | "cancelled" | "shipped"
    )
}

// ── shared helpers (also used by pages::timer) ─────────────────────

/// Resolve the selection to `(slug, org_id)`: the chosen org in `One`
/// mode, else the home org. `None` until the org list (with ids) loads.
pub(crate) fn resolve_org(sel: &OrgSelection, orgs: &[OrgMeta]) -> Option<(String, Uuid)> {
    let meta = match sel {
        OrgSelection::One(slug) => orgs.iter().find(|o| &o.slug == slug),
        OrgSelection::All => orgs.iter().find(|o| o.is_home).or_else(|| orgs.first()),
    }?;
    Some((meta.slug.clone(), meta.id?))
}

/// Stable per-org "local owner" user id — a single-user stand-in until
/// auth threads a real signed-in user id through. Deterministic so
/// start / stop / list all key on the same user, and shared with the
/// CLI and the watch bridge via `timer_proto::owner`.
pub(crate) use timer_proto::local_owner_id as owner_id;

/// `HH:MM:SS` from a (possibly negative, clamped) second count.
pub(crate) fn fmt_hms(secs: i64) -> String {
    let s = secs.max(0);
    format!("{:02}:{:02}:{:02}", s / 3600, (s % 3600) / 60, s % 60)
}

/// Re-render once a second so running clocks advance. Wasm sleeps via
/// `gloo-timers`; native parks (chrome is web-only today).
pub(crate) fn use_second_tick(mut tick: Signal<u64>) {
    use_future(move || async move {
        loop {
            sleep_one_second().await;
            tick += 1;
        }
    });
}

#[cfg(target_arch = "wasm32")]
async fn sleep_one_second() {
    gloo_timers::future::TimeoutFuture::new(1000).await;
}

#[cfg(not(target_arch = "wasm32"))]
async fn sleep_one_second() {
    futures_util::future::pending::<()>().await;
}

// ── notification bell ───────────────────────────────────────────────

/// The bell: unread badge + dropdown of recent notifications, fed by
/// the shared notification store (fetch-once-then-fold off the
/// `Notify` events stream, so the badge is live). Clicking a row
/// marks it read and navigates to its source `href`; the header
/// offers mark-all-read. Rows are written server-side by the
/// notifier — this surface only reads and flips read-state.
#[component]
pub fn NotificationBell() -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));
    let rows = crate::stores::use_notification_list();
    let store = crate::stores::use_notification_store();
    let mut open = use_signal(|| false);

    let mut items: Vec<notify_proto::Notification> = rows
        .value()
        .map(|r| r.iter().map(|(_, n)| n.clone()).collect())
        .unwrap_or_default();
    items.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    let unread = items.iter().filter(|n| n.read_at.is_none()).count();
    items.truncate(15);

    let mark_all = move |_: MouseEvent| {
        let Some((slug, _)) = target() else { return };
        // Optimistic: flip every visible unread now; the stream's
        // folds re-assert the server truth row by row.
        let now = Utc::now();
        if let Some(rows) = rows.value() {
            for (_, n) in rows.iter() {
                if n.read_at.is_none() {
                    let mut read = n.clone();
                    read.read_at = Some(now);
                    store.put(read);
                }
            }
        }
        spawn(async move {
            if let Err(e) = crate::feeds::mark_all_notifications_read(&slug).await {
                tracing::warn!(error = %e, "mark all notifications read failed");
            }
        });
    };

    rsx! {
        div { class: "relative",
            button {
                r#type: "button",
                class: if open() {
                    "relative flex h-7 w-7 items-center justify-center rounded-md bg-accent text-foreground"
                } else {
                    "relative flex h-7 w-7 items-center justify-center rounded-md text-muted-foreground hover:bg-accent/50 hover:text-foreground"
                },
                title: "Notifications",
                aria_label: "Notifications",
                onclick: move |_| {
                    let cur = *open.peek();
                    open.set(!cur);
                },
                architect_ui::lucide_dioxus::Bell { size: 15 }
                if unread > 0 {
                    span {
                        class: "absolute -right-0.5 -top-0.5 flex h-3.5 min-w-3.5 items-center justify-center rounded-full bg-primary px-0.5 text-[9px] font-semibold leading-none text-primary-foreground",
                        {if unread > 99 { "99+".to_string() } else { unread.to_string() }}
                    }
                }
            }
            if open() {
                // Backdrop — click-away closes the panel.
                div {
                    class: "fixed inset-0 z-30",
                    onclick: move |_| open.set(false),
                }
                div { class: "absolute right-0 top-full z-40 mt-1 flex w-80 flex-col overflow-hidden rounded-lg border border-border bg-popover shadow-xl",
                    div { class: "flex items-center justify-between border-b border-border/60 px-3 py-2",
                        span { class: "text-xs font-semibold text-foreground", "Notifications" }
                        if unread > 0 {
                            button {
                                r#type: "button",
                                class: "text-[11px] text-muted-foreground hover:text-foreground",
                                onclick: mark_all,
                                "Mark all read"
                            }
                        }
                    }
                    if items.is_empty() {
                        div { class: "px-3 py-6 text-center text-xs text-muted-foreground",
                            "Nothing yet — task completions, agent turns and bookings land here."
                        }
                    } else {
                        div { class: "flex max-h-96 flex-col overflow-y-auto",
                            for n in items.clone() {
                                NotificationRow { n, on_open: move |_| open.set(false) }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// One dropdown row. Click = mark read (optimistically, then over the
/// wire) + navigate to the source href + close the panel.
#[component]
fn NotificationRow(n: notify_proto::Notification, on_open: EventHandler<()>) -> Element {
    let selection = use_context::<Signal<OrgSelection>>();
    let org_list = use_context::<Signal<Vec<OrgMeta>>>();
    let target = use_memo(move || resolve_org(&selection.read(), &org_list.read()));
    let store = crate::stores::use_notification_store();
    let nav = use_navigator();

    let unread = n.read_at.is_none();
    let when = fmt_rel_time(n.created_at);
    let row = n.clone();
    let click = move |_: MouseEvent| {
        if row.read_at.is_none() {
            let mut read = row.clone();
            read.read_at = Some(Utc::now());
            store.put(read);
            if let Some((slug, _)) = target() {
                let id = row.id;
                spawn(async move {
                    if let Err(e) = crate::feeds::mark_notification_read(&slug, id).await {
                        tracing::warn!(error = %e, "mark notification read failed");
                    }
                });
            }
        }
        if !row.source.href.is_empty() {
            nav.push(row.source.href.as_str());
        }
        on_open.call(());
    };

    rsx! {
        button {
            r#type: "button",
            class: "flex w-full items-start gap-2 border-b border-border/40 px-3 py-2 text-left transition-colors last:border-b-0 hover:bg-accent",
            onclick: click,
            span {
                class: if unread {
                    "mt-1.5 size-1.5 shrink-0 rounded-full bg-primary"
                } else {
                    "mt-1.5 size-1.5 shrink-0 rounded-full bg-transparent"
                },
            }
            div { class: "min-w-0 flex-1",
                div { class: "truncate text-xs font-medium text-foreground", "{n.title}" }
                if !n.body.is_empty() {
                    div { class: "truncate text-[11px] text-muted-foreground", "{n.body}" }
                }
            }
            span { class: "shrink-0 pt-px text-[10px] text-muted-foreground", "{when}" }
        }
    }
}

/// Compact relative timestamp for the bell ("now", "5m", "2h", "3d",
/// else the date).
fn fmt_rel_time(t: chrono::DateTime<Utc>) -> String {
    let secs = (Utc::now() - t).num_seconds().max(0);
    match secs {
        0..60 => "now".to_string(),
        60..3600 => format!("{}m", secs / 60),
        3600..86_400 => format!("{}h", secs / 3600),
        86_400..604_800 => format!("{}d", secs / 86_400),
        _ => t.date_naive().to_string(),
    }
}
