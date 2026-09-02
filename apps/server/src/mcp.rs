//! MCP server — Task as a tool surface for any agent.
//!
//! Mounted at `POST /org/{slug}/mcp` (MCP Streamable HTTP transport,
//! JSON-RPC 2.0 over a single POST; we answer inline rather than
//! upgrading to SSE because every tool here returns promptly).
//!
//! This is the other half of the agent story. `agent_router` lets
//! Task *drive* agents; this lets an agent drive **Task** — create a
//! task, move a calendar event, capture a reminder, read a note —
//! against the same in-process backends the vox layer serves, so an
//! agent's writes are indistinguishable from the UI's.
//!
//! Because it speaks MCP rather than anything Task-specific, the same
//! endpoint serves the Hermes gateway (`mcp_servers:` in its config),
//! Claude Code, Codex, or any other MCP client.
//!
//! **Orientation**: `initialize` returns `instructions`, which MCP
//! clients fold into the agent's system prompt. That's how the agent
//! learns it's inside Task — which org, what today's date is, and the
//! vault's conventions. [`server_instructions`] is that text.
//!
//! **Auth**: `Authorization: Bearer <token>`, accepting either the
//! static `TASK_MCP_TOKEN` or a real architect-auth session token
//! (same rule as [`crate::watch_bridge`]).
//!
//! **Safety**: v1 exposes no deletion. Tasks are completed by status
//! change, events are cancelled explicitly, and captures land as
//! `suggested` for one-tap acceptance unless the caller opts out —
//! an agent should be able to propose freely without polluting a
//! trusted queue.

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::AppState;

/// The read-side Tempo/Loki client behind the `telemetry_*` tools.
/// Declared here rather than in `lib.rs` so the MCP lane owns the one
/// place the cluster's telemetry is read from; `lib.rs` may adopt it
/// with a plain `pub mod telemetry_query;` later.
#[path = "telemetry_query.rs"]
pub mod telemetry_query;

/// MCP revisions we can speak. We echo the client's requested
/// version when it's one of these, else answer with the newest —
/// the spec's negotiation rule.
const SUPPORTED_PROTOCOLS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const LATEST_PROTOCOL: &str = "2025-06-18";

/// Cap on rows returned by any list tool. Agent context is the
/// scarce resource — a 400-task vault must not land in one message.
const DEFAULT_LIMIT: usize = 50;
const MAX_LIMIT: usize = 200;

/// Per-task body cap in `list_untriaged_tasks`. The body is where a
/// bare title's missing context usually hides, so it's worth sending
/// — but a batch of 50 full notes is not.
const TRIAGE_BODY_CAP: usize = 800;

/// Head-truncate on a char boundary, marking the cut so the agent
/// knows it's reading a fragment and doesn't conclude from silence.
fn truncate_body(body: &str, cap: usize) -> String {
    let body = body.trim();
    if body.chars().count() <= cap {
        return body.to_string();
    }
    let head: String = body.chars().take(cap).collect();
    format!("{head}\n…[truncated]")
}

/// Vault id the org's `VaultSync` backend is keyed by (single-vault
/// per org today, same constant the UI passes).
const VAULT_ID: &str = "default";

// ── JSON-RPC envelope ────────────────────────────────────────────

/// JSON-RPC error codes we emit (the standard set plus MCP's use of
/// `-32602` for bad tool arguments).
mod code {
    pub const PARSE: i32 = -32700;
    // The full standard set is kept on purpose (INTERNAL is currently
    // unreferenced): this table documents the wire contract, not just
    // the codes we happen to emit today.
    pub const INVALID_REQUEST: i32 = -32600;
    pub const METHOD_NOT_FOUND: i32 = -32601;
    pub const INVALID_PARAMS: i32 = -32602;
    #[allow(dead_code)]
    pub const INTERNAL: i32 = -32603;
}

fn rpc_result(id: Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn rpc_error(id: Value, code: i32, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message.into() },
    })
}

/// A tool's answer. MCP models tool failure as a *successful*
/// response with `isError: true` — the agent is meant to read the
/// message and adapt, not to see a transport error.
fn tool_ok(value: &Value) -> Value {
    json!({
        "content": [{ "type": "text", "text": compact(value) }],
        "isError": false,
    })
}

fn tool_err(message: impl Into<String>) -> Value {
    json!({
        "content": [{ "type": "text", "text": message.into() }],
        "isError": true,
    })
}

/// Tool payloads are JSON the model reads as text; pretty-printing
/// costs tokens for no comprehension gain.
fn compact(v: &Value) -> String {
    serde_json::to_string(v).unwrap_or_else(|_| "null".to_string())
}

// ── Tool catalog ─────────────────────────────────────────────────

/// One exposed tool: MCP name, one-line purpose, the JSON Schema the
/// client validates arguments against, and the owning plugin.
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub schema: fn() -> Value,
    /// Owning plugin's `task_plugin::CATALOG` id (`"core"` for
    /// platform tools). Mirrors the `plugin` field on
    /// [`crate::permits::Mount`]: a disabled plugin's tools are
    /// absent from `tools/list` and refused at `tools/call` — the
    /// same gate the vox router applies by not mounting the
    /// plugin's services.
    pub plugin: &'static str,
}

/// Shorthand for a JSON Schema object.
fn obj(props: Value, required: &[&str]) -> Value {
    json!({
        "type": "object",
        "properties": props,
        "required": required,
        "additionalProperties": false,
    })
}

fn s_(desc: &str) -> Value {
    json!({ "type": "string", "description": desc })
}

fn i_(desc: &str) -> Value {
    json!({ "type": "integer", "description": desc })
}

fn b_(desc: &str) -> Value {
    json!({ "type": "boolean", "description": desc })
}

/// Array-of-strings schema.
fn a_(desc: &str) -> Value {
    json!({ "type": "array", "items": { "type": "string" }, "description": desc })
}

