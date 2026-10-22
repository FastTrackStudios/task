//! Task's commands on architect's Action List — the MCP tools that have
//! moved off the hand-written catalogue in [`crate::mcp`].
//!
//! Each tool here is one action: an id (`TASK_CREATE`), facet types for
//! what it takes and gives (so its `inputSchema` is generated, not
//! written by hand), and an [`Effect`] a model can trust. They are served
//! by `architect_mcp` in named-tool mode under the names agents already
//! call (`create_task`), so moving a tool here changes no client.
//!
//! What stays in `mcp.rs` is the transport: the org and account lanes,
//! bearer rules, bot key scopes and the plugin gate all run before a call
//! reaches the list, exactly as they did for the hand-written tools.
//!
//! A handler needs the org it acts in and who is calling, and an
//! action's handler is handed only its arguments, so the two ride in the
//! closures as a [`Ctx`]: the list that runs a call is built for it
//! ([`list`]). What the tools *are* needs no caller, so `tools/list` reads
//! one unbound list ([`tool_entries`]), built once.

use std::sync::Arc;

use architect::action::{ActionError, ActionList, ActionMeta, Effect};
use facet::Facet;
use serde_json::{Value, json};

use crate::attribution::Creator;

mod inbox;
mod projects;
mod tasks;

pub use tasks::TaskView;

/// Who a call is from and where it acts — what every handler reads
/// besides its arguments.
pub struct Ctx {
    /// The org the call acts in (the lane's pinned org, or the account
    /// lane's `org` argument).
    pub org: Arc<crate::OrgAppState>,
    /// The caller's account id, for the activity feed's actor ledger.
    pub principal: Option<String>,
    /// The caller as a creator: what new records are attributed to,
    /// and what a filter's `me` means.
    pub creator: Option<Creator>,
}

/// One action served as a named MCP tool: its id, the tool name agents
/// call it by, and the plugin that owns it (the gate `mcp.rs` applies
/// before a call reaches the list).
pub struct Tool {
    pub id: &'static str,
    pub name: &'static str,
    pub plugin: &'static str,
}

/// Every tool on the list, in the order `tools/list` offers them.
pub const TOOLS: &[Tool] = &[
    tool(tasks::CONTEXT.id, "task_context"),
    tool(tasks::LIST.id, "list_tasks"),
    tool(tasks::CREATE.id, "create_task"),
    tool(tasks::UPDATE.id, "update_task"),
    tool(projects::LIST.id, "list_projects"),
    tool(inbox::CAPTURE.id, "capture_inbox"),
    tool(inbox::LIST.id, "list_inbox"),
    tool(inbox::PROCESS.id, "process_inbox_item"),
];

/// All eight are platform tools: the `core` plugin, which no org can
/// turn off. The gate still reads it, so a tool moved here from a
/// plugin keeps its toggle.
const fn tool(id: &'static str, name: &'static str) -> Tool {
    Tool {
        id,
        name,
        plugin: "core",
    }
}

