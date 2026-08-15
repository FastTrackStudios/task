//! `/agents` — the agent command center.
//!
//! Three panes, CodexMonitor/t3code-inspired:
//!
//! - **Session rail** — searchable, pinned-first, hover actions,
//!   priority status pills, hash-colored subagent chips.
//! - **Chat pane** — a derived row timeline ([`timeline`]): turn
//!   folds ("3 tool calls · 1 failed · 0:42") above settled
//!   assistant messages, live activity rows that expand to the
//!   call's arguments and result, streaming markdown, and a status
//!   row naming the tool in flight. Composer with queue-while-busy
//!   sends, `/` + `$` ranked autocomplete, a model chip fed by
//!   Discovery, a CSS-only context ring, and running spend.
//! - **Inspector** — session meta + actions, live backend health,
//!   skill library, capability flags.
//!
//! Two health signals sit in the header: the **event lane** (this
//! UI's subscription, which reconnects on its own with backoff) and
//! the **backend** ([`GatewayChip`], polled from
//! `Discovery::backend_health`). They fail independently — the lane
//! can drop while the gateway is fine, and vice versa — so both are
//! shown rather than collapsed into one "online" dot.
//!
//! All decidable behavior lives in [`logic`] / [`timeline`] as
//! pure, tested functions (the t3code `*.logic.ts` pattern); the
//! components are thin `rsx!` over them.

pub(crate) mod logic;
mod timeline;

use std::collections::HashMap;

use agent_proto::event::AgentEvent;
use agent_proto::message::{ContentBlock, Message, Role};
use agent_proto::question::QuestionRequest;
use agent_proto::service::discovery::{CapabilityFlag, ModelInfo, SkillInfo};
use agent_proto::session::{Session, SessionStatus};
use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{Bot, ChevronLeft, Copy, FileText, Info, Trash2};
use architect_ui::prelude::*;

use logic::{
    PromptHistory, Recall, ScrollMode, autoscroll_js, context_free_percent, context_ring_style,
    cost_badge, fmt_cost, fmt_duration, fmt_elapsed, fmt_tokens, group_models, rank_by,
    referenced_paths, relative_time, scroll_mode, scroll_to_end_js, status_pill,
};
use timeline::{ActivityLine, Row, ToolTone, TurnLog, push_line, running_tool, settle_tool};

/// The agent surface's visual vocabulary.
///
/// This screen had drifted to six type sizes below `text-sm`, four
/// border opacities and nine padding values picked ad hoc, which is
/// what made it read as unconsidered. Three sizes, one border, one
/// rhythm — named here so the discipline is visible and hard to
/// drift from again. The one structural edge weight is
/// `border-border/60`, used inline.
///
/// The transcript is a **work log**, not a messaging app: every turn
/// changes the user's real tasks, calendar and notes. So turns hang
/// off a single hairline rail ([`RAIL`]) that carries chronology and
/// causality down the left edge, and nothing else competes with it.
mod style {
    /// Message prose — the only text read continuously.
    pub const BODY: &str = "text-sm leading-relaxed";
    /// Controls, metadata, secondary rows.
    pub const UI: &str = "text-xs";
    /// Chips, counters, timestamps. Used sparingly.
    pub const MICRO: &str = "text-[11px]";
    /// Section eyebrows.
    pub const EYEBROW: &str =
        "text-[11px] font-semibold uppercase tracking-[0.14em] text-muted-foreground";
    /// The turn spine every activity row hangs from.
    pub const RAIL: &str = "border-l border-border/60 pl-3";
    /// Keyboard focus, applied to every interactive control.
    pub const FOCUS: &str =
        "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary/50";
}

/// Duty-cycled pulse (t3code: `steps()` timing keeps the
/// compositor cheap) + halo styling utility classes can't express.
const AGENTS_CSS: &str = r#"
@keyframes agents-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.35; }
}
.agents-dot-pulse {
  animation: agents-pulse 1.2s steps(6) infinite;
  box-shadow: 0 0 0 3px color-mix(in srgb, currentColor 25%, transparent);
}
@media (prefers-reduced-motion: reduce) {
  .agents-dot-pulse { animation: none; }
}
"#;

#[component]
pub fn AgentsView(session: String) -> Element {
    let selection = use_context::<Signal<crate::orgs::OrgSelection>>();
    let org_list = use_context::<Signal<Vec<crate::orgs::OrgMeta>>>();
    let nav = use_navigator();

    let mut sessions = use_resource(move || async move {
        let slugs = crate::orgs::selected_slugs(&selection.read(), &org_list.read());
        if slugs.is_empty() {
            return Ok(Vec::new());
        }
        crate::feeds::fetch_agent_sessions(&slugs).await
    });

    let active = use_memo(move || {
        crate::orgs::selected_slugs(&selection.read(), &org_list.read())
            .into_iter()
            .next()
    });

    // Discovery — live models / skills / capabilities for the active org.
    let models = use_resource(move || async move {
        match active() {
            Some(s) => crate::feeds::fetch_agent_models(&s)
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        }
    });
    let skills = use_resource(move || async move {
        match active() {
            Some(s) => crate::feeds::fetch_agent_skills(&s)
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        }
    });
    let capabilities = use_resource(move || async move {
        match active() {
            Some(s) => crate::feeds::fetch_agent_capabilities(&s)
                .await
                .unwrap_or_default(),
            None => Vec::new(),
        }
    });
    let model_list = models.read().clone().unwrap_or_default();
    let skill_list = skills.read().clone().unwrap_or_default();
    let cap_list = capabilities.read().clone().unwrap_or_default();

    let mut selected = use_signal(|| None::<(String, Session)>);
    // Touch devices open the inspector on demand — as a sheet it
    // covers the conversation, so it must not be the landing state.
    let touch = use_hook(editor::editor_view::coarse_pointer);
    let mut show_inspector = use_signal(move || !touch);
    let mut create_error = use_signal(String::new);

    // Resolve the routed session id against the fetched sessions —
    // the explorer's Agents section owns the conversation list now.
    use_effect(use_reactive!(|(session,)| {
        let target = session.clone();
        if target.is_empty() {
            selected.set(None);
            return;
        }
        let rows: Vec<(String, Session)> = match &*sessions.read() {
            Some(Ok(rows)) => rows.clone(),
            _ => return,
        };
        selected.set(rows.into_iter().find(|(_, s)| s.id == target));
    }));

    let on_new_chat = move |_| {
        let Some(slug) = active() else { return };
        spawn(async move {
            match crate::feeds::create_agent_session(&slug, "", "").await {
                Ok(s) => {
                    create_error.set(String::new());
                    nav.push(crate::routes::Route::AgentsRoute {
                        session: s.id.clone(),
                    });
                    selected.set(Some((slug, s)));
                    sessions.restart();
                }
                Err(e) => create_error.set(e),
            }
        });
    };

    let fetch_err = match &*sessions.read_unchecked() {
        Some(Err(e)) => e.clone(),
        _ => String::new(),
    };
    let mut session_rows: Vec<(String, Session)> = match &*sessions.read_unchecked() {
        Some(Ok(rows)) => rows.iter().filter(|(_, s)| !s.archived).cloned().collect(),
        _ => Vec::new(),
    };
    session_rows.sort_by(|(_, a), (_, b)| {
        let ka = a.last_message_at.unwrap_or(a.created_at);
        let kb = b.last_message_at.unwrap_or(b.created_at);
        kb.cmp(&ka)
    });

    let mutate = use_callback(move |(slug, id, action): (String, String, SessionAction)| {
        spawn(async move {
            let res: Result<(), String> = match action {
                SessionAction::Pin(v) => crate::feeds::pin_agent_session(&slug, &id, v)
                    .await
                    .map(|_| ()),
                SessionAction::Archive(v) => crate::feeds::archive_agent_session(&slug, &id, v)
                    .await
                    .map(|_| ()),
                SessionAction::Rename(t) => crate::feeds::rename_agent_session(&slug, &id, &t)
                    .await
                    .map(|_| ()),
                SessionAction::Delete => crate::feeds::delete_agent_session(&slug, &id).await,
            };
            if let Err(e) = res {
                create_error.set(e);
            } else {
                let touched_open = matches!(
                    selected.peek().as_ref(),
                    Some((_, s)) if s.id == id
                );
                if touched_open {
                    if let Ok(s) =
                        crate::feeds::fetch_agent_sessions(std::slice::from_ref(&slug)).await
                    {
                        match s.into_iter().find(|(_, s)| s.id == id) {
                            Some(row) => selected.set(Some(row)),
                            None => {
                                selected.set(None);
                                nav.push(crate::routes::Route::AgentsRoute {
                                    session: String::new(),
                                });
                            }
                        }
                    }
                }
                sessions.restart();
            }
        });
    });

    rsx! {
        style { {AGENTS_CSS} }
        div { class: "flex h-full min-h-0 w-full",
            // ── Chat pane ──
            if let Some((slug, session)) = selected.read().clone() {
                ChatPane {
                    key: "{session.id}",
                    slug,
                    session,
                    models: model_list.clone(),
                    skills: skill_list.clone(),
                    inspector_open: show_inspector(),
                    on_toggle_inspector: move |()| {
                        let v = *show_inspector.peek();
                        show_inspector.set(!v);
                    },
                    on_activity: move |()| sessions.restart(),
                }
            } else {
                // Mobile has no agent sidebar, so the conversation list
                // is the page until you pick one (master/detail).
                div { class: "flex min-h-0 flex-1 flex-col gap-1 overflow-y-auto p-3 md:hidden",
                    for (slug , s) in session_rows.iter() {
                        {
                            let target = s.id.clone();
                            let title = if s.title.trim().is_empty() {
                                "(untitled)".to_string()
                            } else {
                                s.title.clone()
                            };
                            let when = relative_time(s.last_message_at.unwrap_or(s.created_at));
                            let pill = status_pill(s.status);
                            let row = (slug.clone(), s.clone());
                            rsx! {
                                button {
                                    key: "m-{s.id}",
                                    r#type: "button",
                                    class: "flex min-h-11 w-full items-center gap-2 rounded-lg border border-border/60 bg-card/30 px-3 py-2 text-left active:bg-accent/40",
                                    onclick: move |_| {
                                        selected.set(Some(row.clone()));
                                        nav.push(crate::routes::Route::AgentsRoute {
                                            session: target.clone(),
                                        });
                                    },
                                    span { class: "min-w-0 flex-1 truncate text-sm text-foreground", "{title}" }
                                    if let Some(p) = &pill {
                                        span { class: "h-2 w-2 shrink-0 rounded-full {p.dot}", title: "{p.label}" }
                                    }
                                    span { class: "shrink-0 {style::MICRO} tabular-nums text-muted-foreground", "{when}" }
                                }
                            }
                        }
                    }
                    Button {
                        variant: ButtonVariant::Primary,
                        disabled: active().is_none(),
                        on_click: on_new_chat,
                        "New chat"
                    }
                    if !fetch_err.is_empty() {
                        div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs leading-relaxed",
                            "Can't reach the agent service. {fetch_err}"
                        }
                    }
                }
                div { class: "hidden flex-1 flex-col items-center justify-center gap-3 text-center md:flex",
                    Bot { size: 32 }
                    Heading { level: HeadingLevel::H3, "Chat with your agents" }
                    Text { variant: TextVariant::Muted, class: "max-w-sm text-sm leading-relaxed",
                        "Ask about your tasks, calendar, or notes — the agent can read and change them. Pick a conversation from the sidebar, or start a new one."
                    }
                    Button {
                        variant: ButtonVariant::Primary,
                        disabled: active().is_none(),
                        on_click: on_new_chat,
                        "New chat"
                    }
                    if !create_error.read().is_empty() {
                        div { class: "rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs",
                            "{create_error}"
                        }
                    }
                    if !fetch_err.is_empty() {
                        div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs leading-relaxed",
                            "Can't reach the agent service. {fetch_err}"
                        }
                    }
                }
            }

            // ── Inspector ──
            // Desktop: a column beside the chat. Mobile: the same
            // content in a bottom sheet — a 288px column on a 390px
            // phone left the conversation with nothing to live in.
            if show_inspector() {
                if let Some((slug, session)) = selected.read().clone() {
                    div { class: "hidden md:flex md:min-h-0",
                        Inspector {
                            key: "insp-{session.id}",
                            slug: slug.clone(),
                            session: session.clone(),
                            skills: skill_list.clone(),
                            capabilities: cap_list.clone(),
                            mutate,
                        }
                    }
                    crate::shell::mobile::BottomSheet {
                        open: true,
                        title: "Session".to_string(),
                        on_close: move |()| show_inspector.set(false),
                        Inspector {
                            key: "insp-sheet-{session.id}",
                            slug,
                            session,
                            skills: skill_list.clone(),
                            capabilities: cap_list.clone(),
                            mutate,
                        }
                    }
                }
            }
        }
    }
}