/// Every tool this server exposes, in the order the agent sees them.
///
/// Descriptions are written *for the model*: they say when to reach
/// for the tool, not merely what it does, because that's what drives
/// correct selection.
#[must_use]
pub fn tool_catalog() -> Vec<ToolDef> {
    let mut v = vec![
        ToolDef {
            name: "task_context",
            plugin: "core",
            description: "Orient yourself: today's date and time, the org you're working in, \
                          and a count of what's open (tasks due, events today, inbox items). \
                          Call this first when the user's request depends on 'now' or 'what's \
                          on my plate'.",
            schema: || obj(json!({}), &[]),
        },
        ToolDef {
            name: "list_tasks",
            plugin: "core",
            description: "List tasks. Defaults to open tasks only. Filter by status, or by \
                          'due_on_or_before' (ISO YYYY-MM-DD) to answer 'what's due today/this \
                          week'. Returns ids you must pass to update_task.",
            schema: || {
                obj(
                    json!({
                        "status": s_("Exact status to match, e.g. 'open', 'in-progress', 'done'. Omit for all open tasks."),
                        "due_on_or_before": s_("ISO date (YYYY-MM-DD). Keeps tasks due or scheduled on/before it."),
                        "project": s_("Project UUID (from list_projects) — keeps tasks belonging to that project."),
                        "query": s_("Case-insensitive substring match on the title."),
                        "limit": i_("Max rows (default 50, max 200)."),
                    }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "create_task",
            plugin: "core",
            description: "Create a task from natural language. The text is parsed the same way \
                          the app's quick-add is: '#tag', '@context', '[[Project]]', '!high', and \
                          dates like 'tomorrow', 'next monday', '2026-08-01' are extracted, and \
                          the remainder becomes the title. Explicit fields override what the text \
                          implies.",
            schema: || {
                obj(
                    json!({
                        "text": s_("The task in natural language, e.g. 'Email Dana about the invoice tomorrow !high @work'."),
                        "due": s_("Override the due date (ISO YYYY-MM-DD)."),
                        "scheduled": s_("When you plan to work on it (ISO YYYY-MM-DD)."),
                        "priority": s_("'none' | 'low' | 'normal' | 'high' | 'critical'."),
                    }),
                    &["text"],
                )
            },
        },
        ToolDef {
            name: "update_task",
            plugin: "core",
            description: "Change a task by id: mark it done, reschedule it, re-prioritize, or \
                          retitle. Only the fields you pass change. Get ids from list_tasks.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Task UUID from list_tasks."),
                        "status": s_("New status, e.g. 'open', 'in-progress', 'done'."),
                        "title": s_("New title."),
                        "due": s_("New due date (ISO YYYY-MM-DD); pass an empty string to clear."),
                        "scheduled": s_("New scheduled date (ISO YYYY-MM-DD); empty string clears."),
                        "priority": s_("'none' | 'low' | 'normal' | 'high' | 'critical'."),
                        "assignees": a_("Replace who owns the task: 'agent:NAME' or 'human:USER_ID' entries (bare names mean agent:). Empty array unclaims. Prefer claim_task to take a task for yourself."),
                    }),
                    &["id"],
                )
            },
        },
        ToolDef {
            name: "list_untriaged_tasks",
            plugin: "core",
            description: "List open tasks that belong to nothing — no project, no parent task, \
                          no workstream, no @context. These are hidden from the user's \
                          'Relevant' view because a bare title like 'Telemetry: Sentry' says \
                          nothing about what it's for. Returns each task's title, body and \
                          age, plus the org's projects and open tasks that could be its home, \
                          so one call gives you everything needed to decide. Follow with \
                          file_task for each one you can place confidently.",
            schema: || {
                obj(
                    json!({
                        "limit": i_("Max tasks to triage this pass (default 50, max 200). Oldest first."),
                    }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "file_task",
            plugin: "core",
            description: "File a task under what it belongs to: a project, a parent task, a \
                          workstream, or GTD @contexts. This is the write that ends triage — \
                          a filed task rejoins the user's working list. Pass only what you're \
                          confident about; one correct anchor beats three guesses. If nothing \
                          fits, leave the task alone and say so rather than inventing a home \
                          for it.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Task UUID from list_untriaged_tasks."),
                        "project": s_("Project UUID it belongs to (from list_projects)."),
                        "parent": s_("Task UUID this is a subtask of — use when the task is one slice of a bigger tracked item."),
                        "workstream": s_("Workstream UUID this rolls up into."),
                        "contexts": a_("GTD contexts like '@studio', '@phone' — the right answer for standalone errands that have no project."),
                        "reason": s_("One line on why it belongs there. Recorded in the response so the user can audit your filing."),
                    }),
                    &["id"],
                )
            },
        },
    ];
    // Calendar tools ride the scheduling plugin.
    #[cfg(feature = "plugin-scheduling")]
    v.extend([
        ToolDef {
            name: "list_events",
            plugin: "scheduling",
            description: "List calendar events, optionally windowed by date. Always call this \
                          before rescheduling anything so you're moving real events and can see \
                          what a new time would collide with.",
            schema: || {
                obj(
                    json!({
                        "from": s_("ISO date (YYYY-MM-DD). Keeps events starting on/after it."),
                        "to": s_("ISO date (YYYY-MM-DD). Keeps events starting on/before it."),
                        "limit": i_("Max rows (default 50, max 200)."),
                    }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "create_event",
            plugin: "scheduling",
            description: "Put something on the calendar. Times are RFC-3339 with offset \
                          (e.g. '2026-07-25T14:00:00-05:00'); for an all-day event pass \
                          plain dates and all_day: true.",
            schema: || {
                obj(
                    json!({
                        "title": s_("What the event is."),
                        "start": s_("RFC-3339 start (inclusive), or YYYY-MM-DD when all_day."),
                        "end": s_("RFC-3339 end (exclusive), or YYYY-MM-DD when all_day."),
                        "all_day": b_("True for an all-day event."),
                        "description": s_("Optional notes on the event."),
                    }),
                    &["title", "start", "end"],
                )
            },
        },
        ToolDef {
            name: "reschedule_event",
            plugin: "scheduling",
            description: "Move an existing event to a new time, keeping everything else. This is \
                          the tool for 'rearrange my schedule' — call list_events first, then one \
                          reschedule per event you're moving.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Event id from list_events."),
                        "start": s_("New RFC-3339 start."),
                        "end": s_("New RFC-3339 end."),
                    }),
                    &["id", "start", "end"],
                )
            },
        },
        ToolDef {
            name: "cancel_event",
            plugin: "scheduling",
            description: "Remove an event from the calendar. Confirm with the user before \
                          cancelling anything you didn't create in this conversation.",
            schema: || obj(json!({ "id": s_("Event id from list_events.") }), &["id"]),
        },
    ]);
    v.extend([
        ToolDef {
            name: "capture_inbox",
            plugin: "core",
            description: "Capture a thought, reminder, or follow-up into the inbox — the right \
                          home for anything not yet shaped into a task. Captures land as \
                          'suggested' for the user to accept with one tap, so capture freely; \
                          pass accepted: true only when the user explicitly asked for it.",
            schema: || {
                obj(
                    json!({
                        "body": s_("The note, verbatim. Markdown."),
                        "accepted": b_("Skip the suggestion queue and file it as an open item."),
                        "resurface_on": s_("ISO date to bring it back up (a reminder)."),
                    }),
                    &["body"],
                )
            },
        },
        ToolDef {
            name: "list_inbox",
            plugin: "core",
            description: "What's waiting in the inbox for review today — the user's unprocessed \
                          captures. Useful for 'what did I say I'd look at'. Pass status \
                          'suggested' to see captures (including your own) still awaiting the \
                          user's accept.",
            schema: || {
                obj(
                    json!({
                        "status": s_("Show every item with this status instead of today's review queue, e.g. 'suggested', 'open'."),
                        "limit": i_("Max rows (default 50, max 200)."),
                    }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "list_projects",
            plugin: "core",
            description: "The user's projects. Use it to attach work to the right project, or to \
                          answer 'what am I working on'.",
            schema: || {
                obj(
                    json!({ "limit": i_("Max rows (default 50, max 200).") }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "list_goals",
            plugin: "core",
            description: "The user's goals — the longer horizon above projects. Consult before \
                          advising on priorities.",
            schema: || {
                obj(
                    json!({ "limit": i_("Max rows (default 50, max 200).") }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "search_vault",
            plugin: "core",
            description: "Find notes by path. The vault is the user's own writing — meeting \
                          notes, journals, references. Returns paths to pass to read_note.",
            schema: || {
                obj(
                    json!({
                        "query": s_("Case-insensitive substring matched against the file path."),
                        "limit": i_("Max rows (default 50, max 200)."),
                    }),
                    &["query"],
                )
            },
        },
        ToolDef {
            name: "read_note",
            plugin: "core",
            description: "Read one note's markdown by vault-relative path (from search_vault).",
            schema: || {
                obj(
                    json!({ "path": s_("Vault-relative path, e.g. 'Records/notes/standup.md'.") }),
                    &["path"],
                )
            },
        },
        ToolDef {
            name: "append_note",
            plugin: "core",
            description: "Append markdown to the end of an existing note. Use for adding to a \
                          running log or daily note; it never overwrites what's there.",
            schema: || {
                obj(
                    json!({
                        "path": s_("Vault-relative path of an existing note."),
                        "text": s_("Markdown to append. A blank line is inserted before it."),
                    }),
                    &["path", "text"],
                )
            },
        },
        ToolDef {
            name: "write_note",
            plugin: "core",
            description: "Create a note, or REPLACE an existing one wholesale. Destructive on \
                          existing paths — read_note first and prefer append_note for adding to \
                          a note that already exists.",
            schema: || {
                obj(
                    json!({
                        "path": s_("Vault-relative path, e.g. 'Records/notes/plan.md'."),
                        "content": s_("The note's full markdown content."),
                    }),
                    &["path", "content"],
                )
            },
        },
        // ── Task workflow ────────────────────────────────────────
        ToolDef {
            name: "claim_task",
            plugin: "core",
            description: "Atomically claim a task before working on it — exactly one caller wins \
                          when several agents try; losers are told who holds it. Use this instead \
                          of update_task assignees when taking work for yourself.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Task UUID from list_tasks."),
                        "agent": s_("Who is claiming: 'agent:NAME' or 'human:USER_ID' (a bare name means agent:)."),
                        "force": b_("Steal an existing claim. Only with the user's explicit say-so."),
                    }),
                    &["id", "agent"],
                )
            },
        },
        // ── Projects / goals / milestones ────────────────────────
        ToolDef {
            name: "create_project",
            plugin: "core",
            description: "Create a project — a container of related tasks with its own vault \
                          page. Check list_projects first so you don't duplicate one that \
                          already exists.",
            schema: || {
                obj(
                    json!({
                        "title": s_("Project name."),
                        "status": s_("'active' (default) | 'planned' | 'paused' | 'done' | 'cancelled'."),
                        "priority": s_("'urgent' | 'high' | 'normal' (default) | 'low'."),
                        "parent_id": s_("UUID of a parent project, for sub-projects."),
                        "details": s_("Markdown body — scope, links, context."),
                    }),
                    &["title"],
                )
            },
        },
        ToolDef {
            name: "update_project",
            plugin: "core",
            description: "Change a project by id: retitle, set status/priority, or replace its \
                          markdown body. Only the fields you pass change.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Project UUID from list_projects."),
                        "title": s_("New name."),
                        "status": s_("New status, e.g. 'active', 'paused', 'done'."),
                        "priority": s_("New priority."),
                        "details": s_("REPLACES the whole markdown body."),
                    }),
                    &["id"],
                )
            },
        },
        ToolDef {
            name: "create_goal",
            plugin: "core",
            description: "Create a goal — the horizon above projects ('run a marathon', 'ship \
                          the album'). Kind sets the horizon: lifetime (default), yearly, \
                          quarterly, cycle, weekly.",
            schema: || {
                obj(
                    json!({
                        "title": s_("The goal."),
                        "kind": s_("'lifetime' (default) | 'yearly' | 'quarterly' | 'cycle' | 'weekly'."),
                        "status": s_("'aspiration' (default) | 'active' | 'paused' | 'achieved' | 'abandoned'."),
                        "target_date": s_("ISO date (YYYY-MM-DD) the goal aims at."),
                        "details": s_("Markdown body — vision, success criteria."),
                    }),
                    &["title"],
                )
            },
        },
        ToolDef {
            name: "update_goal",
            plugin: "core",
            description: "Change a goal by id: retitle, move it through its lifecycle \
                          (aspiration → active → achieved), or shift the target date. Only the \
                          fields you pass change.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Goal UUID from list_goals."),
                        "title": s_("New title."),
                        "status": s_("New status, e.g. 'active', 'achieved'."),
                        "kind": s_("New horizon kind."),
                        "target_date": s_("New target date (YYYY-MM-DD); empty string clears."),
                    }),
                    &["id"],
                )
            },
        },
        ToolDef {
            name: "list_milestones",
            plugin: "core",
            description: "Milestones — dated checkpoints inside a project that tasks roll up \
                          to. Filter by project to see one project's roadmap.",
            schema: || {
                obj(
                    json!({
                        "project": s_("Project UUID — only that project's milestones."),
                        "limit": i_("Max rows (default 50, max 200)."),
                    }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "create_milestone",
            plugin: "core",
            description: "Create a milestone inside a project (project_id from list_projects). \
                          Optionally point it at a goal to build the task → milestone → goal \
                          chain.",
            schema: || {
                obj(
                    json!({
                        "project_id": s_("Owning project UUID. Required."),
                        "title": s_("The checkpoint, e.g. 'v1.0 shipped'."),
                        "due_date": s_("Target date (YYYY-MM-DD)."),
                        "goal_id": s_("Goal UUID this milestone ladders up to."),
                        "details": s_("Markdown description."),
                    }),
                    &["project_id", "title"],
                )
            },
        },
        ToolDef {
            name: "update_milestone",
            plugin: "core",
            description: "Change a milestone by id: retitle, close it ('closed'), or move its \
                          due date. Only the fields you pass change.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Milestone UUID from list_milestones."),
                        "title": s_("New title."),
                        "status": s_("'open' | 'closed'."),
                        "due_date": s_("New due date (YYYY-MM-DD); empty string clears."),
                    }),
                    &["id"],
                )
            },
        },
        // ── Inbox processing ─────────────────────────────────────
        ToolDef {
            name: "process_inbox_item",
            plugin: "core",
            description: "Move an inbox item through its lifecycle: 'open' accepts a suggestion \
                          into the trusted queue, 'processed' marks it handled (e.g. after you \
                          turned it into a task), 'archived' dismisses it. Get ids from \
                          list_inbox.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Inbox item id from list_inbox."),
                        "status": s_("'open' | 'processed' | 'archived'."),
                    }),
                    &["id", "status"],
                )
            },
        },
        // ── Day plans / bookings (scheduling plugin) ─────────────
        ToolDef {
            name: "get_day_plan",
            plugin: "scheduling",
            description: "The time-blocked plan for one day (blocks with start/end/label/\
                          category), or null when the day has no plan yet. Read this before \
                          upsert_day_plan so you edit the real blocks.",
            schema: || {
                obj(
                    json!({ "date": s_("ISO date (YYYY-MM-DD).") }),
                    &["date"],
                )
            },
        },
        ToolDef {
            name: "upsert_day_plan",
            plugin: "scheduling",
            description: "Write one day's time-blocked plan. REPLACES the whole day — call \
                          get_day_plan first and resend every block you want to keep. Times are \
                          'HH:MM' (24h).",
            schema: || {
                obj(
                    json!({
                        "date": s_("ISO date (YYYY-MM-DD)."),
                        "blocks": {
                            "type": "array",
                            "description": "The day's blocks, in order.",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "start": s_("'HH:MM' 24h start."),
                                    "end": s_("'HH:MM' 24h end."),
                                    "label": s_("What the block is, e.g. 'Deep work'."),
                                    "category": s_("One of: reset, spiritual, meal, exercise, hygiene, allocatable, maintenance, wind_down, sleep, other (default)."),
                                    "note": s_("Optional free-text note."),
                                    "fixed": b_("Immovable — reflow never shifts it."),
                                },
                                "required": ["start", "end", "label"],
                                "additionalProperties": false,
                            },
                        },
                    }),
                    &["date", "blocks"],
                )
            },
        },
        ToolDef {
            name: "list_bookings",
            plugin: "scheduling",
            description: "Bookings people have made against the user's bookable event types — \
                          who, when, and their status (pending/confirmed/cancelled).",
            schema: || {
                obj(
                    json!({ "limit": i_("Max rows (default 50, max 200).") }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "list_open_slots",
            plugin: "scheduling",
            description: "Free bookable slots for an event type inside a UTC window — call \
                          before book_slot so you offer times that are actually open.",
            schema: || {
                obj(
                    json!({
                        "event_type_id": s_("The bookable event type's id."),
                        "from": s_("ISO-8601 UTC window start (inclusive)."),
                        "to": s_("ISO-8601 UTC window end (exclusive)."),
                    }),
                    &["event_type_id", "from", "to"],
                )
            },
        },
        ToolDef {
            name: "book_slot",
            plugin: "scheduling",
            description: "Book an open slot (from list_open_slots) for an attendee. Fails if \
                          the slot was taken in the meantime — re-query and offer another.",
            schema: || {
                obj(
                    json!({
                        "event_type_id": s_("The event type being booked."),
                        "start": s_("Slot start, ISO-8601 UTC (from list_open_slots)."),
                        "end": s_("Slot end, ISO-8601 UTC."),
                        "attendee_name": s_("Who the booking is for."),
                        "attendee_email": s_("Their email."),
                        "note": s_("Optional note on the booking."),
                    }),
                    &["event_type_id", "start", "end", "attendee_name", "attendee_email"],
                )
            },
        },
        ToolDef {
            name: "cancel_booking",
            plugin: "scheduling",
            description: "Cancel a booking by id (from list_bookings). Confirm with the user \
                          before cancelling a confirmed booking.",
            schema: || {
                obj(
                    json!({ "id": s_("Booking id from list_bookings.") }),
                    &["id"],
                )
            },
        },
        // ── Contacts ─────────────────────────────────────────────
        ToolDef {
            name: "list_contacts",
            plugin: "contacts",
            description: "The user's people directory. Search by name, email, or organization \
                          to find who they mean; returns ids for upsert_contact.",
            schema: || {
                obj(
                    json!({
                        "query": s_("Case-insensitive match against name, emails, and organization."),
                        "limit": i_("Max rows (default 50, max 200)."),
                    }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "upsert_contact",
            plugin: "contacts",
            description: "Create a contact, or update one by id (from list_contacts). On \
                          update, only the fields you pass change.",
            schema: || {
                obj(
                    json!({
                        "id": s_("Existing contact id to update. Omit to create."),
                        "name": s_("Display name. Required when creating."),
                        "emails": a_("Email addresses, primary first. Replaces the list."),
                        "phones": a_("Phone numbers, primary first. Replaces the list."),
                        "organization": s_("Company / organization."),
                        "notes": s_("Free-form notes."),
                    }),
                    &[],
                )
            },
        },
        // ── Email (read + draft only — no send tool, by design) ──
        ToolDef {
            name: "list_email_accounts",
            plugin: "email",
            description: "The mail accounts this org serves, with their folders. Call first to \
                          learn the account id + folder names list_envelopes needs.",
            schema: || obj(json!({}), &[]),
        },
        ToolDef {
            name: "list_envelopes",
            plugin: "email",
            description: "Message summaries (subject, from, date, snippet) for one folder, \
                          newest first. Returns message ids for read_email.",
            schema: || {
                obj(
                    json!({
                        "account": s_("Account id from list_email_accounts."),
                        "folder": s_("Folder name (from list_email_accounts). Default 'INBOX'."),
                        "limit": i_("Most recent N (default 50, max 200)."),
                    }),
                    &["account"],
                )
            },
        },
        ToolDef {
            name: "read_email",
            plugin: "email",
            description: "One full message by id — headers, text body, attachment names. Read \
                          before drafting a reply.",
            schema: || {
                obj(
                    json!({
                        "account": s_("Account id."),
                        "message_id": s_("Message id from list_envelopes."),
                    }),
                    &["account", "message_id"],
                )
            },
        },
        ToolDef {
            name: "draft_email",
            plugin: "email",
            description: "Compose a NEW message into the account's Drafts folder. Nothing is \
                          sent — the user reviews and sends from their mail client. There is \
                          deliberately no send tool.",
            schema: || {
                obj(
                    json!({
                        "account": s_("Account id from list_email_accounts."),
                        "to": a_("Recipient email addresses."),
                        "cc": a_("CC addresses."),
                        "subject": s_("Subject line."),
                        "body": s_("Plain-text body."),
                    }),
                    &["account", "to", "subject", "body"],
                )
            },
        },
        // ── Discovery ────────────────────────────────────────────
        ToolDef {
            name: "api_reference",
            plugin: "core",
            description: "The org's ENTIRE server API — every vox service and method, with \
                          the permit each needs and whether its plugin is enabled here. The MCP \
                          tools are a curated subset; call this to answer 'can Task do X?' or to \
                          see what exists beyond the tools. Pass `service` for one service's \
                          full detail (argument names, permits, docs).",
            schema: || {
                obj(
                    json!({
                        "service": s_("Service name or alias to expand (substring match), e.g. 'task', 'wiki-search'."),
                    }),
                    &[],
                )
            },
        },
        ToolDef {
            name: "draft_reply",
            plugin: "email",
            description: "Draft a reply to a message (id from list_envelopes) into Drafts, \
                          with correct To/Re:/threading headers. Nothing is sent — the user \
                          reviews and sends from their mail client.",
            schema: || {
                obj(
                    json!({
                        "account": s_("Account id."),
                        "message_id": s_("The message being replied to."),
                        "body": s_("Plain-text reply body."),
                        "reply_all": b_("CC everyone on the original message."),
                    }),
                    &["account", "message_id", "body"],
                )
            },
        },
    ]);
    v
}

/// The catalog as MCP's `tools/list` payload, filtered to the org's
/// enabled plugins — a disabled plugin's tools are *absent*, exactly
/// as its vox services are unmounted from the org router.
fn tools_list_payload(plugins: &task_plugin::PluginSet) -> Value {
    let tools: Vec<Value> = tool_catalog()
        .iter()
        .filter(|t| plugins.contains(t.plugin))
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": (t.schema)(),
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// The plugin gate for one `tools/call`, mirroring [`tools_list_payload`]:
/// - unknown tool → `Err(None)` (protocol-level method-not-found);
/// - tool of a disabled plugin → `Err(Some(msg))` (tool-level error the
///   model reads — the tool exists in the build, this org turned it off);
/// - enabled → `Ok(())`.
fn plugin_gate(name: &str, plugins: &task_plugin::PluginSet) -> Result<(), Option<String>> {
    let Some(def) = tool_catalog().into_iter().find(|t| t.name == name) else {
        return Err(None);
    };
    if plugins.contains(def.plugin) {
        return Ok(());
    }
    let plugin_name = task_plugin::find(def.plugin).map_or(def.plugin, |p| p.name);
    Err(Some(format!(
        "`{name}` is unavailable: the {plugin_name} plugin (`{}`) is disabled for this org. \
         Call `tools/list` for what this org serves.",
        def.plugin,
    )))
}

/// The system-prompt text MCP clients inject on connect. This is
/// what makes the agent aware it is *inside* Task rather than
/// holding a bag of unrelated tools.
#[must_use]
pub fn server_instructions(slug: &str, now: chrono::DateTime<chrono::Local>) -> String {
    format!(
        "You are connected to Task — the user's personal operating system. Task is a \
local-first vault of markdown notes with structured overlays on top: tasks, projects, \
goals, a calendar, and a capture inbox. You are not looking at a generic database; \
these are the user's real commitments, and changes you make show up immediately in \
their app.\n\
\n\
Working context: org `{slug}`. Right now it is {stamp} ({weekday}). Resolve every \
relative date the user says — \"tomorrow\", \"next Tuesday\", \"end of the month\" — \
against that, and pass absolute ISO dates to tools.\n\
\n\
How to work here:\n\
- Never invent an id. Call `list_tasks` / `list_events` first, then act on what came back.\n\
- Prefer `create_task` with the user's own phrasing — it parses tags, contexts, \
projects, priority, and dates the same way the app's quick-add does.\n\
- Anything half-formed — a reminder, a follow-up, a thought worth keeping — belongs in \
`capture_inbox`, not in a task. Captures arrive as suggestions the user accepts with one \
tap, so err on the side of capturing.\n\
- Rescheduling means `list_events` for the window, then one `reschedule_event` per move. \
Say what you're about to move before you move it.\n\
- Read before you write: `search_vault` then `read_note`. The user's notes are usually \
better context than your assumptions.\n\
- Taking on a task yourself? `claim_task` it first — claims are atomic, so parallel \
agents can't collide on the same work.\n\
- Email is read-and-draft only: `draft_email` / `draft_reply` land in Drafts for the \
user to send. There is no send tool; say so when the user asks you to \"send\".\n\
- `api_reference` shows everything this org's server can do (every service and method, \
including what has no dedicated tool here) — use it to answer \"can Task do X?\".\n\
- Confirm before destructive or wide-reaching changes (cancelling events you didn't just \
create, bulk status changes, overwriting notes with `write_note`). Ordinary additions \
don't need a confirmation round-trip.",
        stamp = now.format("%A, %B %-d, %Y at %-I:%M %p %:z"),
        weekday = now.format("%A"),
    )
}

// ── HTTP surface ─────────────────────────────────────────────────

/// `POST /mcp` — the ACCOUNT-scoped lane.
///
/// One endpoint per account rather than one per org. Every per-org
/// tool grows an optional `org` argument (defaulting to the caller's
/// home), and `list_orgs` reports what's reachable. Registering six
/// MCP servers for six orgs was the wrong shape: the client had to
/// know the topology, and adding an org meant editing client config.
pub async fn mcp_account_handler(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: String,
) -> Response {
    mcp_dispatch(state, None, headers, body).await
}

/// `POST /org/{slug}/mcp` — the org-scoped lane, unchanged.
pub async fn mcp_handler(
    State(state): State<AppState>,
    Path(slug): Path<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    mcp_dispatch(state, Some(slug), headers, body).await
}

/// Shared JSON-RPC dispatch. `pinned` is `Some(slug)` on the org lane
/// and `None` on the account lane, where each `tools/call` names its
/// own org.
async fn mcp_dispatch(
    state: AppState,
    pinned: Option<String>,
    headers: HeaderMap,
    body: String,
) -> Response {
    let request: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(e) => {
            return Json(rpc_error(
                Value::Null,
                code::PARSE,
                format!("invalid JSON: {e}"),
            ))
            .into_response();
        }
    };
    // The current MCP revision dropped JSON-RPC batching.
    if request.is_array() {
        return Json(rpc_error(
            Value::Null,
            code::INVALID_REQUEST,
            "batched requests are not supported",
        ))
        .into_response();
    }
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return Json(rpc_error(id, code::INVALID_REQUEST, "missing `method`")).into_response();
    };
    let params = request.get("params").cloned().unwrap_or_else(|| json!({}));

    // Notifications carry no id and take no response body.
    if id.is_null() && method.starts_with("notifications/") {
        return StatusCode::ACCEPTED.into_response();
    }

    match method {
        // `initialize` is unauthenticated on purpose: a client must be
        // able to discover the server (and learn it needs a token)
        // before it can be told its token is wrong.
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Value::as_str)
                .unwrap_or(LATEST_PROTOCOL);
            let version = if SUPPORTED_PROTOCOLS.contains(&requested) {
                requested
            } else {
                LATEST_PROTOCOL
            };
            let (name, instructions) = match &pinned {
                Some(s) => (
                    format!("task/{s}"),
                    server_instructions(s, chrono::Local::now()),
                ),
                None => (
                    "task".to_string(),
                    account_instructions(&state, &headers, chrono::Local::now()).await,
                ),
            };
            Json(rpc_result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": { "listChanged": false } },
                    "serverInfo": { "name": name, "version": env!("CARGO_PKG_VERSION") },
                    "instructions": instructions,
                }),
            ))
            .into_response()
        }
        "ping" => Json(rpc_result(id, json!({}))).into_response(),
        "tools/list" => {
            let Some(target) = default_slug(&state, &pinned, &headers).await else {
                return Json(rpc_error(
                    id,
                    code::INVALID_REQUEST,
                    "no reachable org for this token",
                ))
                .into_response();
            };
            match authenticate_for(&state, &target, &headers).await {
                Ok(org) => {
                    let mut payload = tools_list_payload(&org.plugins);
                    if pinned.is_none() {
                        payload = account_tools_payload(payload, &target);
                        // Cluster telemetry: listed only when a backend
                        // exists, so a self-hoster without a stack never
                        // sees tools that can only say "not configured".
                        if telemetry_query::TelemetryConfig::from_env().any()
                            && let Some(tools) = payload["tools"].as_array_mut()
                        {
                            tools.extend(telemetry_tools_payload());
                        }
                    }
                    Json(rpc_result(id, payload)).into_response()
                }
                Err(e) => Json(rpc_error(id, code::INVALID_REQUEST, e)).into_response(),
            }
        }
        "tools/call" => {
            let args_peek = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            // Account lane: the call names its org, else the default.
            let target = match &pinned {
                Some(s) => s.clone(),
                None => match arg_str(&args_peek, "org") {
                    Some(s) => s,
                    None => match default_slug(&state, &pinned, &headers).await {
                        Some(s) => s,
                        None => {
                            return Json(rpc_error(
                                id,
                                code::INVALID_REQUEST,
                                "no reachable org — pass `org`, or check `list_orgs`",
                            ))
                            .into_response();
                        }
                    },
                },
            };
            // `list_orgs` is account-lane only and spans orgs, so it
            // answers before any single-org authentication.
            if pinned.is_none() && params.get("name").and_then(Value::as_str) == Some("list_orgs") {
                let orgs = reachable_orgs(&state, &headers).await;
                let home = state.home_slug();
                let out: Vec<Value> = orgs
                    .iter()
                    .map(|s| json!({ "slug": s, "is_home": Some(s) == home.as_ref() }))
                    .collect();
                return Json(rpc_result(
                    id,
                    tool_ok(&json!({
                        "count": out.len(),
                        "orgs": out,
                        "default": target,
                        "note": "Each org has its OWN vault. Pass `org` on any tool to act in \
                                 one; omitting it uses the default.",
                    })),
                ))
                .into_response();
            }
            // `telemetry_*` is account-lane only and spans orgs too: it
            // has its own operator check rather than a per-org session,
            // and is method-not-found when no backend is configured.
            if pinned.is_none()
                && let Some(name) = params.get("name").and_then(Value::as_str)
                && TELEMETRY_TOOLS.contains(&name)
            {
                return match telemetry_call(&state, &headers, name, &args_peek).await {
                    Some(payload) => Json(rpc_result(id, payload)).into_response(),
                    None => Json(rpc_error(
                        id,
                        code::METHOD_NOT_FOUND,
                        format!("unknown tool `{name}`"),
                    ))
                    .into_response(),
                };
            }
            let org = match authenticate_for(&state, &target, &headers).await {
                Ok(org) => org,
                Err(e) => return Json(rpc_error(id, code::INVALID_REQUEST, e)).into_response(),
            };
            let Some(name) = params.get("name").and_then(Value::as_str) else {
                return Json(rpc_error(id, code::INVALID_PARAMS, "missing tool `name`"))
                    .into_response();
            };
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or_else(|| json!({}));
            // Plugin gate FIRST — a disabled plugin's tool must fail
            // the same way whether or not its backend would have
            // answered, and before any backend work happens.
            match plugin_gate(name, &org.plugins) {
                Ok(()) => {}
                Err(None) => {
                    return Json(rpc_error(
                        id,
                        code::METHOD_NOT_FOUND,
                        format!("unknown tool `{name}`"),
                    ))
                    .into_response();
                }
                Err(Some(msg)) => {
                    return Json(rpc_result(id, tool_err(msg))).into_response();
                }
            }
            // The domain backends are the same sync trait impls the
            // vox layer serves, and they're written for architect's
            // blocking dispatcher — `vault_sync` in particular takes
            // `blocking_read` locks, which panics on an async worker.
            // Run the whole dispatch on the blocking pool, which also
            // keeps a vault-walking tool off the reactor.
            let called = name.to_string();
            let dispatched =
                tokio::task::spawn_blocking(move || call_tool(&org, &called, &args)).await;
            let outcome = match dispatched {
                Ok(r) => r,
                Err(e) => Err(ToolFailure::Message(format!("tool panicked: {e}"))),
            };
            let payload = match outcome {
                Ok(v) => tool_ok(&v),
                // Unknown tool is a protocol error; anything else is a
                // tool-level failure the model should read and retry.
                Err(ToolFailure::Unknown) => {
                    return Json(rpc_error(
                        id,
                        code::METHOD_NOT_FOUND,
                        format!("unknown tool `{name}`"),
                    ))
                    .into_response();
                }
                Err(ToolFailure::Message(m)) => tool_err(m),
            };
            Json(rpc_result(id, payload)).into_response()
        }
        // Advertised-as-absent capabilities: answer emptily rather
        // than erroring, so clients that probe anyway stay happy.
        "resources/list" => Json(rpc_result(id, json!({ "resources": [] }))).into_response(),
        "prompts/list" => Json(rpc_result(id, json!({ "prompts": [] }))).into_response(),
        other => Json(rpc_error(
            id,
            code::METHOD_NOT_FOUND,
            format!("unsupported method `{other}`"),
        ))
        .into_response(),
    }
}