/// The list's tool named `name`, if it has one.
#[must_use]
pub fn find(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// The Action List for one call, its handlers bound to `ctx`.
///
/// # Errors
///
/// [`ActionError::Duplicate`] if two actions share an id — a bug the
/// registry test catches.
pub fn list(ctx: &Arc<Ctx>) -> Result<ActionList, ActionError> {
    build(Some(ctx))
}

/// The list with no caller: it describes every action, and refuses to
/// run one. What `tools/list` and the registry tests read.
///
/// # Errors
///
/// As [`list`].
pub fn unbound() -> Result<ActionList, ActionError> {
    build(None)
}

fn build(ctx: Option<&Arc<Ctx>>) -> Result<ActionList, ActionError> {
    let list = ActionList::new();
    tasks::register(&list, ctx)?;
    projects::register(&list, ctx)?;
    inbox::register(&list, ctx)?;
    Ok(list)
}

/// `list` as an MCP server offering each action as its own tool, under
/// the names in [`TOOLS`].
#[must_use]
pub fn server(list: ActionList) -> architect_mcp::Server {
    TOOLS.iter().fold(
        architect_mcp::Server::new("task", env!("CARGO_PKG_VERSION"), list)
            .with_tools(architect_mcp::Tools::Named),
        |server, t| server.with_tool_name(t.id, t.name),
    )
}

/// The list's tools as `tools/list` entries, in [`TOOLS`] order.
/// Built once per process: what the tools are never changes.
///
/// # Panics
///
/// If the list cannot be built or listed — a programming error the
/// registry test catches before it ships.
#[must_use]
pub fn tool_entries() -> &'static [Value] {
    static ENTRIES: std::sync::OnceLock<Vec<Value>> = std::sync::OnceLock::new();
    ENTRIES.get_or_init(|| {
        let list = unbound().expect("the action list registers");
        let server = server(list);
        let message = json!({ "jsonrpc": "2.0", "id": 0, "method": "tools/list" });
        // Listing runs no action, so the blocking door never waits.
        let reply = server
            .handle(&message.to_string())
            .expect("tools/list replies");
        let reply: Value = serde_json::from_str(&reply).expect("tools/list reply is JSON");
        let listed = reply["result"]["tools"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        in_tools_order(listed)
    })
}

/// `listed` (architect lists by id) in [`TOOLS`] order: the order the
/// hand-written catalogue offered them in.
fn in_tools_order(mut listed: Vec<Value>) -> Vec<Value> {
    let rank = |t: &Value| {
        let name = t["name"].as_str().unwrap_or_default();
        TOOLS
            .iter()
            .position(|x| x.name == name)
            .unwrap_or(usize::MAX)
    };
    listed.sort_by_key(rank);
    listed
}

/// Run tool `name` with `args`; the reply is MCP's tool result
/// (`content` + `isError`), a failure inside it for the model to read.
///
/// # Errors
///
/// The server's protocol-level refusal (no such tool), as text.
pub async fn call(
    server: &architect_mcp::Server,
    name: &str,
    args: Value,
) -> Result<Value, String> {
    exchange(
        server,
        "tools/call",
        json!({ "name": name, "arguments": args }),
    )
    .await
}

/// One JSON-RPC exchange with `server`: the `result`, or the `error`'s
/// message.
async fn exchange(
    server: &architect_mcp::Server,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let message = json!({ "jsonrpc": "2.0", "id": 0, "method": method, "params": params });
    let reply = server
        .handle_async(&message.to_string())
        .await
        .ok_or_else(|| format!("{method} got no reply"))?;
    let mut reply: Value =
        serde_json::from_str(&reply).map_err(|e| format!("{method} reply is not JSON: {e}"))?;
    if let Some(result) = reply.get_mut("result") {
        return Ok(result.take());
    }
    Err(reply["error"]["message"]
        .as_str()
        .unwrap_or("the action list refused the call")
        .to_owned())
}

/// Register an action whose work is a blocking backend call. The
/// domain backends are architect's blocking-dispatcher trait impls
/// (`vault_sync` takes `blocking_read` locks, which panic on an async
/// worker), so the work runs on the blocking pool and the action awaits
/// it.
///
/// With no `ctx` (the [`unbound`] list) the action is described but
/// refuses to run.
fn register_blocking<A, R>(
    list: &ActionList,
    meta: &'static ActionMeta,
    effect: Effect,
    ctx: Option<&Arc<Ctx>>,
    run: fn(&Ctx, A) -> Result<R, String>,
) -> Result<(), ActionError>
where
    A: Facet<'static> + Send + 'static,
    R: Facet<'static> + Send + 'static,
{
    let ctx = ctx.map(Arc::clone);
    list.register_async(meta, effect, move |args: A| {
        let ctx = ctx.clone();
        async move {
            let ctx = ctx.ok_or("this list only describes its tools; it runs none")?;
            tokio::task::spawn_blocking(move || run(&ctx, args))
                .await
                .map_err(|e| format!("tool panicked: {e}"))?
        }
    })
}

/// An action's metadata. Everything a menu or palette would show is
/// derived from the id and description; tools have no shortcuts.
const fn meta(
    id: &'static str,
    method_name: &'static str,
    display_name: &'static str,
    category: &'static str,
    description: &'static str,
) -> ActionMeta {
    ActionMeta {
        id,
        trait_name: "Task",
        method_name,
        display_name,
        description,
        category,
        group: "",
        shortcut: "",
        toggleable: false,
        undo: false,
    }
}

/// A text argument as the hand-written tools read one: trimmed, and
/// blank the same as absent.
fn present(value: Option<String>) -> Option<String> {
    value.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// A filter naming a person: `me` is the caller's own creator ref
/// (`none` when the caller has none), anything else is taken as given.
fn who(value: Option<String>, creator: Option<&Creator>) -> Option<String> {
    let raw = present(value)?;
    if raw.eq_ignore_ascii_case("me") {
        return Some(creator.map_or_else(|| "none".to_owned(), |c| c.r#ref.clone()));
    }
    Some(raw)
}

/// Note the caller of a task write in the org's actor ledger, so the
/// activity feed names who made it (as `mcp::note_task_writer` does for
/// the tools still in `mcp.rs`).
// t[impl attribution.assignee.logged]
fn note_writer(ctx: &Ctx, key: String) {
    if let Some(user) = &ctx.principal {
        ctx.org.actors.note(key, Some(user.clone()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn me_is_the_callers_ref_and_blank_is_absent() {
        let me = Creator::user("00000000-0000-0000-0000-000000000001", None);
        assert_eq!(who(Some(" me ".into()), Some(&me)), Some(me.r#ref.clone()));
        assert_eq!(who(Some("ME".into()), None), Some("none".to_owned()));
        assert_eq!(who(Some("  ".into()), Some(&me)), None);
        assert_eq!(who(Some("bot:x".into()), Some(&me)), Some("bot:x".into()));
        assert_eq!(present(Some(" a ".into())), Some("a".into()));
    }

    #[test]
    fn every_tool_names_a_known_plugin_and_is_unique() {
        let mut names = std::collections::HashSet::new();
        for t in TOOLS {
            assert!(names.insert(t.name), "duplicate tool `{}`", t.name);
            assert!(task_plugin::find(t.plugin).is_some(), "`{}`", t.name);
            assert_eq!(find(t.name).map(|f| f.id), Some(t.id));
        }
        assert_eq!(TOOLS.len(), 8);
    }

    /// The registry test: ids well-formed and unique, schemas objects,
    /// and hostile arguments refused before a handler runs (the probe
    /// decodes async actions without polling them).
    #[test]
    fn the_list_validates_and_refuses_hostile_args() {
        let list = unbound().expect("registers");
        assert_eq!(list.len(), TOOLS.len());
        list.validate().expect("valid");
        list.probe_hostile_args().expect("hostile args refused");
        for t in TOOLS {
            assert!(list.info(t.id).is_some(), "`{}` registered", t.id);
            assert!(list.is_async(t.id), "`{}` awaits its backend", t.id);
        }
    }

    #[test]
    fn every_tool_is_listed_once_under_its_name() {
        let names: Vec<&str> = tool_entries()
            .iter()
            .filter_map(|t| t["name"].as_str())
            .collect();
        let want: Vec<&str> = TOOLS.iter().map(|t| t.name).collect();
        assert_eq!(names, want);
    }

    #[tokio::test]
    async fn the_unbound_list_describes_but_runs_nothing() {
        let server = server(unbound().expect("registers"));
        let reply = call(&server, "list_projects", json!({}))
            .await
            .expect("a tool result");
        assert_eq!(reply["isError"], true, "{reply}");
        let text = reply["content"][0]["text"].as_str().unwrap_or_default();
        assert!(text.contains("runs none"), "{text}");
        assert!(call(&server, "no_such_tool", json!({})).await.is_err());
    }

    #[tokio::test]
    async fn a_missing_required_argument_is_refused_with_the_schema() {
        let server = server(unbound().expect("registers"));
        let reply = call(&server, "create_task", json!({ "due": "2026-08-01" }))
            .await
            .expect("a tool result");
        assert_eq!(reply["isError"], true, "{reply}");
        let text = reply["content"][0]["text"].as_str().unwrap_or_default();
        let body: Value = serde_json::from_str(text).expect("JSON error");
        assert!(body["takes"]["required"].is_array(), "{body}");
    }
}