/// One composer autocomplete row.
#[derive(Clone, PartialEq)]
struct CompletionRow {
    insert: String,
    label: String,
    detail: String,
}

/// Hermes slash commands the gateway understands (v0.19). Curated —
/// the gateway has no commands-discovery endpoint yet; keep in sync
/// with `hermes --help` when bumping the input.
const HERMES_COMMANDS: &[(&str, &str)] = &[
    (
        "/new",
        "Start a fresh context (clears the session's history)",
    ),
    ("/model", "Show or switch the model for this session"),
    ("/skills", "List the agent's learned skills"),
    (
        "/learn",
        "Teach the agent a new skill from this conversation",
    ),
    ("/journey", "Show what the agent has learned over time"),
    ("/compact", "Compress older context into a summary"),
    ("/status", "Agent + backend status"),
    ("/memory", "Inspect the agent's persistent memory"),
    ("/tools", "List available toolsets"),
    ("/help", "List available commands"),
];

/// The trigger token under the caret-at-end heuristic: `/` only as
/// the very first token (commands), `$` on the last whitespace-
/// separated token (skills). Returns (mode, query, token_start).
fn completion_trigger(text: &str) -> Option<(char, String, usize)> {
    if let Some(rest) = text.strip_prefix('/') {
        if !rest.contains(char::is_whitespace) {
            return Some(('/', rest.to_lowercase(), 0));
        }
    }
    let start = text.rfind(char::is_whitespace).map_or(0, |i| {
        i + text[i..].chars().next().map_or(1, char::len_utf8)
    });
    let token = &text[start..];
    if let Some(q) = token.strip_prefix('$') {
        return Some(('$', q.to_lowercase(), start));
    }
    None
}

/// Rail/inspector session mutations.
#[derive(Clone, PartialEq)]
enum SessionAction {
    Pin(bool),
    Archive(bool),
    Rename(String),
    Delete,
}

/// Display label for a tool call — the backend's own summary when
/// it supplied one, else the bare tool name.
fn tool_label(tool_call: &agent_proto::tool::ToolCall) -> String {
    if tool_call.title.is_empty() {
        tool_call.name.clone()
    } else {
        tool_call.title.clone()
    }
}

/// Re-indent a JSON argument blob for the expanded tool row. Non-JSON
/// (or empty) payloads pass through untouched — some tools take a
/// bare string.
fn pretty_json(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "{}" {
        return String::new();
    }
    serde_json::from_str::<serde_json::Value>(trimmed)
        .ok()
        .and_then(|v| serde_json::to_string_pretty(&v).ok())
        .unwrap_or_else(|| trimmed.to_string())
}

/// Backend chip styling — Hermes gets the primary accent.
fn backend_chip_cls(backend_id: &str) -> &'static str {
    if backend_id == "hermes" {
        "shrink-0 rounded-full bg-primary/15 px-1.5 text-[11px] text-primary"
    } else {
        "shrink-0 rounded-full bg-muted/60 px-1.5 text-[11px] text-muted-foreground"
    }
}