/// The org a call lands in when it doesn't name one: the pinned slug
/// on the org lane, else the caller's home, else the first org they
/// can reach.
async fn default_slug(
    state: &AppState,
    pinned: &Option<String>,
    headers: &HeaderMap,
) -> Option<String> {
    if let Some(s) = pinned {
        return Some(s.clone());
    }
    let reachable = reachable_orgs(state, headers).await;
    if reachable.is_empty() {
        return None;
    }
    match state.home_slug() {
        Some(home) if reachable.contains(&home) => Some(home),
        _ => reachable.first().cloned(),
    }
}

/// Rewrite a per-org tools payload for the account lane: every tool
/// gains `org`, and `list_orgs` is prepended so a client can discover
/// the topology before it guesses at slugs.
fn account_tools_payload(payload: Value, default_slug: &str) -> Value {
    let mut tools: Vec<Value> = payload
        .get("tools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|mut t| {
            if let Some(schema) = t.get("inputSchema").cloned() {
                t["inputSchema"] = with_org_param(schema, default_slug);
            }
            t
        })
        .collect();
    tools.insert(
        0,
        json!({
            "name": "list_orgs",
            "description": "The organizations this account can reach, and which one tools \
                            default to. Each org is a SEPARATE vault — tasks, projects and \
                            notes do not cross between them. Call this before assuming where \
                            something lives.",
            "inputSchema": obj(json!({}), &[]),
        }),
    );
    json!({ "tools": tools })
}

// ── Telemetry tools (account lane, operator only) ────────────────
//
// The cluster's traces and logs span every org, so these are not org
// tools and never take `org`. They exist on `POST /mcp` only, appear
// only when a backend URL is configured (`TASK_TELEMETRY_TEMPO_URL` /
// `TASK_TELEMETRY_LOKI_URL`), and answer only an OPERATOR: the static
// `TASK_MCP_TOKEN`, or a session whose principal holds `admin` in the
// server's home org. Everyone else gets a tool-level refusal.

/// The `telemetry_*` tool names, in listing order.
const TELEMETRY_TOOLS: &[&str] = &[
    "telemetry_status",
    "telemetry_query_traces",
    "telemetry_get_trace",
    "telemetry_query_logs",
];

/// `tools/list` entries for the telemetry tools. Descriptions carry
/// example queries because the reader is a model that has to write
/// TraceQL/LogQL from them.
fn telemetry_tools_payload() -> Vec<Value> {
    vec![
        json!({
            "name": "telemetry_status",
            "description": "Which telemetry backends this server can read (Tempo for traces, \
                            Loki for logs) and whether YOU are allowed to query them. Call \
                            this first when a telemetry tool refuses or errors. Operator-only: \
                            the cluster's telemetry spans every org.",
            "inputSchema": obj(json!({}), &[]),
        }),
        json!({
            "name": "telemetry_query_traces",
            "description": "Search the cluster's traces with TraceQL (Tempo). Every task-server \
                            request is ONE span carrying wide-event fields — auth.*, perm.*, \
                            rpc.*, org.slug, mcp.tool — so filter on those rather than on log \
                            text. Returns one row per matching trace; follow up with \
                            telemetry_get_trace for the spans. Examples: \
                            `{ resource.service.name = \"task-server\" && span.auth.outcome = \"rejected\" }` \
                            (sessions being refused); \
                            `{ span.rpc.service = \"TimerService\" && duration > 500ms }` (slow RPCs); \
                            `{ span.perm.decision = \"deny\" && span.org.slug = \"fasttrackstudios\" }` \
                            (denials in one org); `{ span.mcp.tool = \"create_task\" }` (this lane). \
                            Operator-only.",
            "inputSchema": obj(
                json!({
                    "traceql": s_("TraceQL query, e.g. `{ resource.service.name = \"task-server\" && status = error }`."),
                    "since": s_("Lookback window ending now: `15m`, `2h`, `1d` (max 30d). Default `1h`."),
                    "limit": i_("Max traces to return (1-200, default 20)."),
                }),
                &["traceql"],
            ),
        }),
        json!({
            "name": "telemetry_get_trace",
            "description": "One trace by id (from telemetry_query_traces), as its spans sorted \
                            by start: service, name, duration_ms, status, and every attribute \
                            flattened to `key: value`. This is where the wide event lives — \
                            read auth.outcome, perm.decision, perm.reason, org.slug, rpc.method \
                            off the span rather than guessing from logs. Operator-only.",
            "inputSchema": obj(
                json!({
                    "trace_id": s_("Hex trace id exactly as telemetry_query_traces returned it."),
                }),
                &["trace_id"],
            ),
        }),
        json!({
            "name": "telemetry_query_logs",
            "description": "Search the cluster's logs with LogQL (Loki), newest first, ANSI \
                            colour stripped. Reach for this for boot lines, panics, and the \
                            one-line denial/refusal warnings — allowed requests write NO log \
                            line, only a span, so use telemetry_query_traces for those. \
                            Examples: `{service_name=\"task\"} |= \"central auth\"`; \
                            `{namespace=\"task\", container=\"server\"} |~ \"panic|ERROR\"`; \
                            `{service_name=\"task\"} | json | perm_decision=\"deny\"`. \
                            Labels available: service_name (`task`), namespace (`task`), app, container (`server`). \
                            Operator-only.",
            "inputSchema": obj(
                json!({
                    "logql": s_("LogQL query, e.g. `{service_name=\"task\"} |= \"warn\"`."),
                    "since": s_("Lookback window ending now: `15m`, `2h`, `1d` (max 30d). Default `1h`."),
                    "limit": i_("Max log lines to return (1-200, default 20)."),
                }),
                &["logql"],
            ),
        }),
    ]
}

/// One `telemetry_*` call, end to end: configured? operator? then the
/// backend. Returns the MCP tool result (`tool_ok` / `tool_err`), or
/// `None` when the tools are hidden (no backend configured), which the
/// dispatcher turns into method-not-found — the same answer an
/// unlisted tool gets.
async fn telemetry_call(
    state: &AppState,
    headers: &HeaderMap,
    name: &str,
    args: &Value,
) -> Option<Value> {
    use architect_telemetry::wide;
    use telemetry_query::{QueryError, TelemetryClient, TelemetryConfig, clamp_limit};

    let cfg = TelemetryConfig::from_env();
    if !cfg.any() {
        return None;
    }
    wide::set("mcp.tool", name.to_owned());
    let operator = crate::operator::is_operator(state, headers).await;
    if name == "telemetry_status" {
        wide::set("telemetry.outcome", if operator { "ok" } else { "refused" });
        return Some(tool_ok(&json!({
            "backends": cfg.describe(),
            "allowed": operator,
            "note": if operator {
                "You may query. Traces: telemetry_query_traces / telemetry_get_trace. \
                 Logs: telemetry_query_logs."
            } else {
                "Telemetry spans every org, so it needs an operator: the server's static \
                 MCP token, or an `admin` in the home org."
            },
        })));
    }
    if !operator {
        wide::set("telemetry.outcome", "refused");
        return Some(tool_err(
            "operator role required: telemetry spans every org on this server. Use the \
             static MCP token, or an account holding `admin` in the home org.",
        ));
    }
    let client = TelemetryClient::new(cfg);
    let since = arg_str(args, "since").unwrap_or_else(|| telemetry_query::DEFAULT_SINCE.to_owned());
    let limit = clamp_limit(args.get("limit").and_then(Value::as_u64));
    let (backend, result) = match name {
        "telemetry_query_traces" => (
            "tempo",
            match required_str(args, "traceql") {
                Ok(q) => client.search_traces(&q, &since, limit).await,
                Err(ToolFailure::Message(m)) => Err(QueryError::BadRequest(m)),
                Err(ToolFailure::Unknown) => return None,
            },
        ),
        "telemetry_get_trace" => (
            "tempo",
            match required_str(args, "trace_id") {
                Ok(id) => client.get_trace(&id).await,
                Err(ToolFailure::Message(m)) => Err(QueryError::BadRequest(m)),
                Err(ToolFailure::Unknown) => return None,
            },
        ),
        "telemetry_query_logs" => (
            "loki",
            match required_str(args, "logql") {
                Ok(q) => client.query_logs(&q, &since, limit).await,
                Err(ToolFailure::Message(m)) => Err(QueryError::BadRequest(m)),
                Err(ToolFailure::Unknown) => return None,
            },
        ),
        _ => return None,
    };
    wide::set("telemetry.backend", backend);
    Some(match result {
        Ok(v) => {
            wide::set("telemetry.outcome", "ok");
            tool_ok(&v)
        }
        Err(e) => {
            wide::set("telemetry.outcome", e.outcome());
            tool_err(e.to_string())
        }
    })
}

/// Account-lane orientation: the org-lane text for the default org,
/// plus the list of everything else this account can act in.
async fn account_instructions(
    state: &AppState,
    headers: &HeaderMap,
    now: chrono::DateTime<chrono::Local>,
) -> String {
    let reachable = reachable_orgs(state, headers).await;
    let default = default_slug(state, &None, headers)
        .await
        .unwrap_or_else(|| "(none)".into());
    let base = server_instructions(&default, now);
    if reachable.len() <= 1 {
        return base;
    }
    format!(
        "{base}\n\nThis connection is ACCOUNT-scoped, not org-scoped. Reachable orgs: {}. \
Every tool takes an optional `org` argument; omitting it acts in `{default}`. The orgs have \
SEPARATE vaults — a task in one is invisible in another, so when the user names a project or \
song you don't recognise, check `list_orgs` rather than assuming it is missing.",
        reachable.join(", ")
    )
}

/// Which orgs this caller can reach, and how.
///
/// The static `TASK_MCP_TOKEN` reaches every hosted org — that is what
/// it is for, and how the cluster agent works today. A *session* token
/// reaches the org that issued it, plus every org that account has
/// linked into its home identity locker. The locker is the whole point:
/// auth stores are per-org, so without it a session is a credential in
/// exactly one place.
pub async fn reachable_orgs(state: &AppState, headers: &HeaderMap) -> Vec<String> {
    let Some(token) = crate::watch_bridge::bearer(headers) else {
        return Vec::new();
    };
    let hosted = state.org_slugs();
    let static_token = std::env::var("TASK_MCP_TOKEN").unwrap_or_default();
    if !static_token.is_empty() && token == static_token {
        return hosted;
    }
    let mut reachable = Vec::new();
    // The issuing org, found by asking each hosted org whether the
    // token validates there. Cheap (one indexed lookup per org) and it
    // needs no guess about which org the caller came from.
    for slug in &hosted {
        if let Some(org) = state.org(slug)
            && org
                .auth
                .auth
                .current_session(architect_auth::CurrentSession {
                    token: token.clone(),
                })
                .await
                .is_ok()
        {
            reachable.push(slug.clone());
        }
    }
    // Plus everything the locker holds a credential for.
    for slug in linked_slugs(state, &token).await {
        if !reachable.contains(&slug) && hosted.contains(&slug) {
            reachable.push(slug);
        }
    }
    // Plus every org the memberships table says this principal belongs
    // to — the only way a token the issuer minted reaches anything,
    // since no org's own store ever saw it.
    for slug in member_slugs(state, &token).await {
        if !reachable.contains(&slug) && hosted.contains(&slug) {
            reachable.push(slug);
        }
    }
    reachable.sort();
    reachable
}

/// Org slugs the home org's memberships table holds for this token's
/// principal — a home-org session or, with central auth, an account the
/// issuer vouches for (`central_auth::home_principal`). Empty when the
/// token is neither, or the server has no home identity.
async fn member_slugs(state: &AppState, token: &str) -> Vec<String> {
    let Some(home) = &state.home_identity else {
        return Vec::new();
    };
    let Some(user_id) = crate::central_auth::home_principal(state, token).await else {
        return Vec::new();
    };
    home.memberships
        .for_user(user_id)
        .await
        .map(|rows| rows.into_iter().map(|m| m.org_slug).collect())
        .unwrap_or_default()
}

/// Org slugs the caller's home identity locker holds a link for.
/// Empty when the token isn't a home session, or the server hosts no
/// home org — both normal, neither an error.
async fn linked_slugs(state: &AppState, token: &str) -> Vec<String> {
    let Some(home_slug) = state.home_slug() else {
        return Vec::new();
    };
    let Some(home) = state.org(&home_slug) else {
        return Vec::new();
    };
    let Ok(bundle) = home
        .auth
        .auth
        .current_session(architect_auth::CurrentSession {
            token: token.to_owned(),
        })
        .await
    else {
        return Vec::new();
    };
    let Some(store) = home.identity else {
        return Vec::new();
    };
    store
        .list_links(bundle.user.id)
        .await
        .map(|links| links.into_iter().map(|l| l.remote_slug).collect())
        .unwrap_or_default()
}

/// Authenticate for ONE org on the account-scoped lane.
///
/// Three ways in, tried in order: the static token; the presented
/// token validating against this org directly; or a locker link whose
/// stored token does. The third is what lets one sign-in reach six
/// orgs without the client holding six credentials.
async fn authenticate_for(
    state: &AppState,
    slug: &str,
    headers: &HeaderMap,
) -> Result<crate::OrgAppState, String> {
    let token = crate::watch_bridge::bearer(headers).ok_or("missing bearer token")?;
    let org = state
        .org(slug)
        .ok_or_else(|| format!("org `{slug}` not hosted"))?;

    let static_token = std::env::var("TASK_MCP_TOKEN").unwrap_or_default();
    if !static_token.is_empty() && token == static_token {
        return Ok(org);
    }
    if org
        .auth
        .auth
        .current_session(architect_auth::CurrentSession {
            token: token.clone(),
        })
        .await
        .is_ok()
    {
        return Ok(org);
    }
    // Fall back to the locker's credential for this org.
    if let Some(home_slug) = state.home_slug()
        && let Some(home) = state.org(&home_slug)
        && let Ok(bundle) = home
            .auth
            .auth
            .current_session(architect_auth::CurrentSession {
                token: token.clone(),
            })
            .await
        && let Some(store) = home.identity
        && let Ok(links) = store.list_links(bundle.user.id).await
        && let Some(link) = links.into_iter().find(|l| l.remote_slug == slug)
        && let Some(linked) = link.token
        && org
            .auth
            .auth
            .current_session(architect_auth::CurrentSession { token: linked })
            .await
            .is_ok()
    {
        return Ok(org);
    }
    // Or a membership row: the fence the org lane itself uses for a
    // central principal (`CentralFallbackResolver`), so the MCP lane
    // admits exactly whom the RPCs would.
    if member_slugs(state, &token).await.iter().any(|s| s == slug) {
        return Ok(org);
    }
    Err(format!(
        "not authorized for org `{slug}` — sign in there, or link it with `task auth link --org {slug}`"
    ))
}

/// Add the account-lane `org` argument to a per-org tool's schema.
///
/// The catalog is shared with `/org/{slug}/mcp`, where the org is in
/// the URL and the argument would be meaningless. Injecting it here
/// keeps one catalog rather than two that drift.
fn with_org_param(mut schema: Value, default_slug: &str) -> Value {
    if let Some(props) = schema.get_mut("properties").and_then(Value::as_object_mut) {
        props.insert(
            "org".into(),
            json!({
                "type": "string",
                "description": format!(
                    "Org slug to act in. Defaults to `{default_slug}`. Call list_orgs to see \
                     which orgs you can reach — they have SEPARATE vaults, so a task in one is \
                     invisible in another."
                ),
            }),
        );
    }
    schema
}

/// Why a tool call didn't produce a result.
enum ToolFailure {
    /// No such tool — a protocol-level error.
    Unknown,
    /// The tool ran and failed, or its arguments were wrong. The
    /// model sees this text.
    Message(String),
}

impl From<String> for ToolFailure {
    fn from(m: String) -> Self {
        Self::Message(m)
    }
}

// ── Argument helpers ─────────────────────────────────────────────