/// The two-pane model picker (hermes-desktop's chat dropdown):
/// provider rail with count badges + an All row, searched flat model
/// list with context/cost/reasoning badges, active model floated
/// first. Selection is session-only.
#[component]
fn ModelPicker(
    models: Vec<ModelInfo>,
    current: String,
    on_pick: EventHandler<ModelInfo>,
) -> Element {
    let mut open = use_signal(|| false);
    let mut search = use_signal(String::new);
    let mut provider = use_signal(|| None::<String>);

    let current_label = if current.is_empty() {
        "default model".to_string()
    } else {
        models
            .iter()
            .find(|m| m.id == current)
            .map(|m| {
                if m.label.is_empty() {
                    m.id.clone()
                } else {
                    m.label.clone()
                }
            })
            .unwrap_or_else(|| current.clone())
    };

    let groups = group_models(&models, &current);
    let total: usize = groups.iter().map(|(_, _, ms)| ms.len()).sum();
    let q = search.read().to_lowercase();
    let visible: Vec<ModelInfo> = groups
        .iter()
        .filter(|(pid, _, _)| provider.read().as_ref().is_none_or(|sel| sel == pid))
        .flat_map(|(_, _, ms)| ms.iter().cloned())
        .filter(|m| {
            q.is_empty() || m.label.to_lowercase().contains(&q) || m.id.to_lowercase().contains(&q)
        })
        .collect();

    rsx! {
        div { class: "relative",
            button {
                r#type: "button",
                class: "flex min-h-8 items-center gap-1 rounded-md border border-border/60 bg-card/30 px-2 py-1 text-xs text-foreground hover:border-primary/60 md:min-h-0 md:py-0.5 {style::FOCUS}",
                onclick: move |_| {
                    let v = *open.peek();
                    open.set(!v);
                    if !v {
                        search.set(String::new());
                    }
                },
                span { class: "max-w-48 truncate", "{current_label}" }
                span { class: "text-muted-foreground", "▾" }
            }
            if open() {
                div { class: "absolute bottom-full left-0 z-40 mb-1 flex h-[70vh] w-[min(34rem,calc(100vw-1.5rem))] flex-col overflow-hidden rounded-lg border border-border bg-popover shadow-lg md:h-80",
                    input {
                        class: "m-2 rounded-md border border-border/60 bg-card/30 px-2 py-2 text-base text-foreground outline-none focus:border-primary/60 md:py-1 md:text-xs",
                        placeholder: "Search models…",
                        autofocus: true,
                        value: "{search}",
                        oninput: move |e| search.set(e.value()),
                        onkeydown: move |e| {
                            if e.key() == Key::Escape {
                                open.set(false);
                            }
                        },
                    }
                    div { class: "flex min-h-0 flex-1 flex-col md:flex-row",
                        // Provider rail — a row of chips on mobile, a
                        // column beside the list on desktop.
                        div { class: "flex shrink-0 gap-0.5 overflow-x-auto border-b border-border/60 p-1 md:w-40 md:flex-col md:overflow-x-hidden md:overflow-y-auto md:border-b-0 md:border-r",
                            button {
                                r#type: "button",
                                class: if provider.read().is_none() {
                                    "flex shrink-0 items-center justify-between gap-1 rounded-md bg-accent px-2 py-1.5 text-left text-xs md:py-1"
                                } else {
                                    "flex shrink-0 items-center justify-between gap-1 rounded-md px-2 py-1.5 text-left text-xs hover:bg-accent/50 md:py-1"
                                },
                                onclick: move |_| provider.set(None),
                                span { "All models" }
                                span { class: "text-[11px] text-muted-foreground", "{total}" }
                            }
                            for (pid , pname , ms) in groups.iter() {
                                {
                                    let pid_cl = pid.clone();
                                    let is_sel = provider.read().as_deref() == Some(pid.as_str());
                                    rsx! {
                                        button {
                                            key: "{pid}",
                                            r#type: "button",
                                            class: if is_sel {
                                                "flex shrink-0 items-center justify-between gap-1 rounded-md bg-accent px-2 py-1.5 text-left text-xs md:py-1"
                                            } else {
                                                "flex shrink-0 items-center justify-between gap-1 rounded-md px-2 py-1.5 text-left text-xs hover:bg-accent/50 md:py-1"
                                            },
                                            onclick: move |_| {
                                                // Toggle back to All on re-click.
                                                if provider.peek().as_deref() == Some(pid_cl.as_str()) {
                                                    provider.set(None);
                                                } else {
                                                    provider.set(Some(pid_cl.clone()));
                                                }
                                            },
                                            span { class: "truncate", "{pname}" }
                                            span { class: "text-[11px] text-muted-foreground", "{ms.len()}" }
                                        }
                                    }
                                }
                            }
                        }
                        // Model list.
                        div { class: "flex min-h-0 flex-1 flex-col gap-0.5 overflow-y-auto p-1",
                            if visible.is_empty() {
                                div { class: "px-2 py-4 text-center text-xs text-muted-foreground", "No models match." }
                            }
                            for m in visible.iter() {
                                {
                                    let is_active = m.id == current || (current.is_empty() && m.is_default);
                                    let picked = m.clone();
                                    let title_txt = if m.label.is_empty() { m.id.clone() } else { m.label.clone() };
                                    let sub = format!("{} · {}", m.provider_name, m.id);
                                    let ctx = (m.context_length > 0).then(|| fmt_tokens(m.context_length));
                                    let cost = cost_badge(m.cost_in_per_mtok, m.cost_out_per_mtok);
                                    rsx! {
                                        button {
                                            key: "{m.id}",
                                            r#type: "button",
                                            class: if is_active {
                                                "flex w-full flex-col rounded-md bg-accent px-2 py-2 text-left md:py-1"
                                            } else {
                                                "flex w-full flex-col rounded-md px-2 py-2 text-left hover:bg-accent/50 md:py-1"
                                            },
                                            onclick: move |_| {
                                                on_pick.call(picked.clone());
                                                open.set(false);
                                            },
                                            div { class: "flex items-center gap-1.5",
                                                span { class: "truncate text-xs font-medium text-foreground", "{title_txt}" }
                                                if is_active {
                                                    span { class: "text-emerald-500", "✓" }
                                                }
                                                span { class: "ml-auto flex shrink-0 items-center gap-1 text-[11px] text-muted-foreground",
                                                    if m.reasoning {
                                                        span { class: "rounded-full bg-purple-500/15 px-1 text-purple-400", title: "Reasoning model", "R" }
                                                    }
                                                    if let Some(c) = &ctx {
                                                        span { title: "Context window", "{c}" }
                                                    }
                                                    if let Some(c) = &cost {
                                                        span { title: "$ per Mtok in/out", "{c}" }
                                                    }
                                                }
                                            }
                                            span { class: "truncate text-[11px] text-muted-foreground", "{sub}" }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    div { class: "border-t border-border/60 px-2 py-1 text-[11px] text-muted-foreground/70",
                        "Catalog models switch the Hermes session via /model — review + send the prefilled command."
                    }
                }
            }
        }
    }
}

/// Live event-stream health chip state.
#[derive(Clone, PartialEq)]
enum StreamState {
    Connecting,
    /// Connected. The count is how many times we've had to
    /// reconnect — non-zero is worth showing.
    Live(u32),
    /// Disconnected, retrying in `secs`.
    Retrying {
        why: String,
        secs: u32,
    },
}

/// The backend's own status, polled from `Discovery::backend_health`.
/// Renders as a chip whose tooltip carries the detail (state,
/// connected platforms, in-flight agents, probe latency).
#[component]
pub(crate) fn GatewayChip(slug: String, backend_id: String) -> Element {
    // Poll rather than subscribe: health is cheap, changes slowly,
    // and a dead gateway can't push us an event saying so.
    let mut health = use_resource(use_reactive!(|(slug,)| async move {
        crate::feeds::fetch_agent_health(&slug).await
    }));
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        loop {
            architect::platform::sleep(std::time::Duration::from_secs(20)).await;
            tick += 1;
        }
    });
    use_effect(move || {
        let _ = tick();
        health.restart();
    });

    let row = health.read().as_ref().and_then(|r| {
        r.as_ref()
            .ok()
            .and_then(|rows| rows.iter().find(|h| h.backend_id == backend_id).cloned())
    });
    let Some(h) = row else {
        return rsx! {};
    };

    let mut detail = Vec::new();
    if !h.state.is_empty() {
        detail.push(h.state.clone());
    }
    if h.active_agents > 0 {
        detail.push(format!("{} agent(s) running", h.active_agents));
    }
    if !h.platforms.is_empty() {
        detail.push(format!("via {}", h.platforms.join(", ")));
    }
    if h.last_ping_ms > 0 {
        detail.push(format!("{}ms", h.last_ping_ms));
    }
    if !h.status_text.is_empty() {
        detail.push(h.status_text.clone());
    }
    let tooltip = detail.join(" · ");

    if !h.reachable {
        return rsx! {
            span {
                class: "shrink-0 rounded-full bg-destructive/15 px-1.5 {style::MICRO} text-destructive",
                title: "{tooltip}",
                "{backend_id} unreachable"
            }
        };
    }
    // Reachable and quiet is the normal case — stay out of the way,
    // but keep the tooltip and a busy count when work is in flight.
    rsx! {
        span {
            class: "shrink-0 rounded-full bg-muted/50 px-1.5 {style::MICRO} text-muted-foreground",
            title: "{tooltip}",
            if h.active_agents > 0 {
                "{backend_id} · {h.active_agents} busy"
            } else {
                "{backend_id} ok"
            }
        }
    }
}

/// One open conversation. Keyed by session id — remounting on
/// selection change gives each session its own subscription
/// lifecycle.
#[component]
pub(crate) fn ChatPane(
    slug: String,
    session: Session,
    models: Vec<ModelInfo>,
    skills: Vec<SkillInfo>,
    inspector_open: bool,
    on_toggle_inspector: EventHandler<()>,
    on_activity: EventHandler<()>,
) -> Element {
    let session_id = session.id.clone();
    // Touch changes real behaviour here, not just sizing — see the
    // composer's Enter handling and the 16px input rule below.
    let touch = use_hook(editor::editor_view::coarse_pointer);
    let mut messages = use_signal(Vec::<Message>::new);
    let streaming = use_signal(|| None::<(String, String)>);
    let reasoning = use_signal(String::new);
    // Live activity for the CURRENT turn; folded into `turns` on
    // completion (keyed by the concluding assistant message id).
    let live_lines = use_signal(Vec::<ActivityLine>::new);
    let turns = use_signal(HashMap::<String, TurnLog>::new);
    let expanded_folds = use_signal(std::collections::HashSet::<String>::new);
    let mut error = use_signal(String::new);
    let mut busy = use_signal(|| matches!(session.status, SessionStatus::Running));
    let mut composer = use_signal(String::new);
    // Queue-while-busy sends (CodexMonitor's queue intent).
    let mut queued = use_signal(Vec::<String>::new);
    // Pending structured question (numbered-shortcut cards).
    let mut pending_question = use_signal(|| None::<QuestionRequest>);
    let mut completion_sel = use_signal(|| 0usize);
    let mut completion_dismissed = use_signal(String::new);
    let mut model = use_signal(String::new);
    let mut responding = use_signal(String::new);
    let stream_state = use_signal(|| StreamState::Connecting);
    // Set while a cancel is in flight; cleared by the terminal event.
    let mut stopping = use_signal(|| false);
    // Shell-style `↑`/`↓` recall, restored from localStorage per
    // conversation.
    let mut history = use_signal(PromptHistory::default);
    // Whether the reader is following the tail; drives Jump to latest.
    let mut scroll = use_signal(|| ScrollMode::FollowingEnd);
    // How many times the event stream has had to reconnect this
    // session — the health chip shows it once it's non-zero.
    let reconnects = use_signal(|| 0u32);
    let tokens = use_signal(|| (session.usage.input_tokens, session.usage.output_tokens));
    let spend = use_signal(|| session.usage.estimated_cost_usd);
    let turn_started = use_signal(|| None::<chrono::DateTime<chrono::Utc>>);
    let mut elapsed = use_signal(|| 0i64);

    // Restore this conversation's prompt history.
    let history_key = format!("task.agent.history.{session_id}");
    use_future({
        let key = history_key.clone();
        move || {
            let key = key.clone();
            async move {
                let mut js = dioxus::document::eval(&format!(
                    "dioxus.send(localStorage.getItem('{key}') || '[]');"
                ));
                if let Ok(raw) = js.recv::<String>().await {
                    if let Ok(entries) = serde_json::from_str::<Vec<String>>(&raw) {
                        history.set(PromptHistory::from_entries(entries));
                    }
                }
            }
        }
    });

    // 1s ticker driving the "Working… 0:42" row.
    use_future(move || async move {
        loop {
            architect::platform::sleep(std::time::Duration::from_secs(1)).await;
            if let Some(t0) = *turn_started.peek() {
                elapsed.set((chrono::Utc::now() - t0).num_seconds().max(0));
            }
        }
    });

    // The send path — used by the composer, queue drain, and
    // question cards.
    let send_slug = slug.clone();
    let send_sid = session_id.clone();
    let dispatch_text = use_callback(move |text: String| {
        messages.write().push(Message {
            id: format!("local-{}", chrono::Utc::now().timestamp_micros()),
            session_id: send_sid.clone(),
            role: Role::User,
            content: vec![ContentBlock::Text { text: text.clone() }],
            partial: false,
            errored: false,
            error_text: String::new(),
            reasoning: None,
            created_at: chrono::Utc::now(),
        });
        busy.set(true);
        error.set(String::new());
        let slug = send_slug.clone();
        let sid = send_sid.clone();
        let model_override = model.peek().trim().to_string();
        spawn(async move {
            match crate::feeds::dispatch_agent_turn(&slug, &sid, &text, &model_override).await {
                Ok(ack) => {
                    let with = if ack.effective_model.is_empty() {
                        format!("{} · default model", ack.effective_backend_id)
                    } else {
                        format!("{} · {}", ack.effective_backend_id, ack.effective_model)
                    };
                    responding.set(with);
                }
                Err(e) => {
                    busy.set(false);
                    error.set(format!("Dispatch failed: {e}"));
                }
            }
        });
    });

    // ── Transcript hydrate + live event fold ──────────────────
    // `Subscriptions`' `#[subscribe]` stream, consumed through
    // `architect::use_stream` — it owns the reconnect + backoff that
    // used to be a hand-rolled loop here.
    //
    // The subscribe future runs on every (re)connect and re-pulls the
    // transcript first: that is both the initial hydrate and the
    // recovery path for events published while we were detached
    // (fetch-once-then-fold, per the `AgentEventEnvelope` contract —
    // subscribe first so nothing is missed in between).
    let stream_slug = slug.clone();
    let stream_sid = session_id.clone();
    let filter_sid = session_id.clone();
    let resync_slug = slug.clone();
    let resync_sid = session_id.clone();
    architect::use_stream(
        move |tx| {
            let slug = stream_slug.clone();
            let sid = stream_sid.clone();
            let (mut messages, mut error, mut stream_state, mut reconnects) =
                (messages, error, stream_state, reconnects);
            let (mut busy, mut stopping, mut streaming) = (busy, stopping, streaming);
            async move {
                match crate::feeds::fetch_agent_messages(&slug, &sid).await {
                    Ok(mut msgs) => {
                        msgs.reverse();
                        messages.set(msgs);
                    }
                    Err(e) => error.set(format!("Couldn't load the transcript: {e}")),
                }
                let client = match crate::vox_clients::establish_for::<
                    agent_proto::service::subscriptions::SubscriptionsStreamClient,
                >(&slug)
                .await
                {
                    Ok(c) => c,
                    Err(e) => {
                        stream_state.set(StreamState::Retrying { why: e, secs: 0 });
                        return false;
                    }
                };
                stream_state.set(StreamState::Live(*reconnects.peek()));
                let outcome = client.events(tx).await;
                // The lane closed. A turn that was in flight is no
                // longer being reported, so stop the spinner; the hook
                // backs off and re-runs this future.
                reconnects += 1;
                stream_state.set(StreamState::Retrying {
                    why: match outcome {
                        Ok(()) => "the server closed the event stream".to_string(),
                        Err(e) => format!("{e:?}"),
                    },
                    secs: 0,
                });
                busy.set(false);
                stopping.set(false);
                streaming.set(None);
                true
            }
        },
        move |envelope: agent_proto::AgentEventEnvelope| {
            // One stream carries every session the backend runs —
            // keep this chat's own.
            if envelope.session_id != filter_sid {
                return;
            }
            let (mut busy, mut stopping, mut error, mut reasoning, mut live_lines) =
                (busy, stopping, error, reasoning, live_lines);
            let (mut turn_started, mut elapsed, mut streaming, mut messages) =
                (turn_started, elapsed, streaming, messages);
            let (mut pending_question, mut tokens, mut spend, mut turns) =
                (pending_question, tokens, spend, turns);
            let (mut responding, mut queued) = (responding, queued);
            let (slug, sid) = (resync_slug.clone(), resync_sid.clone());
            match envelope.event {
                AgentEvent::TurnStarted { at, .. } => {
                    busy.set(true);
                    stopping.set(false);
                    error.set(String::new());
                    reasoning.set(String::new());
                    live_lines.set(Vec::new());
                    turn_started.set(Some(at));
                    elapsed.set(0);
                    on_activity.call(());
                }
                AgentEvent::MessageWritten { message } => {
                    if streaming
                        .peek()
                        .as_ref()
                        .is_some_and(|(id, _)| *id == message.id)
                    {
                        streaming.set(None);
                    }
                    let mut list = messages.write();
                    if let Some(existing) = list.iter_mut().find(|m| m.id == message.id) {
                        *existing = message;
                    } else if matches!(message.role, Role::User)
                        && list.last().is_some_and(|m| {
                            m.id.starts_with("local-") && text_of(m) == text_of(&message)
                        })
                    {
                        *list.last_mut().expect("non-empty") = message;
                    } else {
                        list.push(message);
                    }
                }
                AgentEvent::MessageDelta {
                    message_id,
                    content_delta,
                    ..
                } => {
                    let mut cur = streaming.write();
                    match cur.as_mut() {
                        Some((id, text)) if *id == message_id => text.push_str(&content_delta),
                        _ => *cur = Some((message_id, content_delta)),
                    }
                }
                AgentEvent::ReasoningDelta { delta, .. } => {
                    reasoning.write().push_str(&delta);
                }
                AgentEvent::ToolStarted { tool_call } => {
                    push_line(
                        &mut live_lines.write(),
                        ActivityLine {
                            tone: ToolTone::Running,
                            text: tool_label(&tool_call),
                            tool_id: tool_call.id.clone(),
                            args: pretty_json(&tool_call.input_json),
                            output: String::new(),
                            duration_ms: 0,
                        },
                    );
                }
                AgentEvent::ToolFinished { tool_call } => {
                    let ok = !matches!(tool_call.status, agent_proto::tool::ToolStatus::Error);
                    settle_tool(
                        &mut live_lines.write(),
                        &tool_call.id,
                        tool_label(&tool_call),
                        ok,
                        tool_call.duration_ms,
                        if tool_call.output_json.is_empty() {
                            tool_call.preview.clone()
                        } else {
                            tool_call.output_json.clone()
                        },
                    );
                }
                AgentEvent::ToolProgress { preview, .. } => {
                    push_line(&mut live_lines.write(), ActivityLine::note(preview));
                }
                AgentEvent::Warning { kind, message, .. } => {
                    push_line(
                        &mut live_lines.write(),
                        ActivityLine::note(format!("{kind}: {message}")),
                    );
                }
                AgentEvent::CompressionStarted { engine, .. } => {
                    push_line(
                        &mut live_lines.write(),
                        ActivityLine::note(format!("compressing context ({engine})")),
                    );
                }
                AgentEvent::CompressionFinished { .. } => {
                    push_line(
                        &mut live_lines.write(),
                        ActivityLine::note("context compressed"),
                    );
                }
                AgentEvent::QuestionAsked { request } => {
                    pending_question.set(Some(request));
                }
                AgentEvent::QuestionResolved { .. } => pending_question.set(None),
                AgentEvent::Metering {
                    input_tokens,
                    output_tokens,
                    estimated_cost_usd,
                    ..
                } => {
                    tokens.set((input_tokens, output_tokens));
                    if estimated_cost_usd > 0.0 {
                        spend += estimated_cost_usd;
                    }
                }
                AgentEvent::TurnFinished { message_id, at, .. } => {
                    // Fold the live work log behind its assistant
                    // message (t3code's turn fold).
                    let duration = turn_started
                        .peek()
                        .as_ref()
                        .map(|t0| (at - *t0).num_seconds().max(0))
                        .unwrap_or(0);
                    let lines = std::mem::take(&mut *live_lines.write());
                    let r = std::mem::take(&mut *reasoning.write());
                    if !lines.is_empty() || !r.is_empty() {
                        turns.write().insert(
                            message_id,
                            TurnLog {
                                lines,
                                reasoning: r,
                                duration_secs: duration,
                            },
                        );
                    }
                    busy.set(false);
                    stopping.set(false);
                    streaming.set(None);
                    responding.set(String::new());
                    turn_started.set(None);
                    on_activity.call(());
                    // Drain the queue.
                    let next = {
                        let mut q = queued.write();
                        if q.is_empty() {
                            None
                        } else {
                            Some(q.remove(0))
                        }
                    };
                    if let Some(text) = next {
                        dispatch_text(text);
                    }
                }
                AgentEvent::TurnErrored { kind, message, .. } => {
                    busy.set(false);
                    stopping.set(false);
                    streaming.set(None);
                    responding.set(String::new());
                    turn_started.set(None);
                    error.set(format!("{kind}: {message}"));
                    on_activity.call(());
                }
                AgentEvent::TurnCancelled { .. } => {
                    busy.set(false);
                    stopping.set(false);
                    streaming.set(None);
                    responding.set(String::new());
                    turn_started.set(None);
                    push_line(&mut live_lines.write(), ActivityLine::note("cancelled"));
                    on_activity.call(());
                }
                // The server skipped events (mailbox overflow) — the
                // transcript can't be folded forward from here, so
                // re-pull it. The fold handler is sync; the refetch
                // rides its own task.
                AgentEvent::Resync => {
                    spawn(async move {
                        if let Ok(mut msgs) = crate::feeds::fetch_agent_messages(&slug, &sid).await
                        {
                            msgs.reverse();
                            messages.set(msgs);
                        }
                    });
                }
                _ => {}
            }
        },
    );

    // Send-or-queue (CodexMonitor: the button morphs; typing while
    // the agent runs queues the message for the next turn).
    let send = use_callback(move |_: ()| {
        let text = composer.peek().trim().to_string();
        if text.is_empty() {
            return;
        }
        composer.set(String::new());
        // Remember it for `↑` recall, and persist so it survives a
        // reload the way a shell history would.
        let stored = {
            let mut h = history.write();
            h.record(&text);
            serde_json::to_string(h.entries()).unwrap_or_else(|_| "[]".to_string())
        };
        let _ = dioxus::document::eval(&format!(
            "localStorage.setItem('{history_key}', {});",
            serde_json::to_string(&stored).unwrap_or_else(|_| "\"[]\"".to_string())
        ));
        if *busy.peek() {
            queued.write().push(text);
        } else {
            dispatch_text(text);
        }
    });

    let stop_slug = slug.clone();
    let stop_sid = session_id.clone();
    // Stop is optimistic: flip the button the moment it's pressed, so
    // there's feedback while the request and the backend's teardown
    // land. `stopping` clears on whichever terminal event arrives.
    let on_stop = move |_| {
        if *stopping.peek() {
            return;
        }
        stopping.set(true);
        let slug = stop_slug.clone();
        let sid = stop_sid.clone();
        spawn(async move {
            if let Err(e) = crate::feeds::cancel_agent_turn(&slug, &sid).await {
                stopping.set(false);
                error.set(format!("Couldn't stop the turn: {e}"));
            }
        });
    };

    // Answer a question card: dispatch the chosen option's label as
    // the user turn. Provisional until backends grow a first-class
    // answer verb — conversational agents handle it naturally.
    let answer_question = use_callback(move |label: String| {
        pending_question.set(None);
        dispatch_text(label);
    });

    let title = if session.title.trim().is_empty() {
        "(untitled)".to_string()
    } else {
        session.title.clone()
    };
    let streaming_view = streaming.read().clone();
    let reasoning_text = reasoning.read().clone();
    let live_view = live_lines.read().clone();
    let (tok_in, tok_out) = tokens();
    let stream = stream_state.read().clone();
    let session_models: Vec<ModelInfo> = models
        .iter()
        .filter(|m| m.backend_id == session.backend_id)
        .cloned()
        .collect();
    // Context ring: window of the selected (or default) model.
    let context_window = {
        let chosen = model.read().trim().to_string();
        session_models
            .iter()
            .find(|m| {
                if chosen.is_empty() {
                    m.is_default
                } else {
                    m.id == chosen
                }
            })
            .map(|m| m.context_length)
            .unwrap_or(0)
    };
    let ring = context_free_percent(tok_in, context_window);
    let ring_pct_used = ring.map(|f| 100.0 - f);

    // Derived transcript rows (pure).
    let derived_rows = timeline::build_rows(&messages.read(), &turns.read());
    let msgs_snapshot = messages.read().clone();
    let turns_snapshot = turns.read().clone();

    // Composer autocomplete (ranked: exact > prefix > substring >
    // subsequence).
    let completion: Option<(usize, Vec<CompletionRow>)> = {
        let text = composer.read().clone();
        completion_trigger(&text).and_then(|(mode, query, start)| {
            if *completion_dismissed.read() == text {
                return None;
            }
            let rows: Vec<CompletionRow> = match mode {
                '/' if session.backend_id == "hermes" => {
                    let all: Vec<CompletionRow> = HERMES_COMMANDS
                        .iter()
                        .map(|(c, d)| CompletionRow {
                            insert: (*c).to_string(),
                            label: (*c).to_string(),
                            detail: (*d).to_string(),
                        })
                        .collect();
                    rank_by(&all, &query, |r| r.label[1..].to_string(), 10)
                }
                '$' => {
                    let all: Vec<CompletionRow> = skills
                        .iter()
                        .filter(|sk| sk.enabled)
                        .map(|sk| CompletionRow {
                            insert: format!("${}", sk.name),
                            label: sk.name.clone(),
                            detail: sk.description.clone(),
                        })
                        .collect();
                    rank_by(&all, &query, |r| r.label.clone(), 8)
                }
                _ => Vec::new(),
            };
            (!rows.is_empty()).then_some((start, rows))
        })
    };
    let completion_open = completion.is_some();
    let completion_rows = completion
        .as_ref()
        .map(|(_, r)| r.clone())
        .unwrap_or_default();
    let completion_start = completion.as_ref().map(|(s, _)| *s).unwrap_or(0);
    let sel = completion_sel().min(completion_rows.len().saturating_sub(1));

    let accept = use_callback(move |(start, insert): (usize, String)| {
        let mut text = composer.peek().clone();
        text.truncate(start);
        text.push_str(&insert);
        text.push(' ');
        composer.set(text);
        completion_sel.set(0);
    });

    // Follow the tail as the turn streams — but only while the reader
    // is already at the bottom. Scrolling up to re-read something used
    // to be undone by the next token.
    let transcript_id = format!("agent-transcript-{session_id}");
    use_effect({
        let transcript_id = transcript_id.clone();
        move || {
            let _ = messages.read().len();
            let _ = streaming.read().is_some();
            let _ = live_lines.read().len();
            let _ = dioxus::document::eval(&autoscroll_js(&transcript_id));
        }
    });

    let question_view = pending_question.read().clone();
    let queued_view = queued.read().clone();

    rsx! {
        // `min-h-0` is load-bearing: without it this column can't
        // shrink below its content, so the transcript's
        // `overflow-y-auto` never engages — the list just grows and
        // the composer gets pushed out of view with nothing to
        // scroll.
        div { class: "flex min-h-0 min-w-0 flex-1 flex-col",
            // Header.
            div { class: "flex h-12 shrink-0 items-center justify-between gap-2 border-b border-border/60 px-2 md:h-11 md:px-4",
                div { class: "flex min-w-0 items-center gap-2",
                    Link {
                        to: crate::routes::Route::AgentsRoute { session: String::new() },
                        class: "-ml-1 flex h-11 w-9 shrink-0 items-center justify-center rounded-md text-muted-foreground active:bg-accent/40 md:hidden",
                        aria_label: "Back to conversations",
                        ChevronLeft { size: 18 }
                    }
                    span { class: "truncate text-sm font-medium text-foreground", title: "{title}", "{title}" }
                    span { class: backend_chip_cls(&session.backend_id), "{session.backend_id}" }
                    match &stream {
                        StreamState::Connecting => rsx! {
                            span { class: "shrink-0 rounded-full bg-muted/60 px-1.5 {style::MICRO} text-muted-foreground",
                                "connecting"
                            }
                        },
                        StreamState::Live(reconnects) => rsx! {
                            span {
                                class: "shrink-0 rounded-full bg-emerald-500/15 px-1.5 {style::MICRO} text-emerald-500",
                                title: if *reconnects > 0 {
                                    format!("Live — reconnected {reconnects} time(s) this session")
                                } else {
                                    "Live event stream connected".to_string()
                                },
                                "● live"
                            }
                        },
                        StreamState::Retrying { why, secs } => rsx! {
                            span {
                                class: "shrink-0 rounded-full bg-amber-500/15 px-1.5 {style::MICRO} text-amber-500",
                                title: "{why}",
                                if *secs > 0 {
                                    "○ reconnecting in {secs}s"
                                } else {
                                    "○ reconnecting…"
                                }
                            }
                        },
                    }
                    GatewayChip { slug: slug.clone(), backend_id: session.backend_id.clone() }
                }
                div { class: "flex shrink-0 items-center gap-2",
                    if busy() {
                        Button {
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Small,
                            disabled: stopping(),
                            on_click: on_stop,
                            if stopping() { "Stopping…" } else { "Stop" }
                        }
                    }
                    button {
                        r#type: "button",
                        class: if inspector_open {
                            "flex h-11 w-11 items-center justify-center rounded-md bg-accent text-foreground md:h-7 md:w-7 {style::FOCUS}"
                        } else {
                            "flex h-11 w-11 items-center justify-center rounded-md text-muted-foreground hover:text-foreground md:h-7 md:w-7 {style::FOCUS}"
                        },
                        title: "Session details",
                        onclick: move |_| on_toggle_inspector.call(()),
                        Info { size: 14 }
                    }
                }
            }

            // Transcript timeline (derived rows + live tail). The id is
            // per-session because the panel and the /agents page can
            // both be mounted at once.
            div {
                id: "{transcript_id}",
                class: "flex min-h-0 w-full flex-1 flex-col gap-4 overflow-y-auto overflow-x-hidden px-3 py-4 md:px-4",
                onscroll: move |e| {
                    let d = e.data();
                    scroll.set(scroll_mode(
                        d.scroll_top(),
                        f64::from(d.scroll_height()),
                        f64::from(d.client_height()),
                    ));
                },
                for row in derived_rows.iter() {
                    match row {
                        Row::Message(i) => rsx! {
                            if let Some(m) = msgs_snapshot.get(*i) {
                                {message_view(m)}
                            }
                        },
                        Row::TurnFold { anchor, summary } => rsx! {
                            {turn_fold_view(anchor, summary, &turns_snapshot, expanded_folds)}
                        },
                    }
                }
                // Live turn tail: reasoning, activity, streaming, timer.
                if !reasoning_text.is_empty() {
                    div { class: "{style::RAIL}",
                        details { class: "{style::UI} text-muted-foreground", open: busy(),
                            summary { class: "cursor-pointer select-none font-medium",
                                if busy() {
                                    span { class: "mr-1 inline-block h-1.5 w-1.5 rounded-full bg-primary align-middle agents-dot-pulse" }
                                }
                                "Thinking"
                            }
                            pre { class: "mt-1 whitespace-pre-wrap font-sans leading-relaxed", "{reasoning_text}" }
                        }
                    }
                }
                if !live_view.is_empty() {
                    div { class: "flex flex-col gap-0.5",
                        for (i , line) in live_view.iter().enumerate() {
                            {activity_line_view(i, line)}
                        }
                    }
                }
                if let Some((_, text)) = &streaming_view {
                    div { class: "max-w-none",
                        task_ui::Markdown { source: text.clone() }
                        span { class: "ml-0.5 inline-block h-4 w-2 bg-primary/70 agents-dot-pulse" }
                    }
                }
                if busy() {
                    div { class: "flex items-center gap-2 {style::UI} text-muted-foreground",
                        Spinner { size: SpinnerSize::Small }
                        // What it's actually doing beats a bare
                        // "Working…" — the in-flight tool if there is
                        // one, else the phase we can infer.
                        span { class: "truncate",
                            match running_tool(&live_view) {
                                Some(t) => t.text.clone(),
                                None if streaming_view.is_some() => "Writing the answer".to_string(),
                                None if !reasoning_text.is_empty() => "Thinking".to_string(),
                                None => "Working".to_string(),
                            }
                        }
                        span { class: "shrink-0 tabular-nums text-muted-foreground/60",
                            "{fmt_elapsed(elapsed())}"
                        }
                        if !responding.read().is_empty() {
                            span { class: "hidden shrink-0 truncate text-xs text-muted-foreground/60 sm:inline",
                                "· {responding}"
                            }
                        }
                    }
                }
                if !error.read().is_empty() {
                    div { class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 {style::UI} leading-relaxed",
                        "{error}"
                    }
                }
            }

            // Jump to latest — the way back once you've scrolled off
            // the tail (t3code's free-scrolling mode made visible).
            if *scroll.read() == ScrollMode::FreeScrolling {
                div { class: "pointer-events-none relative z-20 h-0",
                    div { class: "pointer-events-auto absolute bottom-1 flex w-full justify-center",
                        button {
                            r#type: "button",
                            class: "flex min-h-9 items-center rounded-full border border-border/60 bg-popover/95 px-4 text-xs text-foreground shadow-lg hover:border-primary/60 md:min-h-0 md:px-3 md:py-1",
                            onclick: {
                                let id = transcript_id.clone();
                                move |_| {
                                    let _ = dioxus::document::eval(&scroll_to_end_js(&id));
                                    scroll.set(ScrollMode::FollowingEnd);
                                }
                            },
                            if busy() { "Jump to latest ·  live" } else { "Jump to latest" }
                        }
                    }
                }
            }

            // Pending question card (numbered-shortcut options).
            if let Some(q) = &question_view {
                {question_card(q, answer_question)}
            }

            // Queued sends.
            if !queued_view.is_empty() {
                div { class: "flex flex-wrap items-center gap-1.5 border-t border-border/60 px-4 pt-2",
                    span { class: "{style::MICRO} uppercase tracking-[0.14em] text-muted-foreground", "Queued" }
                    for (i , q) in queued_view.iter().enumerate() {
                        span {
                            key: "{i}",
                            class: "flex max-w-64 items-center gap-1 rounded-full bg-muted/50 px-2 py-0.5 text-xs",
                            span { class: "truncate", "{q}" }
                            button {
                                r#type: "button",
                                class: "text-muted-foreground hover:text-foreground",
                                onclick: move |_| {
                                    queued.write().remove(i);
                                },
                                "×"
                            }
                        }
                    }
                }
            }

            // Composer + chip row.
            div {
                class: "relative border-t border-border/60 px-3 py-2 md:px-4 md:py-3",
                style: "padding-bottom: max(0.5rem, env(safe-area-inset-bottom, 0px));",
                if completion_open {
                    div { class: "absolute bottom-full left-3 z-30 mb-1 max-h-64 w-[min(26rem,calc(100vw-1.5rem))] overflow-y-auto rounded-lg border border-border bg-popover p-1 shadow-lg md:left-4",
                        for (i , row) in completion_rows.iter().enumerate() {
                            {
                                let insert = row.insert.clone();
                                rsx! {
                                    div {
                                        key: "{row.insert}",
                                        role: "button",
                                        class: if i == sel {
                                            "flex min-h-9 cursor-pointer items-baseline gap-2 rounded-md bg-accent px-2 py-2 md:min-h-0 md:py-1"
                                        } else {
                                            "flex min-h-9 cursor-pointer items-baseline gap-2 rounded-md px-2 py-2 hover:bg-accent/50 md:min-h-0 md:py-1"
                                        },
                                        onmousedown: move |e| {
                                            e.prevent_default();
                                            accept((completion_start, insert.clone()));
                                        },
                                        span { class: "shrink-0 font-mono text-xs font-semibold text-foreground", "{row.label}" }
                                        span { class: "truncate text-xs text-muted-foreground", "{row.detail}" }
                                    }
                                }
                            }
                        }
                        if !touch {
                            div { class: "mt-0.5 border-t border-border/60 px-2 pt-1 text-[11px] text-muted-foreground/70",
                                "↑↓ navigate · Tab/Enter accept · Esc dismiss"
                            }
                        }
                    }
                }
                div { class: "flex flex-col rounded-xl border border-border/60 bg-card/30 transition-colors focus-within:border-primary/60",
                    textarea {
                        class: "max-h-40 min-h-[2.75rem] w-full resize-y border-0 bg-transparent px-3 pb-1 pt-2.5 text-base leading-relaxed text-foreground outline-none placeholder:text-muted-foreground/60 md:text-sm",
                        placeholder: if touch {
                            "Message the agent"
                        } else {
                            "Message the agent — / for commands, $ for skills"
                        },
                        value: "{composer}",
                        oninput: move |e| {
                            composer.set(e.value());
                            completion_sel.set(0);
                            history.write().reset();
                        },
                        onkeydown: {
                            let rows = completion_rows.clone();
                            move |e| {
                                if completion_open && !rows.is_empty() {
                                    match e.key() {
                                        Key::ArrowDown => {
                                            e.prevent_default();
                                            completion_sel.set((sel + 1) % rows.len());
                                            return;
                                        }
                                        Key::ArrowUp => {
                                            e.prevent_default();
                                            completion_sel.set(sel.checked_sub(1).unwrap_or(rows.len() - 1));
                                            return;
                                        }
                                        Key::Tab => {
                                            e.prevent_default();
                                            accept((completion_start, rows[sel].insert.clone()));
                                            return;
                                        }
                                        Key::Enter if !e.modifiers().shift() => {
                                            e.prevent_default();
                                            accept((completion_start, rows[sel].insert.clone()));
                                            return;
                                        }
                                        Key::Escape => {
                                            e.prevent_default();
                                            completion_dismissed.set(composer.peek().clone());
                                            return;
                                        }
                                        _ => {}
                                    }
                                }
                                // Shell-style recall. Deliberately after
                                // the autocomplete arm: while the popover
                                // is open the arrows belong to it.
                                if matches!(e.key(), Key::ArrowUp | Key::ArrowDown)
                                    && !e.modifiers().shift()
                                    && !e.modifiers().ctrl()
                                    && !e.modifiers().alt()
                                    && !e.modifiers().meta()
                                {
                                    let dir = if e.key() == Key::ArrowUp {
                                        Recall::Older
                                    } else {
                                        Recall::Newer
                                    };
                                    let current = composer.peek().clone();
                                    if let Some(text) = history.write().recall(dir, &current) {
                                        e.prevent_default();
                                        composer.set(text);
                                        return;
                                    }
                                }
                                // On a touch keyboard Return is the only
                                // way to get a newline, so it must not
                                // send. The Send button is the commit.
                                if e.key() == Key::Enter && !e.modifiers().shift() && !touch {
                                    e.prevent_default();
                                    send(());
                                }
                            }
                        },
                    }
                div { class: "flex items-center gap-2 px-2 pb-2 pt-0.5",
                    ModelPicker {
                        models: session_models.clone(),
                        current: model.read().clone(),
                        on_pick: {
                            let backend = session.backend_id.clone();
                            move |m: ModelInfo| {
                            // Session-only selection (hermes-desktop's
                            // persist:false): the override rides each
                            // dispatch. For catalog models on Hermes,
                            // prefill the gateway's per-session /model
                            // switch so the change is explicit + visible.
                            if m.provider_id == "hermes" || m.is_default {
                                model.set(String::new());
                            } else {
                                model.set(m.id.clone());
                                if backend == "hermes" {
                                    composer.set(format!("/model {}", m.id));
                                }
                            }
                        }
                        },
                    }
                    // Context gauge: conic-gradient ring when the
                    // window is known, raw counter otherwise.
                    div { class: "ml-auto flex items-center gap-1.5 text-xs tabular-nums text-muted-foreground",
                        if let (Some(free), Some(used_pct)) = (ring, ring_pct_used) {
                            span {
                                class: "inline-block h-5 w-5 rounded-full",
                                style: "{context_ring_style(free)}",
                                title: "{used_pct:.0}% of context used — {fmt_tokens(tok_in)} of {fmt_tokens(context_window)} tokens",
                            }
                        }
                        span {
                            title: "context (input) / generated (output) tokens",
                            if tok_in + tok_out > 0 {
                                "{fmt_tokens(tok_in)} ctx · {fmt_tokens(tok_out)} out"
                            } else {
                                "no usage yet"
                            }
                        }
                        if let Some(c) = fmt_cost(spend()) {
                            span {
                                class: "rounded-full bg-muted/50 px-1.5",
                                title: "Estimated spend this session, priced from the models.dev catalog",
                                "{c}"
                            }
                        }
                    }
                    // Send lives inside the field, after the meta, so
                    // the composer reads as one control rather than a
                    // box with a button parked beside it.
                    Button {
                        variant: ButtonVariant::Primary,
                        size: ButtonSize::Small,
                        class: "min-h-9 px-4 md:min-h-0 md:px-3",
                        disabled: composer.read().trim().is_empty(),
                        on_click: move |_| send(()),
                        if busy() { "Queue" } else { "Send" }
                    }
                }
            }
            }
        }
    }
}

/// A folded completed turn: one-line summary, expandable to the
/// retained activity log + reasoning.
fn turn_fold_view(
    anchor: &str,
    summary: &str,
    turns: &HashMap<String, TurnLog>,
    mut expanded: Signal<std::collections::HashSet<String>>,
) -> Element {
    let is_open = expanded.read().contains(anchor);
    let key = anchor.to_string();
    let log = turns.get(anchor).cloned().unwrap_or_default();

    rsx! {
        div { key: "fold-{anchor}", class: "flex flex-col gap-1",
            button {
                r#type: "button",
                class: "flex w-fit items-center gap-1.5 rounded-md py-0.5 pr-2 {style::MICRO} text-muted-foreground/80 hover:text-foreground {style::FOCUS}",
                onclick: move |_| {
                    let mut set = expanded.write();
                    if !set.remove(&key) {
                        set.insert(key.clone());
                    }
                },
                span {
                    class: if is_open {
                        "inline-block w-3 rotate-90 text-center transition-transform"
                    } else {
                        "inline-block w-3 text-center transition-transform"
                    },
                    "›"
                }
                span { "{summary}" }
            }
            if is_open {
                div { class: "ml-1.5 flex flex-col gap-1.5 {style::RAIL}",
                    if !log.reasoning.is_empty() {
                        details { class: "{style::UI} text-muted-foreground",
                            summary { class: "cursor-pointer select-none font-medium {style::FOCUS}", "Thinking" }
                            pre { class: "mt-1.5 whitespace-pre-wrap font-sans leading-relaxed", "{log.reasoning}" }
                        }
                    }
                    for (i , line) in log.lines.iter().enumerate() {
                        {activity_line_view(i, line)}
                    }
                }
            }
        }
    }
}

/// One activity line: status glyph + mono text (t3code's
/// `SimpleWorkEntryRow` glyph state machine, compact form). Tool
/// rows carry their duration and expand to arguments + result.
fn activity_line_view(i: usize, line: &ActivityLine) -> Element {
    let (glyph, glyph_cls) = match line.tone {
        ToolTone::Running => ("▸", "text-primary"),
        ToolTone::Ok => ("✓", "text-emerald-500"),
        ToolTone::Fail => ("✗", "text-destructive"),
        ToolTone::Note => ("·", "text-muted-foreground/70"),
    };
    let took = (line.duration_ms > 0).then(|| fmt_duration(line.duration_ms));
    // Fixed-width glyph column: every tool name starts at the same x,
    // so a turn's activity reads as a list instead of a ragged pile.
    let head = rsx! {
        span { class: "w-3 shrink-0 text-center {glyph_cls}", "{glyph}" }
        span { class: "min-w-0 truncate", title: "{line.text}", "{line.text}" }
        if let Some(t) = &took {
            span { class: "ml-auto shrink-0 pl-2 tabular-nums text-muted-foreground/50", "{t}" }
        }
    };
    let row_cls = "flex w-full items-baseline gap-2 rounded-md px-2 py-1 font-mono text-[11px] text-muted-foreground";

    if !line.has_detail() {
        return rsx! {
            div { key: "{i}", class: "{row_cls}", {head} }
        };
    }
    rsx! {
        details { key: "{i}", class: "w-full",
            summary { class: "{row_cls} cursor-pointer list-none rounded-md hover:bg-muted/50 {style::FOCUS}", {head} }
            div { class: "ml-3 mt-1 flex flex-col gap-1.5 {style::RAIL}",
                if !line.args.is_empty() {
                    pre { class: "max-h-40 overflow-auto whitespace-pre-wrap rounded-md bg-muted/40 px-2.5 py-1.5 font-mono text-[11px] leading-relaxed text-muted-foreground",
                        "{line.args}"
                    }
                }
                if !line.output.is_empty() {
                    pre {
                        class: if line.tone == ToolTone::Fail {
                            "max-h-56 overflow-auto whitespace-pre-wrap rounded-md border border-destructive/40 bg-destructive/5 px-2.5 py-1.5 font-mono text-[11px] leading-relaxed text-muted-foreground"
                        } else {
                            "max-h-56 overflow-auto whitespace-pre-wrap rounded-md bg-muted/25 px-2.5 py-1.5 font-mono text-[11px] leading-relaxed text-muted-foreground"
                        },
                        "{line.output}"
                    }
                }
            }
        }
    }
}

/// A pending structured question: numbered option cards (t3code's
/// `ComposerPendingUserInputPanel`, compact form). Answering
/// dispatches the option label as the user's turn.
fn question_card(q: &QuestionRequest, answer: Callback<String>) -> Element {
    let Some(first) = q.questions.first() else {
        return rsx! {};
    };
    let total = q.questions.len();

    rsx! {
        div { class: "border-t border-border/60 bg-card/40 px-4 py-3",
            div { class: "mb-2 flex items-center gap-2",
                if !first.header.is_empty() {
                    span { class: "rounded-full bg-primary/15 px-2 py-0.5 text-xs font-medium text-primary",
                        "{first.header}"
                    }
                }
                span { class: "text-sm font-medium", "{first.text}" }
                if total > 1 {
                    span { class: "ml-auto rounded-full bg-muted/60 px-2 py-0.5 text-xs text-muted-foreground",
                        "1/{total}"
                    }
                }
            }
            div { class: "flex flex-col gap-1",
                for (i , opt) in first.options.iter().enumerate().take(9) {
                    {
                        let label = opt.label.clone();
                        rsx! {
                            button {
                                key: "{opt.label}",
                                r#type: "button",
                                class: "flex w-full items-baseline gap-2 rounded-lg border border-border/60 px-3 py-1.5 text-left hover:border-primary/60 hover:bg-primary/5",
                                onclick: move |_| answer(label.clone()),
                                kbd { class: "rounded border border-border/60 bg-muted/40 px-1 font-mono text-xs text-muted-foreground",
                                    "{i + 1}"
                                }
                                span { class: "text-sm", "{opt.label}" }
                                if !opt.description.is_empty() {
                                    span { class: "truncate text-xs text-muted-foreground", "{opt.description}" }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Right panel: session meta + actions, skills, capabilities.
#[component]
fn Inspector(
    slug: String,
    session: Session,
    skills: Vec<SkillInfo>,
    capabilities: Vec<CapabilityFlag>,
    mutate: Callback<(String, String, SessionAction)>,
) -> Element {
    let mut title_draft = use_signal(|| session.title.clone());
    let mut confirm_delete = use_signal(|| false);
    let created = session.created_at.format("%b %-d, %-I:%M %p").to_string();
    let tokens = session.usage.input_tokens + session.usage.output_tokens;
    let sid = session.id.clone();
    let sslug = slug.clone();

    rsx! {
        div { class: "flex w-full flex-col gap-5 overflow-y-auto px-0 py-1 md:w-72 md:shrink-0 md:border-l md:border-border/60 md:px-4 md:py-4",
            div { class: "flex flex-col gap-2",
                span { class: "{style::EYEBROW}",
                    "Session"
                }
                div { class: "flex items-center gap-1.5",
                    input {
                        class: "w-full rounded-md border border-border/60 bg-card/30 px-2 py-1 text-xs text-foreground outline-none focus:border-primary/60",
                        value: "{title_draft}",
                        placeholder: "Title",
                        oninput: move |e| title_draft.set(e.value()),
                    }
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Small,
                        on_click: {
                            let (s, id) = (sslug.clone(), sid.clone());
                            move |_| {
                                mutate((s.clone(), id.clone(), SessionAction::Rename(title_draft.peek().clone())));
                            }
                        },
                        "Save"
                    }
                }
                div { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                    div { class: "flex justify-between",
                        span { "Backend" }
                        span { class: backend_chip_cls(&session.backend_id), "{session.backend_id}" }
                    }
                    div { class: "flex justify-between",
                        span { "Created" }
                        span { "{created}" }
                    }
                    div { class: "flex justify-between",
                        span { "Tokens" }
                        span { class: "tabular-nums", "{fmt_tokens(tokens)}" }
                    }
                    if let Some(c) = fmt_cost(session.usage.estimated_cost_usd) {
                        div { class: "flex justify-between",
                            span { "Est. cost" }
                            span { class: "tabular-nums", title: "Priced from the models.dev catalog", "{c}" }
                        }
                    }
                    div { class: "flex justify-between",
                        span { "Status" }
                        span { "{status_text(session.status)}" }
                    }
                }
                div { class: "flex items-center gap-1.5",
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Small,
                        on_click: {
                            let (s, id, v) = (sslug.clone(), sid.clone(), session.pinned);
                            move |_| mutate((s.clone(), id.clone(), SessionAction::Pin(!v)))
                        },
                        if session.pinned { "Unpin" } else { "Pin" }
                    }
                    Button {
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Small,
                        on_click: {
                            let (s, id, v) = (sslug.clone(), sid.clone(), session.archived);
                            move |_| mutate((s.clone(), id.clone(), SessionAction::Archive(!v)))
                        },
                        if session.archived { "Unarchive" } else { "Archive" }
                    }
                    button {
                        r#type: "button",
                        class: "ml-auto flex items-center gap-1 rounded-md px-1.5 py-1 text-xs text-destructive hover:bg-destructive/10",
                        onclick: {
                            let (s, id) = (sslug.clone(), sid.clone());
                            move |_| {
                                if *confirm_delete.peek() {
                                    mutate((s.clone(), id.clone(), SessionAction::Delete));
                                } else {
                                    confirm_delete.set(true);
                                }
                            }
                        },
                        Trash2 { size: 12 }
                        if confirm_delete() { "Really delete?" } else { "Delete" }
                    }
                }
            }

            BackendHealthPanel { slug: slug.clone(), backend_id: session.backend_id.clone() }

            div { class: "flex flex-col gap-1.5",
                span { class: "{style::EYEBROW}",
                    "Skills ({skills.len()})"
                }
                if skills.is_empty() {
                    Text { variant: TextVariant::Muted, class: "text-xs",
                        "No skills reported — the agent learns them over time (`/learn`)."
                    }
                }
                for sk in skills.iter() {
                    div { key: "{sk.backend_id}/{sk.name}", class: "rounded-md border border-border/60 bg-card/30 px-2 py-1.5",
                        div { class: "flex items-center justify-between gap-2",
                            span { class: "truncate text-xs font-medium text-foreground", "{sk.name}" }
                            if !sk.enabled {
                                span { class: "text-[11px] text-muted-foreground", "off" }
                            }
                        }
                        if !sk.description.is_empty() {
                            p { class: "mt-0.5 line-clamp-2 text-xs leading-snug text-muted-foreground",
                                "{sk.description}"
                            }
                        }
                    }
                }
            }

            if !capabilities.is_empty() {
                div { class: "flex flex-col gap-1",
                    span { class: "{style::EYEBROW}",
                        "Capabilities"
                    }
                    div { class: "flex flex-wrap gap-1",
                        for c in capabilities.iter() {
                            span {
                                key: "{c.backend_id}/{c.name}",
                                class: if c.enabled {
                                    "rounded-full bg-emerald-500/10 px-1.5 py-0.5 text-[11px] text-emerald-500"
                                } else {
                                    "rounded-full bg-muted/40 px-1.5 py-0.5 text-[11px] text-muted-foreground line-through"
                                },
                                "{c.name}"
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Inspector block for the backend serving this session: is it up,
/// what state does it report, what's it connected to, how far away
/// is it. Refreshes on the same 20s cadence as the header chip.
#[component]
fn BackendHealthPanel(slug: String, backend_id: String) -> Element {
    let mut health = use_resource(use_reactive!(|(slug,)| async move {
        crate::feeds::fetch_agent_health(&slug).await
    }));
    let mut tick = use_signal(|| 0u32);
    use_future(move || async move {
        loop {
            architect::platform::sleep(std::time::Duration::from_secs(20)).await;
            tick += 1;
        }
    });
    use_effect(move || {
        let _ = tick();
        health.restart();
    });

    let snapshot = health.read().clone();
    let (row, err) = match &snapshot {
        Some(Ok(rows)) => (
            rows.iter().find(|h| h.backend_id == backend_id).cloned(),
            String::new(),
        ),
        Some(Err(e)) => (None, e.clone()),
        None => (None, String::new()),
    };

    rsx! {
        div { class: "flex flex-col gap-1",
            span { class: "{style::EYEBROW}",
                "Backend"
            }
            if !err.is_empty() {
                div { class: "rounded-md border border-destructive/40 bg-destructive/10 px-2 py-1 text-xs",
                    "{err}"
                }
            }
            if let Some(h) = row {
                div { class: "flex flex-col gap-1 text-xs text-muted-foreground",
                    div { class: "flex items-center justify-between gap-2",
                        span { "Reachable" }
                        span {
                            class: if h.reachable {
                                "text-emerald-500"
                            } else {
                                "text-destructive"
                            },
                            if h.reachable { "yes" } else { "no" }
                        }
                    }
                    if !h.state.is_empty() {
                        div { class: "flex justify-between gap-2",
                            span { "State" }
                            span { class: "truncate", "{h.state}" }
                        }
                    }
                    if !h.model.is_empty() {
                        div { class: "flex justify-between gap-2",
                            span { "Model" }
                            span { class: "truncate", title: "{h.model}", "{h.model}" }
                        }
                    }
                    if h.reachable {
                        div { class: "flex justify-between gap-2",
                            span { "In flight" }
                            span { class: "tabular-nums", "{h.active_agents}" }
                        }
                    }
                    if h.last_ping_ms > 0 {
                        div { class: "flex justify-between gap-2",
                            span { "Latency" }
                            span { class: "tabular-nums", "{h.last_ping_ms}ms" }
                        }
                    }
                    if !h.platforms.is_empty() {
                        div { class: "flex flex-wrap gap-1 pt-0.5",
                            for p in h.platforms.iter() {
                                span {
                                    key: "{p}",
                                    class: "rounded-full bg-muted/50 px-1.5 py-0.5 text-[11px]",
                                    "{p}"
                                }
                            }
                        }
                    }
                    if !h.status_text.is_empty() {
                        p { class: "pt-0.5 text-xs leading-snug text-muted-foreground/80",
                            "{h.status_text}"
                        }
                    }
                }
            }
        }
    }
}

fn status_text(status: SessionStatus) -> &'static str {
    match status {
        SessionStatus::Idle => "Idle",
        SessionStatus::Running => "Running",
        SessionStatus::AwaitingUser => "Awaiting user",
        SessionStatus::Cancelled => "Cancelled",
        SessionStatus::Errored => "Errored",
    }
}

/// Concatenated text content of a message.
fn text_of(m: &Message) -> String {
    m.content
        .iter()
        .filter_map(|b| match b {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Copy text to the clipboard via the browser API.
fn copy_text(text: &str) {
    let js = format!(
        "navigator.clipboard && navigator.clipboard.writeText({});",
        serde_json::to_string(text).unwrap_or_default()
    );
    let _ = dioxus::document::eval(&js);
}

/// A vault note the answer referenced, as a link into the vault.
fn note_chip(path: &str) -> Element {
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.strip_suffix(".md").unwrap_or(name);
    let to = crate::routes::Route::VaultRoute {
        path: path.to_string(),
        org: String::new(),
    };
    rsx! {
        Link {
            key: "{path}",
            to,
            class: "flex max-w-64 items-center gap-1 rounded-full border border-border/60 bg-muted/40 px-2 py-0.5 text-[11px] text-muted-foreground hover:border-primary/50 hover:text-foreground",
            title: "{path}",
            FileText { size: 10 }
            span { class: "truncate", "{name}" }
        }
    }
}

/// One transcript entry.
///
/// Both roles run flush left against the turn rail rather than the
/// usual left/right bubble pair. In a 400px sidebar a right-aligned
/// bubble throws away half the width and breaks the vertical scan
/// line that the tool activity between messages depends on. The ask
/// is a quiet, indented prompt; the answer is the primary prose.
fn message_view(m: &Message) -> Element {
    let text = text_of(m);
    if m.errored {
        return rsx! {
            div {
                key: "{m.id}",
                class: "rounded-lg border border-destructive/40 bg-destructive/10 px-3 py-2 {style::BODY}",
                "{m.error_text}"
            }
        };
    }
    match m.role {
        Role::User => rsx! {
            div {
                key: "{m.id}",
                class: "whitespace-pre-wrap border-l-2 border-primary/60 pl-3 {style::BODY} text-foreground/90",
                "{text}"
            }
        },
        Role::Assistant => {
            let copy_source = text.clone();
            // Notes the answer cites — one click instead of a path you
            // have to go find (CodexMonitor's message file links).
            let refs = referenced_paths(&text);
            rsx! {
                div { key: "{m.id}", class: "group relative min-w-0 max-w-none break-words",
                    task_ui::Markdown { source: text }
                    if !refs.is_empty() {
                        div { class: "mt-2 flex flex-wrap items-center gap-1",
                            span { class: "{style::MICRO} text-muted-foreground/60", "Notes" }
                            for path in refs.iter() {
                                {note_chip(path)}
                            }
                        }
                    }
                    button {
                        r#type: "button",
                        class: "absolute -top-1 right-0 hidden rounded-md border border-border/60 bg-card/90 p-1 text-muted-foreground hover:text-foreground group-hover:block {style::FOCUS}",
                        title: "Copy message",
                        onclick: move |_| copy_text(&copy_source),
                        Copy { size: 12 }
                    }
                }
            }
        }
        Role::System | Role::Tool => rsx! {
            div { key: "{m.id}", class: "text-xs italic text-muted-foreground", "{text}" }
        },
    }
}