fn arg_str(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

fn required_str(args: &Value, key: &str) -> Result<String, ToolFailure> {
    arg_str(args, key).ok_or_else(|| ToolFailure::Message(format!("`{key}` is required")))
}

fn arg_bool(args: &Value, key: &str) -> bool {
    args.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// A list tool's row cap: caller's value, clamped, defaulted.
fn arg_limit(args: &Value) -> usize {
    args.get("limit")
        .and_then(Value::as_u64)
        .map_or(DEFAULT_LIMIT, |n| (n as usize).clamp(1, MAX_LIMIT))
}

/// Distinguish "field absent, leave it alone" from "field present and
/// empty, clear it" — the difference between not touching a due date
/// and removing one.
fn optional_field(args: &Value, key: &str) -> Option<Option<String>> {
    match args.get(key) {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if s.trim().is_empty() => Some(None),
        Some(Value::String(s)) => Some(Some(s.trim().to_string())),
        Some(_) => None,
    }
}

/// The date part of an RFC-3339 or ISO timestamp, for windowing.
fn date_of(stamp: &str) -> &str {
    stamp.split('T').next().unwrap_or(stamp)
}

/// Parse a UUID argument with an error naming the field.
fn parse_uuid(value: &str, what: &str) -> Result<uuid::Uuid, ToolFailure> {
    value.parse::<uuid::Uuid>().map_err(|_| {
        ToolFailure::Message(format!("`{value}` is not a {what} id (expected a UUID)"))
    })
}

/// `YYYY-MM-DD` → `NaiveDate`, with an actionable error.
fn parse_date(value: &str, key: &str) -> Result<chrono::NaiveDate, ToolFailure> {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        ToolFailure::Message(format!(
            "`{key}` must be an ISO date (YYYY-MM-DD), got `{value}`"
        ))
    })
}

/// `"agent:NAME"` / `"human:USER_ID"` / bare name (= agent) → the
/// claim identity — the same convention `task issue claim` uses.
fn parse_agent(s: &str) -> Result<task::workflows_proto::AgentRef, ToolFailure> {
    use task::workflows_proto::AgentRef;
    let s = s.trim();
    if let Some(user) = s.strip_prefix("human:") {
        let user = user.trim();
        if user.is_empty() {
            return Err(ToolFailure::Message("`human:` needs a user id".into()));
        }
        return Ok(AgentRef::human(user));
    }
    let name = s.strip_prefix("agent:").unwrap_or(s).trim();
    if name.is_empty() {
        return Err(ToolFailure::Message(
            "agent ref is empty — pass 'agent:NAME' or 'human:USER_ID'".into(),
        ));
    }
    Ok(AgentRef::agent(name))
}

/// `"HH:MM"` (24h) → minutes-since-midnight.
fn parse_hhmm(s: &str, key: &str) -> Result<scheduling_proto::TimeOfDay, ToolFailure> {
    let bad = || ToolFailure::Message(format!("`{key}` must be 'HH:MM' (24h), got `{s}`"));
    let (h, m) = s.split_once(':').ok_or_else(bad)?;
    let h: u8 = h.parse().map_err(|_| bad())?;
    let m: u8 = m.parse().map_err(|_| bad())?;
    if h > 24 || m > 59 {
        return Err(bad());
    }
    Ok(scheduling_proto::TimeOfDay::new(h, m))
}

/// Day-plan block category, spelled the way the schema documents it
/// (lower snake_case); unknown values are an error rather than a
/// silent `Other` so the model learns the vocabulary.
fn parse_block_category(s: &str) -> Result<scheduling_proto::BlockCategory, ToolFailure> {
    use scheduling_proto::BlockCategory as C;
    Ok(match s.to_ascii_lowercase().as_str() {
        "reset" => C::Reset,
        "spiritual" => C::Spiritual,
        "meal" => C::Meal,
        "exercise" => C::Exercise,
        "hygiene" => C::Hygiene,
        "allocatable" => C::Allocatable,
        "maintenance" => C::Maintenance,
        "wind_down" | "winddown" => C::WindDown,
        "sleep" => C::Sleep,
        "other" => C::Other,
        other => {
            return Err(ToolFailure::Message(format!(
                "unknown block category `{other}` — use reset, spiritual, meal, exercise, \
                 hygiene, allocatable, maintenance, wind_down, sleep, or other"
            )));
        }
    })
}

/// A string-array argument. Absent → `None`; present (even empty)
/// → the parsed list, so "replace with empty" is expressible.
fn arg_str_list(args: &Value, key: &str) -> Result<Option<Vec<String>>, ToolFailure> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Array(items)) => {
            let mut out = Vec::with_capacity(items.len());
            for item in items {
                match item.as_str() {
                    Some(s) if !s.trim().is_empty() => out.push(s.trim().to_string()),
                    Some(_) => {}
                    None => {
                        return Err(ToolFailure::Message(format!(
                            "`{key}` must be an array of strings"
                        )));
                    }
                }
            }
            Ok(Some(out))
        }
        Some(_) => Err(ToolFailure::Message(format!(
            "`{key}` must be an array of strings"
        ))),
    }
}

/// Turn a backend error into something the model can act on.
///
/// Domain errors debug-print as bare variants (`NotFound`,
/// `AlreadyExists(..)`) — true but useless to an agent deciding what
/// to do next. Naming the operation and the subject turns a dead end
/// into a next step.
fn backend_err(what: &str, subject: &str, e: &impl std::fmt::Debug) -> ToolFailure {
    let raw = format!("{e:?}");
    let msg = if raw.starts_with("NotFound") {
        format!(
            "no {what} matching `{subject}`. Call the matching list/search tool first and use a value from its result."
        )
    } else if raw.starts_with("AlreadyExists") {
        format!("a {what} already exists at `{subject}`.")
    } else {
        format!("couldn't {what} `{subject}`: {raw}")
    };
    ToolFailure::Message(msg)
}

// ── Tool implementations ─────────────────────────────────────────

/// Dispatch one tool call. Synchronous on purpose — the backends
/// are architect's blocking-dispatcher trait impls; the caller runs
/// this on `spawn_blocking`.
#[allow(clippy::too_many_lines)]
fn call_tool(org: &crate::OrgAppState, name: &str, args: &Value) -> Result<Value, ToolFailure> {
    use contacts_proto::Contacts as _;
    use email_proto::EmailSync as _;
    use goal::GoalService as _;
    use inbox_proto::Inbox as _;
    use milestone::MilestoneService as _;
    use project::ProjectService as _;
    #[cfg(feature = "plugin-scheduling")]
    use scheduling_proto::{Bookings as _, CalendarEvents as _, DayPlans as _, Slots as _};
    use task::TaskService as _;
    use vault_proto::VaultSync as _;

    match name {
        "task_context" => {
            let now = chrono::Local::now();
            let today = now.format("%Y-%m-%d").to_string();
            let open = org
                .tasks
                .query(task::TaskListFilter {
                    open_only: true,
                    ..Default::default()
                })
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let due_today = open
                .iter()
                .filter(|t| {
                    t.due
                        .as_deref()
                        .is_some_and(|d| date_of(d) <= today.as_str())
                        || t.scheduled
                            .as_deref()
                            .is_some_and(|d| date_of(d) <= today.as_str())
                })
                .count();
            // Scheduling is a plugin twice over: compiled out under
            // --no-default-features (cfg) and toggleable per org at
            // runtime (PluginSet) — a core orientation call must not
            // touch a backend that is off either way.
            #[cfg(feature = "plugin-scheduling")]
            let events_today = if org.plugins.contains("scheduling") {
                org.scheduling
                    .list_events()
                    .map(|evs| evs.iter().filter(|e| date_of(&e.start) == today).count())
                    .unwrap_or(0)
            } else {
                0
            };
            #[cfg(not(feature = "plugin-scheduling"))]
            let events_today = 0;
            let inbox_open = org
                .inbox
                .review_queue(today.clone())
                .map(|q| q.len())
                .unwrap_or(0);
            Ok(json!({
                "org": org.slug,
                "now": now.to_rfc3339(),
                "today": today,
                "weekday": now.format("%A").to_string(),
                "open_tasks": open.len(),
                "tasks_due_or_scheduled_today": due_today,
                "events_today": events_today,
                "inbox_awaiting_review": inbox_open,
            }))
        }

        "list_tasks" => {
            let status = arg_str(args, "status");
            let project = match arg_str(args, "project") {
                Some(p) => Some(parse_uuid(&p, "project")?),
                None => None,
            };
            let filter = task::TaskListFilter {
                // An explicit status wins; without one we show what's
                // actionable, which is what "my tasks" means.
                open_only: status.is_none(),
                status,
                project,
                due_on_or_before: arg_str(args, "due_on_or_before"),
                ..Default::default()
            };
            let query = arg_str(args, "query").map(|q| q.to_lowercase());
            let rows = org
                .tasks
                .query(filter)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let out: Vec<Value> = rows
                .iter()
                .filter(|t| {
                    query
                        .as_ref()
                        .is_none_or(|q| t.title.to_lowercase().contains(q))
                })
                .take(arg_limit(args))
                .map(task_json)
                .collect();
            Ok(json!({ "count": out.len(), "tasks": out }))
        }

        "create_task" => {
            let text = required_str(args, "text")?;
            let mut draft = task::capture(&text);
            if let Some(due) = arg_str(args, "due") {
                draft.due = Some(due);
            }
            if let Some(scheduled) = arg_str(args, "scheduled") {
                draft.scheduled = Some(scheduled);
            }
            if let Some(priority) = arg_str(args, "priority") {
                draft.priority = priority;
            }
            let created = org
                .tasks
                .create(draft)
                .map_err(|e| backend_err("task", &text, &e))?;
            Ok(task_json(&created))
        }

        "update_task" => {
            let id = required_str(args, "id")?;
            let uuid = id
                .parse::<uuid::Uuid>()
                .map_err(|_| ToolFailure::Message(format!("`{id}` is not a task id")))?;
            let mut current = org
                .tasks
                .get(uuid)
                .map_err(|e| backend_err("task", &id, &e))?;
            if let Some(status) = arg_str(args, "status") {
                current.status = status;
            }
            if let Some(title) = arg_str(args, "title") {
                current.title = title;
            }
            if let Some(priority) = arg_str(args, "priority") {
                current.priority = priority;
            }
            if let Some(due) = optional_field(args, "due") {
                current.due = due;
            }
            if let Some(scheduled) = optional_field(args, "scheduled") {
                current.scheduled = scheduled;
            }
            if let Some(refs) = arg_str_list(args, "assignees")? {
                let parsed: Result<Vec<_>, ToolFailure> =
                    refs.iter().map(|r| parse_agent(r)).collect();
                current
                    .workflow
                    .get_or_insert_with(Default::default)
                    .assignees = task::model::AgentRefList(parsed?);
            }
            let saved = org
                .tasks
                .update(current)
                .map_err(|e| backend_err("task", &id, &e))?;
            Ok(task_json(&saved))
        }

        "list_untriaged_tasks" => {
            let rows = org
                .tasks
                .query(task::TaskListFilter {
                    open_only: true,
                    ..Default::default()
                })
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let mut unfiled: Vec<&task::TaskInfo> =
                rows.iter().filter(|t| task::is_unfiled(t)).collect();
            // Oldest capture first: the thing that's been sitting
            // longest is the thing most worth naming.
            unfiled.sort_by(|a, b| {
                a.date_created
                    .cmp(&b.date_created)
                    .then_with(|| a.title.to_lowercase().cmp(&b.title.to_lowercase()))
            });
            let total = unfiled.len();
            let out: Vec<Value> = unfiled
                .iter()
                .take(arg_limit(args))
                .map(|t| {
                    let mut v = task_json(t);
                    // The body is where the "what is this for" answer
                    // usually hides — a bare title rarely carries it.
                    if let Some(map) = v.as_object_mut() {
                        map.insert(
                            "details".into(),
                            json!(truncate_body(&t.details, TRIAGE_BODY_CAP)),
                        );
                        map.insert("created".into(), json!(t.date_created));
                    }
                    v
                })
                .collect();

            // The candidate homes, inline. Triage that needs three
            // round-trips before it can think is triage nobody runs.
            let projects: Vec<Value> = org
                .projects
                .list()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?
                .iter()
                .filter(|p| project::Status::from_str(&p.status).is_none_or(|s| !s.is_closed()))
                .map(project_json)
                .collect();
            let parents: Vec<Value> = rows
                .iter()
                .filter(|t| task::is_filed(t))
                .map(|t| json!({ "id": t.id.to_string(), "title": t.title }))
                .collect();
            Ok(json!({
                "count": out.len(),
                "total_untriaged": total,
                "tasks": out,
                "projects": projects,
                "filed_tasks": parents,
                "note": "File each task with file_task. Leave anything you can't place \
                         confidently — an unfiled task is recoverable, a wrongly filed one \
                         is invisible.",
            }))
        }

        "file_task" => {
            let id = required_str(args, "id")?;
            let uuid = parse_uuid(&id, "task")?;
            let mut current = org
                .tasks
                .get(uuid)
                .map_err(|e| backend_err("task", &id, &e))?;

            let mut filed_as: Vec<String> = Vec::new();
            if let Some(p) = arg_str(args, "project") {
                let pid = parse_uuid(&p, "project")?;
                // Resolve the title so the markdown page keeps its
                // human-readable `projects:` wikilink — the vault is
                // the source of truth, not just the DB column.
                let title = org
                    .projects
                    .list()
                    .ok()
                    .and_then(|ps| ps.iter().find(|x| x.id == pid).map(|x| x.title.clone()));
                let Some(title) = title else {
                    return Err(ToolFailure::Message(format!(
                        "no project with id `{p}` — call list_projects first"
                    )));
                };
                current.project_id = Some(pid);
                current.projects = vec![format!("[[{title}]]")].into();
                filed_as.push(format!("project {title}"));
            }
            if let Some(p) = arg_str(args, "parent") {
                let parent = parse_uuid(&p, "task")?;
                if parent == uuid {
                    return Err(ToolFailure::Message(
                        "a task cannot be its own parent".into(),
                    ));
                }
                // A parent that doesn't exist would orphan the task
                // in a way no view can show.
                org.tasks
                    .get(parent)
                    .map_err(|_| ToolFailure::Message(format!("no task with id `{p}`")))?;
                current.workflow.get_or_insert_with(Default::default).parent = Some(parent);
                filed_as.push("parent task".to_string());
            }
            if let Some(w) = arg_str(args, "workstream") {
                current
                    .workflow
                    .get_or_insert_with(Default::default)
                    .workstream = Some(parse_uuid(&w, "workstream")?);
                filed_as.push("workstream".to_string());
            }
            if let Some(cs) = arg_str_list(args, "contexts")? {
                current.contexts = cs
                    .iter()
                    .map(|c| {
                        let c = c.trim().trim_start_matches('@');
                        format!("@{c}")
                    })
                    .collect::<Vec<_>>()
                    .into();
                filed_as.push("contexts".to_string());
            }
            if filed_as.is_empty() {
                return Err(ToolFailure::Message(
                    "nothing to file by — pass at least one of project, parent, workstream, \
                     or contexts"
                        .into(),
                ));
            }
            let saved = org
                .tasks
                .update(current)
                .map_err(|e| backend_err("task", &id, &e))?;
            let mut out = task_json(&saved);
            if let Some(map) = out.as_object_mut() {
                map.insert("filed_as".into(), json!(filed_as.join(", ")));
                if let Some(reason) = arg_str(args, "reason") {
                    map.insert("reason".into(), json!(reason));
                }
            }
            Ok(out)
        }

        "claim_task" => {
            let id = required_str(args, "id")?;
            let uuid = parse_uuid(&id, "task")?;
            let agent = parse_agent(&required_str(args, "agent")?)?;
            let agent_json = serde_json::to_string(&agent)
                .map_err(|e| ToolFailure::Message(format!("encode agent: {e}")))?;
            let outcome = org
                .tasks
                .try_claim(uuid, agent_json, arg_bool(args, "force"))
                .map_err(|e| backend_err("task", &id, &e))?;
            Ok(match outcome {
                task::service::ClaimResult::Won => json!({
                    "id": id, "claim": "won",
                    "note": "the task is yours — set it in-progress and start",
                }),
                task::service::ClaimResult::AlreadyMine => json!({
                    "id": id, "claim": "already_mine",
                }),
                task::service::ClaimResult::Lost { holder } => json!({
                    "id": id, "claim": "lost", "holder": holder,
                    "note": "someone else holds this task — pick different work \
                             or ask the user before forcing",
                }),
            })
        }

        "list_events" => {
            let from = arg_str(args, "from");
            let to = arg_str(args, "to");
            let mut rows = org
                .scheduling
                .list_events()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            rows.retain(|e| {
                let day = date_of(&e.start);
                from.as_deref().is_none_or(|f| day >= f) && to.as_deref().is_none_or(|t| day <= t)
            });
            rows.sort_by(|a, b| a.start.cmp(&b.start));
            let out: Vec<Value> = rows.iter().take(arg_limit(args)).map(event_json).collect();
            Ok(json!({ "count": out.len(), "events": out }))
        }

        #[cfg(feature = "plugin-scheduling")]
        "create_event" => {
            let event = scheduling_proto::CalEvent {
                id: uuid::Uuid::new_v4().to_string(),
                title: required_str(args, "title")?,
                start: required_str(args, "start")?,
                end: required_str(args, "end")?,
                all_day: arg_bool(args, "all_day"),
                color: "primary".to_string(),
                description: arg_str(args, "description"),
                recurrence: None,
            };
            org.scheduling
                .upsert_event(&event)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            Ok(event_json(&event))
        }

        #[cfg(feature = "plugin-scheduling")]
        "reschedule_event" => {
            let id = required_str(args, "id")?;
            let start = required_str(args, "start")?;
            let end = required_str(args, "end")?;
            let mut event = org
                .scheduling
                .list_events()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?
                .into_iter()
                .find(|e| e.id == id)
                .ok_or_else(|| ToolFailure::Message(format!("no event with id `{id}`")))?;
            let was = format!("{} → {}", event.start, event.end);
            event.start = start;
            event.end = end;
            org.scheduling
                .upsert_event(&event)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let mut out = event_json(&event);
            out["moved_from"] = json!(was);
            Ok(out)
        }

        #[cfg(feature = "plugin-scheduling")]
        "cancel_event" => {
            let id = required_str(args, "id")?;
            org.scheduling
                .delete_event(&id)
                .map_err(|e| backend_err("event", &id, &e))?;
            Ok(json!({ "cancelled": id }))
        }

        "capture_inbox" => {
            let body = required_str(args, "body")?;
            let now = chrono::Utc::now().to_rfc3339();
            let mut item = inbox_proto::InboxItem::capture(
                uuid::Uuid::new_v4().to_string(),
                body,
                "agent",
                now,
            );
            if !arg_bool(args, "accepted") {
                item.status = inbox_proto::InboxItem::STATUS_SUGGESTED.to_string();
            }
            item.resurface_on = arg_str(args, "resurface_on");
            org.inbox
                .upsert_inbox_item(&item)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            Ok(json!({
                "id": item.id,
                "status": item.status,
                "resurface_on": item.resurface_on,
                "note": if item.status == inbox_proto::InboxItem::STATUS_SUGGESTED {
                    "captured as a suggestion — the user accepts or dismisses it in their inbox"
                } else {
                    "filed as an open inbox item"
                },
            }))
        }

        "list_inbox" => {
            // The review queue is "what surfaces today", which by
            // design excludes suggestions — so an agent couldn't see
            // its own captures. `status` opens the full list.
            let rows = match arg_str(args, "status") {
                Some(status) => {
                    let all = org
                        .inbox
                        .list_inbox()
                        .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
                    all.into_iter()
                        .filter(|i| i.status.eq_ignore_ascii_case(&status))
                        .collect()
                }
                None => {
                    let today = chrono::Local::now().format("%Y-%m-%d").to_string();
                    org.inbox
                        .review_queue(today)
                        .map_err(|e| ToolFailure::Message(format!("{e:?}")))?
                }
            };
            let out: Vec<Value> = rows
                .iter()
                .take(arg_limit(args))
                .map(|i| {
                    json!({
                        "id": i.id,
                        "body": i.body,
                        "kind": i.kind,
                        "status": i.status,
                        "created": i.created,
                    })
                })
                .collect();
            Ok(json!({ "count": out.len(), "items": out }))
        }

        "list_projects" => {
            let rows = org
                .projects
                .list()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let out: Vec<Value> = rows
                .iter()
                .take(arg_limit(args))
                .map(|p| {
                    json!({
                        "id": p.id.to_string(),
                        "title": p.title,
                        "status": p.status,
                        "path": p.path,
                    })
                })
                .collect();
            Ok(json!({ "count": out.len(), "projects": out }))
        }

        "list_goals" => {
            let rows = org
                .goals
                .list()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let out: Vec<Value> = rows
                .iter()
                .take(arg_limit(args))
                .map(|g| {
                    json!({
                        "id": g.id.to_string(),
                        "title": g.title,
                        "status": g.status,
                        "path": g.path,
                    })
                })
                .collect();
            Ok(json!({ "count": out.len(), "goals": out }))
        }

        "search_vault" => {
            let query = required_str(args, "query")?.to_lowercase();
            let manifest = org
                .vault_sync
                .manifest(VAULT_ID)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let mut hits: Vec<&str> = manifest
                .files
                .iter()
                .map(|f| f.path.as_str())
                .filter(|p| p.to_lowercase().contains(&query))
                .collect();
            hits.sort_unstable();
            let limit = arg_limit(args);
            let total = hits.len();
            hits.truncate(limit);
            Ok(json!({
                "count": hits.len(),
                "truncated": total > hits.len(),
                "paths": hits,
            }))
        }

        "read_note" => {
            let path = required_str(args, "path")?;
            let bytes = org
                .vault_sync
                .get_file(VAULT_ID, &path)
                .map_err(|e| backend_err("note", &path, &e))?;
            let text = String::from_utf8_lossy(&bytes.0).to_string();
            Ok(json!({ "path": path, "content": text }))
        }

        "append_note" => {
            let path = required_str(args, "path")?;
            let text = required_str(args, "text")?;
            let existing = org
                .vault_sync
                .get_file(VAULT_ID, &path)
                .map_err(|e| backend_err("note", &path, &e))?;
            let mut body = String::from_utf8_lossy(&existing.0).to_string();
            if !body.ends_with('\n') {
                body.push('\n');
            }
            body.push('\n');
            body.push_str(&text);
            body.push('\n');
            org.vault_sync
                .put_file(
                    VAULT_ID,
                    &path,
                    body.into_bytes(),
                    vault_proto::IfMatch::Force,
                )
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            Ok(json!({ "path": path, "appended": true }))
        }

        "write_note" => {
            let path = required_str(args, "path")?;
            let content = required_str(args, "content")?;
            let existed = org.vault_sync.get_file(VAULT_ID, &path).is_ok();
            org.vault_sync
                .put_file(
                    VAULT_ID,
                    &path,
                    content.into_bytes(),
                    vault_proto::IfMatch::Force,
                )
                .map_err(|e| backend_err("note", &path, &e))?;
            Ok(json!({
                "path": path,
                "written": true,
                "replaced_existing": existed,
            }))
        }

        // ── Projects / goals / milestones ────────────────────────
        "create_project" => {
            let draft = project::ProjectInfo {
                title: required_str(args, "title")?,
                status: arg_str(args, "status").unwrap_or_else(|| "active".into()),
                priority: arg_str(args, "priority").unwrap_or_else(|| "normal".into()),
                project_type: "general".into(),
                parent_id: match arg_str(args, "parent_id") {
                    Some(p) => Some(parse_uuid(&p, "project")?),
                    None => None,
                },
                details: arg_str(args, "details").unwrap_or_default(),
                verify_command: arg_str(args, "verify_command").unwrap_or_default(),
                ..Default::default()
            };
            let title = draft.title.clone();
            let created = org
                .projects
                .create(draft)
                .map_err(|e| backend_err("project", &title, &e))?;
            Ok(project_json(&created))
        }

        "update_project" => {
            let id = required_str(args, "id")?;
            let uuid = parse_uuid(&id, "project")?;
            let mut current = org
                .projects
                .get(uuid)
                .map_err(|e| backend_err("project", &id, &e))?;
            if let Some(title) = arg_str(args, "title") {
                current.title = title;
            }
            if let Some(status) = arg_str(args, "status") {
                current.status = status;
            }
            if let Some(priority) = arg_str(args, "priority") {
                current.priority = priority;
            }
            if let Some(details) = arg_str(args, "details") {
                current.details = details;
            }
            let saved = org
                .projects
                .update(current)
                .map_err(|e| backend_err("project", &id, &e))?;
            Ok(project_json(&saved))
        }

        "create_goal" => {
            let draft = goal::Goal {
                id: uuid::Uuid::nil(),
                path: String::new(),
                title: required_str(args, "title")?,
                kind: arg_str(args, "kind").unwrap_or_else(|| "lifetime".into()),
                status: arg_str(args, "status").unwrap_or_else(|| "aspiration".into()),
                parent_id: None,
                target_date: match arg_str(args, "target_date") {
                    Some(d) => Some(parse_date(&d, "target_date")?),
                    None => None,
                },
                cycle_id: None,
                tags: goal::Tags::default(),
                date_created: None,
                date_modified: None,
                details: arg_str(args, "details").unwrap_or_default(),
            };
            let title = draft.title.clone();
            let created = org
                .goals
                .create(draft)
                .map_err(|e| backend_err("goal", &title, &e))?;
            Ok(goal_json(&created))
        }

        "update_goal" => {
            let id = required_str(args, "id")?;
            let uuid = parse_uuid(&id, "goal")?;
            let mut current = org
                .goals
                .get(uuid)
                .map_err(|e| backend_err("goal", &id, &e))?;
            if let Some(title) = arg_str(args, "title") {
                current.title = title;
            }
            if let Some(status) = arg_str(args, "status") {
                current.status = status;
            }
            if let Some(kind) = arg_str(args, "kind") {
                current.kind = kind;
            }
            if let Some(target) = optional_field(args, "target_date") {
                current.target_date = match target {
                    Some(d) => Some(parse_date(&d, "target_date")?),
                    None => None,
                };
            }
            let saved = org
                .goals
                .update(current)
                .map_err(|e| backend_err("goal", &id, &e))?;
            Ok(goal_json(&saved))
        }

        "list_milestones" => {
            let project = match arg_str(args, "project") {
                Some(p) => Some(parse_uuid(&p, "project")?),
                None => None,
            };
            let rows = org
                .milestones
                .list()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let out: Vec<Value> = rows
                .iter()
                .filter(|m| project.is_none_or(|p| m.project_id == p))
                .take(arg_limit(args))
                .map(milestone_json)
                .collect();
            Ok(json!({ "count": out.len(), "milestones": out }))
        }

        "create_milestone" => {
            let draft = milestone::Milestone {
                id: uuid::Uuid::nil(),
                path: String::new(),
                title: required_str(args, "title")?,
                project_id: parse_uuid(&required_str(args, "project_id")?, "project")?,
                goal_id: match arg_str(args, "goal_id") {
                    Some(g) => Some(parse_uuid(&g, "goal")?),
                    None => None,
                },
                status: "open".into(),
                due_date: match arg_str(args, "due_date") {
                    Some(d) => Some(parse_date(&d, "due_date")?),
                    None => None,
                },
                tags: milestone::Tags::default(),
                forge_ref: None,
                date_created: None,
                date_modified: None,
                details: arg_str(args, "details").unwrap_or_default(),
            };
            let title = draft.title.clone();
            let created = org
                .milestones
                .create(draft)
                .map_err(|e| backend_err("milestone", &title, &e))?;
            Ok(milestone_json(&created))
        }

        "update_milestone" => {
            let id = required_str(args, "id")?;
            let uuid = parse_uuid(&id, "milestone")?;
            let mut current = org
                .milestones
                .get(uuid)
                .map_err(|e| backend_err("milestone", &id, &e))?;
            if let Some(title) = arg_str(args, "title") {
                current.title = title;
            }
            if let Some(status) = arg_str(args, "status") {
                current.status = status;
            }
            if let Some(due) = optional_field(args, "due_date") {
                current.due_date = match due {
                    Some(d) => Some(parse_date(&d, "due_date")?),
                    None => None,
                };
            }
            let saved = org
                .milestones
                .update(current)
                .map_err(|e| backend_err("milestone", &id, &e))?;
            Ok(milestone_json(&saved))
        }

        // ── Inbox processing ─────────────────────────────────────
        "process_inbox_item" => {
            let id = required_str(args, "id")?;
            let status = required_str(args, "status")?.to_lowercase();
            let allowed = [
                inbox_proto::InboxItem::STATUS_OPEN,
                inbox_proto::InboxItem::STATUS_PROCESSED,
                inbox_proto::InboxItem::STATUS_ARCHIVED,
            ];
            if !allowed.contains(&status.as_str()) {
                return Err(ToolFailure::Message(format!(
                    "`status` must be one of open, processed, archived — got `{status}`"
                )));
            }
            let mut item = org
                .inbox
                .get_inbox_item(&id)
                .map_err(|e| backend_err("inbox item", &id, &e))?;
            let was = item.status.clone();
            item.status = status;
            org.inbox
                .upsert_inbox_item(&item)
                .map_err(|e| backend_err("inbox item", &id, &e))?;
            Ok(json!({ "id": item.id, "status": item.status, "was": was }))
        }

        // ── Day plans / bookings ─────────────────────────────────
        "get_day_plan" => {
            let date = required_str(args, "date")?;
            parse_date(&date, "date")?;
            let plan = org
                .scheduling
                .get_day_plan(&date)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            Ok(json!({ "date": date, "plan": plan.map(|p| day_plan_json(&p)) }))
        }

        "upsert_day_plan" => {
            let date = required_str(args, "date")?;
            parse_date(&date, "date")?;
            let Some(blocks) = args.get("blocks").and_then(Value::as_array) else {
                return Err(ToolFailure::Message("`blocks` (array) is required".into()));
            };
            let mut planned = Vec::with_capacity(blocks.len());
            for b in blocks {
                let start = b.get("start").and_then(Value::as_str).ok_or_else(|| {
                    ToolFailure::Message("every block needs a `start` ('HH:MM')".into())
                })?;
                let end = b.get("end").and_then(Value::as_str).ok_or_else(|| {
                    ToolFailure::Message("every block needs an `end` ('HH:MM')".into())
                })?;
                let label = b
                    .get("label")
                    .and_then(Value::as_str)
                    .ok_or_else(|| ToolFailure::Message("every block needs a `label`".into()))?;
                planned.push(scheduling_proto::PlannedBlock {
                    id: scheduling_proto::TimeBlockId(uuid::Uuid::new_v4().to_string()),
                    start: parse_hhmm(start, "start")?,
                    end: parse_hhmm(end, "end")?,
                    label: label.to_string(),
                    category: match b.get("category").and_then(Value::as_str) {
                        Some(c) => parse_block_category(c)?,
                        None => scheduling_proto::BlockCategory::Other,
                    },
                    note: b.get("note").and_then(Value::as_str).map(str::to_owned),
                    assignment: None,
                    fixed: b.get("fixed").and_then(Value::as_bool).unwrap_or(false),
                });
            }
            let count = planned.len();
            let plan = scheduling_proto::DayPlan {
                date: date.clone(),
                from_template: None,
                blocks: planned.into(),
            };
            org.scheduling
                .upsert_day_plan(&plan)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            Ok(json!({ "date": date, "blocks": count, "written": true }))
        }

        "list_bookings" => {
            let mut rows = org
                .scheduling
                .list_bookings()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            rows.sort_by(|a, b| a.start_utc.cmp(&b.start_utc));
            let out: Vec<Value> = rows
                .iter()
                .take(arg_limit(args))
                .map(booking_json)
                .collect();
            Ok(json!({ "count": out.len(), "bookings": out }))
        }

        "list_open_slots" => {
            let query = scheduling_proto::SlotQuery {
                event_type_id: scheduling_proto::EventTypeId(required_str(args, "event_type_id")?),
                from_utc: required_str(args, "from")?,
                to_utc: required_str(args, "to")?,
            };
            let slots = org
                .scheduling
                .list_open_slots(&query)
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let out: Vec<Value> = slots
                .iter()
                .take(arg_limit(args))
                .map(|s| json!({ "start": s.start_utc, "end": s.end_utc }))
                .collect();
            Ok(json!({ "count": out.len(), "slots": out }))
        }

        "book_slot" => {
            let new = scheduling_proto::NewBooking {
                event_type_id: scheduling_proto::EventTypeId(required_str(args, "event_type_id")?),
                start_utc: required_str(args, "start")?,
                end_utc: required_str(args, "end")?,
                attendee_name: required_str(args, "attendee_name")?,
                attendee_email: required_str(args, "attendee_email")?,
                note: arg_str(args, "note"),
            };
            let start = new.start_utc.clone();
            let booked = org
                .scheduling
                .create_booking(&new)
                .map_err(|e| backend_err("booking", &start, &e))?;
            Ok(booking_json(&booked))
        }

        "cancel_booking" => {
            let id = required_str(args, "id")?;
            org.scheduling
                .update_booking_status(
                    &scheduling_proto::BookingId(id.clone()),
                    scheduling_proto::BookingStatus::Cancelled,
                )
                .map_err(|e| backend_err("booking", &id, &e))?;
            Ok(json!({ "cancelled": id }))
        }

        // ── Contacts ─────────────────────────────────────────────
        "list_contacts" => {
            let query = arg_str(args, "query").map(|q| q.to_lowercase());
            let rows = org
                .contacts
                .list_contacts()
                .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
            let out: Vec<Value> = rows
                .iter()
                .filter(|c| {
                    query.as_ref().is_none_or(|q| {
                        c.full_name.to_lowercase().contains(q)
                            || c.emails.to_lowercase().contains(q)
                            || c.organization
                                .as_deref()
                                .is_some_and(|o| o.to_lowercase().contains(q))
                    })
                })
                .take(arg_limit(args))
                .map(contact_json)
                .collect();
            Ok(json!({ "count": out.len(), "contacts": out }))
        }

        "upsert_contact" => {
            let mut contact = match arg_str(args, "id") {
                Some(id) => org
                    .contacts
                    .get_contact(id.clone())
                    .map_err(|e| backend_err("contact", &id, &e))?
                    .ok_or_else(|| {
                        ToolFailure::Message(format!(
                            "no contact with id `{id}` — list_contacts first, or omit `id` to create"
                        ))
                    })?,
                None => {
                    let name = arg_str(args, "name").ok_or_else(|| {
                        ToolFailure::Message("`name` is required when creating a contact".into())
                    })?;
                    contacts_proto::Contact::create(
                        uuid::Uuid::new_v4().to_string(),
                        name,
                        chrono::Utc::now().to_rfc3339(),
                    )
                }
            };
            if let Some(name) = arg_str(args, "name") {
                contact.full_name = name;
            }
            if let Some(emails) = arg_str_list(args, "emails")? {
                contact.emails = emails.join("\n");
            }
            if let Some(phones) = arg_str_list(args, "phones")? {
                contact.phones = phones.join("\n");
            }
            if let Some(org_name) = arg_str(args, "organization") {
                contact.organization = Some(org_name);
            }
            if let Some(notes) = arg_str(args, "notes") {
                contact.notes = Some(notes);
            }
            contact.updated = Some(chrono::Utc::now().to_rfc3339());
            org.contacts
                .upsert_contact(&contact)
                .map_err(|e| backend_err("contact", &contact.id, &e))?;
            Ok(contact_json(&contact))
        }

        // ── Email (read + draft; no send by design) ──────────────
        "list_email_accounts" => {
            let accounts = org.email.accounts().map_err(email_err)?;
            let out: Vec<Value> = accounts
                .iter()
                .map(|a| {
                    let folders = org
                        .email
                        .list_folders(&a.id.0)
                        .map(|fs| {
                            fs.iter()
                                .map(|f| {
                                    json!({
                                        "name": f.name,
                                        "unread": f.unread_count,
                                        "messages": f.message_count,
                                    })
                                })
                                .collect::<Vec<_>>()
                        })
                        .unwrap_or_default();
                    json!({
                        "id": a.id.0,
                        "name": a.name,
                        "address": a.address,
                        "folders": folders,
                    })
                })
                .collect();
            Ok(json!({ "count": out.len(), "accounts": out }))
        }

        "list_envelopes" => {
            let account = required_str(args, "account")?;
            let folder = arg_str(args, "folder").unwrap_or_else(|| "INBOX".into());
            let limit = arg_limit(args);
            let mut rows = org
                .email
                .fetch_envelopes(
                    &account,
                    &folder,
                    email_proto::SeqRange::Recent(limit as u32),
                )
                .map_err(email_err)?;
            // Newest first for the model, whatever the backend's order.
            rows.sort_by(|a, b| b.date_ms.cmp(&a.date_ms));
            let out: Vec<Value> = rows.iter().take(limit).map(envelope_json).collect();
            Ok(json!({ "account": account, "folder": folder, "count": out.len(), "messages": out }))
        }

        "read_email" => {
            let account = required_str(args, "account")?;
            let message_id = required_str(args, "message_id")?;
            let msg = org
                .email
                .fetch_message(&account, &message_id)
                .map_err(email_err)?;
            let body = msg
                .body_text
                .clone()
                .or_else(|| msg.body_html.clone())
                .unwrap_or_default();
            let mut out = envelope_json(&msg.envelope);
            out["body"] = json!(body);
            out["attachments"] = json!(
                msg.attachments
                    .iter()
                    .map(|a| json!({ "name": a.filename, "part": a.part }))
                    .collect::<Vec<_>>()
            );
            Ok(out)
        }

        "draft_email" => {
            let account = required_str(args, "account")?;
            let to = arg_str_list(args, "to")?.unwrap_or_default();
            if to.is_empty() {
                return Err(ToolFailure::Message(
                    "`to` needs at least one address".into(),
                ));
            }
            let draft = email_proto::Draft {
                from: account_addr(org, &account)?,
                to: to.into_iter().map(bare_addr).collect(),
                cc: arg_str_list(args, "cc")?
                    .unwrap_or_default()
                    .into_iter()
                    .map(bare_addr)
                    .collect(),
                bcc: Vec::new(),
                subject: required_str(args, "subject")?,
                body_text: required_str(args, "body")?,
                body_html: None,
                in_reply_to: None,
                references: Vec::new(),
                attachments: Vec::new(),
            };
            let id = org.email.append_draft(&account, draft).map_err(email_err)?;
            Ok(json!({
                "draft_id": id,
                "note": "saved to Drafts — the user reviews and sends it from their mail client",
            }))
        }

        "draft_reply" => {
            let account = required_str(args, "account")?;
            let message_id = required_str(args, "message_id")?;
            let original = org
                .email
                .fetch_message(&account, &message_id)
                .map_err(email_err)?;
            let env = &original.envelope;
            let subject = if env.subject.to_lowercase().starts_with("re:") {
                env.subject.clone()
            } else {
                format!("Re: {}", env.subject)
            };
            let mut references = original.references.clone();
            references.push(env.message_id.clone());
            let draft = email_proto::Draft {
                from: account_addr(org, &account)?,
                to: env.from.clone(),
                cc: if arg_bool(args, "reply_all") {
                    env.to.iter().chain(env.cc.iter()).cloned().collect()
                } else {
                    Vec::new()
                },
                bcc: Vec::new(),
                subject,
                body_text: required_str(args, "body")?,
                body_html: None,
                in_reply_to: Some(env.message_id.clone()),
                references,
                attachments: Vec::new(),
            };
            let id = org.email.append_draft(&account, draft).map_err(email_err)?;
            Ok(json!({
                "draft_id": id,
                "in_reply_to": message_id,
                "note": "saved to Drafts — the user reviews and sends it from their mail client",
            }))
        }

        // ── Discovery ────────────────────────────────────────────
        //
        // Why discovery but no generic `invoke_service`: vox's wire
        // is typed end-to-end — per-connection schema exchange builds
        // phon compat decode programs per (method, direction, reader
        // type), and the only dispatch entry (`Handler::handle`)
        // takes a `RequestCall` + `DriverReplySink` owned by a live
        // connection driver. There is no `call(MethodId, json)`
        // surface to build on without hand-writing a closure per
        // method (i.e. re-curating the catalog). If vox grows a
        // dynamic client, wire it here behind the same plugin gate.
        "api_reference" => {
            let services = crate::api_ref::reference_for(&org.plugins);
            match arg_str(args, "service").map(|s| s.to_lowercase()) {
                // The whole surface, compact: one line per method.
                None => {
                    let out: Vec<Value> = services
                        .iter()
                        .map(|s| {
                            json!({
                                "service": s.name,
                                "alias": s.alias,
                                "plugin": s.plugin,
                                "mounted": s.mounted,
                                "methods": s
                                    .methods
                                    .iter()
                                    .map(|m| format!("{}({})", m.name, m.args.join(", ")))
                                    .collect::<Vec<_>>(),
                            })
                        })
                        .collect();
                    Ok(json!({
                        "service_count": out.len(),
                        "note": "The org's full vox RPC surface (the MCP tools are a curated \
                                 subset of it). `mounted: false` = that plugin is disabled \
                                 here. Pass `service` for one service's permits and docs.",
                        "services": out,
                    }))
                }
                // One service (or a few matches), full detail.
                Some(f) => {
                    let hits: Vec<Value> = services
                        .iter()
                        .filter(|s| {
                            s.name.to_lowercase().contains(&f)
                                || s.alias.is_some_and(|a| a.to_lowercase().contains(&f))
                        })
                        .map(|s| {
                            json!({
                                "service": s.name,
                                "alias": s.alias,
                                "plugin": s.plugin,
                                "mounted": s.mounted,
                                "doc": s.doc,
                                "methods": s.methods.iter().map(|m| json!({
                                    "name": m.name,
                                    "args": m.args,
                                    "stream": m.stream,
                                    "permit": m.action.zip(m.resource)
                                        .map(|(a, r)| format!("{a} {r}")),
                                    "audited": m.audited,
                                    "doc": m.doc,
                                })).collect::<Vec<_>>(),
                            })
                        })
                        .collect();
                    if hits.is_empty() {
                        return Err(ToolFailure::Message(format!(
                            "no service matches `{f}` — call api_reference with no arguments \
                             for the full list"
                        )));
                    }
                    Ok(json!({ "services": hits }))
                }
            }
        }

        _ => Err(ToolFailure::Unknown),
    }
}

/// The sender identity for a draft: the account's own address.
fn account_addr(org: &crate::OrgAppState, account: &str) -> Result<email_proto::Addr, ToolFailure> {
    use email_proto::EmailSync as _;
    let accounts = org.email.accounts().map_err(email_err)?;
    let acct = accounts.iter().find(|a| a.id.0 == account).ok_or_else(|| {
        ToolFailure::Message(format!(
            "no mail account `{account}` — call list_email_accounts first"
        ))
    })?;
    Ok(email_proto::Addr {
        name: acct.display_name.clone(),
        email: acct.address.clone(),
    })
}

fn bare_addr(email: String) -> email_proto::Addr {
    email_proto::Addr { name: None, email }
}

/// Email backend errors, with the one common capability gap named
/// instead of debug-dumped.
fn email_err(e: email_proto::EmailSyncError) -> ToolFailure {
    let raw = format!("{e:?}");
    if raw.starts_with("Unsupported") {
        ToolFailure::Message(format!(
            "this org's mail backend doesn't support that operation yet: {raw}"
        ))
    } else {
        ToolFailure::Message(raw)
    }
}

/// A task, trimmed to the fields an agent reasons over. The full row
/// carries time entries, recurrence anchors, and relation graphs that
/// would only burn context.
fn task_json(t: &task::TaskInfo) -> Value {
    let workflow = t.workflow.as_ref();
    json!({
        "id": t.id.to_string(),
        "title": t.title,
        "status": t.status,
        "priority": t.priority,
        "due": t.due,
        "scheduled": t.scheduled,
        "tags": t.tags.0,
        "contexts": t.contexts.0,
        "projects": t.projects.0,
        // Filing, spelled out: without these an agent can't tell a
        // task that belongs somewhere from one that doesn't, and
        // "which of my tasks need sorting" is unanswerable.
        "project_id": t.project_id.map(|id| id.to_string()),
        "parent": workflow.and_then(|w| w.parent).map(|id| id.to_string()),
        "workstream": workflow.and_then(|w| w.workstream).map(|id| id.to_string()),
        "milestone_id": t.milestone_id.map(|id| id.to_string()),
        "filed": task::is_filed(t),
        "assignees": workflow.map(|w| {
            w.assignees.0.iter().map(|a| a.short_label()).collect::<Vec<_>>()
        }),
        "path": t.path,
    })
}

#[cfg(feature = "plugin-scheduling")]
fn event_json(e: &scheduling_proto::CalEvent) -> Value {
    json!({
        "id": e.id,
        "title": e.title,
        "start": e.start,
        "end": e.end,
        "all_day": e.all_day,
        "description": e.description,
    })
}

fn project_json(p: &project::ProjectInfo) -> Value {
    json!({
        "id": p.id.to_string(),
        "title": p.title,
        "status": p.status,
        "priority": p.priority,
        "path": p.path,
    })
}

fn goal_json(g: &goal::Goal) -> Value {
    json!({
        "id": g.id.to_string(),
        "title": g.title,
        "kind": g.kind,
        "status": g.status,
        "target_date": g.target_date.map(|d| d.to_string()),
        "path": g.path,
    })
}

fn milestone_json(m: &milestone::Milestone) -> Value {
    json!({
        "id": m.id.to_string(),
        "title": m.title,
        "project_id": m.project_id.to_string(),
        "goal_id": m.goal_id.map(|g| g.to_string()),
        "status": m.status,
        "due_date": m.due_date.map(|d| d.to_string()),
        "path": m.path,
    })
}

fn day_plan_json(p: &scheduling_proto::DayPlan) -> Value {
    let hhmm = |t: scheduling_proto::TimeOfDay| format!("{:02}:{:02}", t.hours(), t.minutes());
    json!({
        "date": p.date,
        "blocks": p.blocks.iter().map(|b| json!({
            "start": hhmm(b.start),
            "end": hhmm(b.end),
            "label": b.label,
            "category": format!("{:?}", b.category).to_lowercase(),
            "note": b.note,
            "fixed": b.fixed,
        })).collect::<Vec<_>>(),
    })
}

fn booking_json(b: &scheduling_proto::Booking) -> Value {
    json!({
        "id": b.id.0,
        "event_type": b.event_type_id.0,
        "start": b.start_utc,
        "end": b.end_utc,
        "attendee": b.attendee_name,
        "email": b.attendee_email,
        "status": format!("{:?}", b.status).to_lowercase(),
        "note": b.note,
    })
}

fn contact_json(c: &contacts_proto::Contact) -> Value {
    json!({
        "id": c.id,
        "name": c.full_name,
        "emails": c.email_list(),
        "phones": c.phones.lines().collect::<Vec<_>>(),
        "organization": c.organization,
        "notes": c.notes,
    })
}

fn envelope_json(e: &email_proto::Envelope) -> Value {
    let addr = |a: &email_proto::Addr| match &a.name {
        Some(n) => format!("{n} <{}>", a.email),
        None => a.email.clone(),
    };
    json!({
        "message_id": e.message_id,
        "folder": e.folder,
        "subject": e.subject,
        "from": e.from.iter().map(addr).collect::<Vec<_>>(),
        "to": e.to.iter().map(addr).collect::<Vec<_>>(),
        "date": chrono::DateTime::from_timestamp_millis(e.date_ms)
            .map(|d| d.to_rfc3339())
            .unwrap_or_default(),
        "unread": !e.flags.iter().any(|f| f.eq_ignore_ascii_case("\\Seen") || f.eq_ignore_ascii_case("Seen")),
        "has_attachments": e.has_attachments,
        "snippet": e.snippet,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_entries_are_well_formed() {
        let catalog = tool_catalog();
        assert!(catalog.len() >= 15, "catalog shrank unexpectedly");
        let mut seen = std::collections::HashSet::new();
        for tool in &catalog {
            assert!(seen.insert(tool.name), "duplicate tool `{}`", tool.name);
            // A typo'd plugin id would silently hide the tool for
            // every org (unknown ids are never in a resolved set).
            assert!(
                task_plugin::find(tool.plugin).is_some(),
                "`{}` names unknown plugin `{}`",
                tool.name,
                tool.plugin
            );
            assert!(
                tool.description.len() > 40,
                "`{}` needs a description the model can select on",
                tool.name
            );
            let schema = (tool.schema)();
            assert_eq!(schema["type"], "object", "`{}` schema", tool.name);
            assert!(schema["properties"].is_object(), "`{}` schema", tool.name);
            // Every declared `required` name must exist in properties,
            // or clients reject the tool at registration.
            for req in schema["required"].as_array().expect("required array") {
                let key = req.as_str().expect("required entry is a string");
                assert!(
                    schema["properties"].get(key).is_some(),
                    "`{}` requires `{key}` but doesn't declare it",
                    tool.name
                );
            }
        }
    }

    #[test]
    fn tools_list_payload_matches_the_catalog() {
        let all = task_plugin::PluginSet::resolve(None);
        let payload = tools_list_payload(&all);
        let tools = payload["tools"].as_array().expect("tools array");
        assert_eq!(tools.len(), tool_catalog().len());
        assert!(tools.iter().all(|t| t["inputSchema"].is_object()));
        assert!(tools.iter().any(|t| t["name"] == "create_task"));
    }

    /// The plugin toggle on both MCP surfaces: a disabled plugin's
    /// tools vanish from `tools/list`, and `tools/call` refuses them
    /// with a message naming the plugin — while unknown tools stay a
    /// protocol-level method-not-found.
    ///
    /// Exercised through `email`, which must stay a **non-core** plugin
    /// that owns tools for this to test anything: `PluginSet::resolve`
    /// keeps core plugins whatever the deny-list says, so naming a core
    /// one here would assert that a plugin which cannot be disabled is
    /// disabled. This test used to name `scheduling`, and started
    /// failing the day scheduling became core — correctly, and for a
    /// reason that read like a toggle bug.
    #[test]
    fn disabled_plugin_hides_and_refuses_its_tools() {
        use task_plugin::{PluginChoice, PluginSet};
        let no_email = PluginSet::resolve(Some(&PluginChoice::Disabled(vec!["email".into()])));
        assert!(!no_email.contains("email"), "email must be disableable");

        let payload = tools_list_payload(&no_email);
        let names: Vec<&str> = payload["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        assert!(!names.contains(&"read_email"), "email tool listed");
        assert!(names.contains(&"create_task"), "core tool missing");
        let dropped = tool_catalog()
            .iter()
            .filter(|t| t.plugin == "email")
            .count();
        assert!(dropped > 0, "the email plugin owns tools");
        assert_eq!(names.len(), tool_catalog().len() - dropped);

        // Call gate mirrors the listing.
        assert!(plugin_gate("create_task", &no_email).is_ok());
        match plugin_gate("read_email", &no_email) {
            Err(Some(msg)) => {
                assert!(msg.contains("email"), "{msg}");
                assert!(msg.contains("disabled"), "{msg}");
            }
            other => panic!("expected a disabled-plugin message, got {other:?}"),
        }
        assert_eq!(plugin_gate("no_such_tool", &no_email), Err(None));
    }

    /// The telemetry catalog and its dispatch list must agree, and every
    /// required argument must be declared — the same shape rule the
    /// org catalog is held to.
    #[test]
    fn telemetry_tools_match_their_dispatch_list_and_declare_required_args() {
        let listed: Vec<String> = telemetry_tools_payload()
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_owned))
            .collect();
        assert_eq!(listed, TELEMETRY_TOOLS);
        for tool in telemetry_tools_payload() {
            let schema = &tool["inputSchema"];
            for key in schema["required"].as_array().expect("required").iter() {
                let key = key.as_str().expect("string");
                assert!(
                    schema["properties"].get(key).is_some(),
                    "`{}` requires `{key}` but doesn't declare it",
                    tool["name"]
                );
            }
            let desc = tool["description"].as_str().expect("description");
            assert!(desc.contains("Operator-only"), "{desc}");
        }
    }

    #[test]
    fn instructions_carry_the_org_and_the_current_date() {
        let now = chrono::Local::now();
        let text = server_instructions("acme", now);
        assert!(text.contains("org `acme`"));
        assert!(text.contains(&now.format("%Y").to_string()));
        assert!(text.contains(&now.format("%A").to_string()));
        // The orientation the whole feature exists for.
        assert!(text.contains("Task"));
        assert!(text.contains("capture_inbox"));
    }

    #[test]
    fn optional_field_separates_absent_from_cleared() {
        let args = json!({ "due": "2026-08-01", "scheduled": "", "priority": "high" });
        assert_eq!(
            optional_field(&args, "due"),
            Some(Some("2026-08-01".to_string()))
        );
        // Present but empty = clear it.
        assert_eq!(optional_field(&args, "scheduled"), Some(None));
        // Absent = don't touch it.
        assert_eq!(optional_field(&args, "missing"), None);
    }

    #[test]
    fn limits_are_clamped_and_defaulted() {
        assert_eq!(arg_limit(&json!({})), DEFAULT_LIMIT);
        assert_eq!(arg_limit(&json!({ "limit": 5 })), 5);
        assert_eq!(arg_limit(&json!({ "limit": 100_000 })), MAX_LIMIT);
        assert_eq!(arg_limit(&json!({ "limit": 0 })), 1);
    }

    #[test]
    fn arg_helpers_trim_and_reject_blank() {
        let args = json!({ "text": "  hi  ", "blank": "   " });
        assert_eq!(arg_str(&args, "text").as_deref(), Some("hi"));
        assert_eq!(arg_str(&args, "blank"), None);
        assert!(required_str(&args, "blank").is_err());
    }

    #[test]
    fn date_of_takes_the_day_from_either_stamp_form() {
        assert_eq!(date_of("2026-07-25T14:00:00-05:00"), "2026-07-25");
        assert_eq!(date_of("2026-07-25"), "2026-07-25");
    }

    #[test]
    fn tool_results_use_the_mcp_content_envelope() {
        let ok = tool_ok(&json!({ "count": 1 }));
        assert_eq!(ok["isError"], false);
        assert_eq!(ok["content"][0]["type"], "text");
        assert_eq!(ok["content"][0]["text"], r#"{"count":1}"#);

        let err = tool_err("no event with id `x`");
        assert_eq!(err["isError"], true);
        assert!(
            err["content"][0]["text"]
                .as_str()
                .expect("text")
                .contains("no event")
        );
    }

    #[test]
    fn backend_errors_become_actionable_text() {
        #[derive(Debug)]
        #[allow(dead_code)]
        enum FakeErr {
            NotFound,
            AlreadyExists(String),
            Io(String),
        }
        let msg = match backend_err("note", "nope.md", &FakeErr::NotFound) {
            ToolFailure::Message(m) => m,
            ToolFailure::Unknown => panic!("wrong variant"),
        };
        // Names the thing AND the way out — a bare "NotFound" leaves
        // the model with nothing to do next.
        assert!(msg.contains("nope.md"), "{msg}");
        assert!(msg.contains("list/search tool"), "{msg}");

        let msg = match backend_err("task", "a.md", &FakeErr::AlreadyExists("a.md".into())) {
            ToolFailure::Message(m) => m,
            ToolFailure::Unknown => panic!("wrong variant"),
        };
        assert!(msg.contains("already exists"), "{msg}");

        // Anything unrecognized keeps the raw detail rather than
        // swallowing it.
        let msg = match backend_err("note", "x.md", &FakeErr::Io("disk full".into())) {
            ToolFailure::Message(m) => m,
            ToolFailure::Unknown => panic!("wrong variant"),
        };
        assert!(msg.contains("disk full"), "{msg}");
    }

    #[test]
    fn rpc_envelopes_carry_the_request_id() {
        let ok = rpc_result(json!(7), json!({ "a": 1 }));
        assert_eq!(ok["jsonrpc"], "2.0");
        assert_eq!(ok["id"], 7);
        let err = rpc_error(json!("abc"), code::METHOD_NOT_FOUND, "nope");
        assert_eq!(err["id"], "abc");
        assert_eq!(err["error"]["code"], code::METHOD_NOT_FOUND);
    }
}
