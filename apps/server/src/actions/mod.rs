//! Task's Action List: the commands an agent runs, registered once.
//!
//! This is the pilot of moving the MCP lane ([`crate::mcp`]) onto
//! architect's Action List (`architect::action::ActionList`, served by
//! `architect-mcp` in named-tool mode). Eight tools live here instead of
//! in `mcp.rs`'s hand-written catalog and `match`: `task_context`,
//! `list_tasks`, `create_task`, `update_task` ([`tasks`]),
//! `list_projects` ([`projects`]) and `capture_inbox`, `list_inbox`,
//! `process_inbox_item` ([`inbox`]). Each is an action whose arguments
//! are a facet type, so `tools/list`'s `inputSchema` is derived from the
//! same type the call is decoded into, rather than a JSON literal kept
//! in step with a `match` arm by hand.
//!
//! **Why every pilot action is registered by hand.** architect can also
//! project a whole service into the list (`#[architect::service(actions)]`,
//! `commands.md` section 7), but none of these eight can take that path
//! yet:
//!
//! - their backing traits (`task::TaskService`, `project::ProjectService`,
//!   `inbox_proto::Inbox`) are **per-org**: one backend per org, mounted
//!   on that org's vox router. The projection registers one backend into
//!   one list and goes through the generated HTTP face, and Task has no
//!   per-tenant HTTP mount to put it on, so they stay
//!   `#[architect::rpc]`;
//! - the tools are not one trait method each: `task_context` reads three
//!   services, `create_task` parses quick-add text and stamps attribution,
//!   `list_tasks` filters on top of `query`. A projected
//!   `TASK_SERVICE_CREATE(TaskInfo)` would change the agent's tool, which
//!   named mode exists to keep.
//!
//! **Who is calling.** An MCP call acts in one org for one caller, and an
//! Action List handler takes only its arguments. The list is built once
//! for the process ([`list`]); the org and the caller ride as an ambient
//! [`CallContext`] that [`call`] installs on the calling thread for the
//! length of one call. The handlers are sync (the backends are
//! architect's blocking-dispatcher impls), `architect-mcp` runs a sync
//! action on the thread that calls it, and `mcp_dispatch` runs [`call`]
//! on the blocking pool, so the context is always the call's own. A
//! handler run with no context (the registry test's hostile-args probe)
//! refuses, rather than acting in some org.
//!
//! What stays in `mcp.rs` for these tools: the transport, bearer rules,
//! org routing (the account lane's `org` argument, which is stripped
//! before the action sees its arguments), the bot tool scope and the
//! plugin gate. Those are unchanged.

pub mod inbox;
pub mod projects;
pub mod tasks;

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use architect::action::{ActionError, ActionList, ActionMeta, Effect, Handler};
use facet::Facet;
use serde_json::{Value, json};

use crate::mcp::ToolFailure;

/// One pilot tool: the action behind it, the MCP tool name an agent
/// already calls it by, and the `task_plugin::CATALOG` id that owns it
/// (what the plugin gate checks).
pub struct ActionTool {
    pub id: &'static str,
    pub tool: &'static str,
    pub plugin: &'static str,
}

/// Every action on Task's list, with its tool, in the order `tools/list`
/// offers them.
pub const TOOLS: &[ActionTool] = &[
    ActionTool {
        id: "TASKS_CONTEXT",
        tool: "task_context",
        plugin: "core",
    },
    ActionTool {
        id: "TASKS_LIST",
        tool: "list_tasks",
        plugin: "core",
    },
    ActionTool {
        id: "TASKS_CREATE",
        tool: "create_task",
        plugin: "core",
    },
    ActionTool {
        id: "TASKS_UPDATE",
        tool: "update_task",
        plugin: "core",
    },
    ActionTool {
        id: "INBOX_CAPTURE",
        tool: "capture_inbox",
        plugin: "core",
    },
    ActionTool {
        id: "INBOX_LIST",
        tool: "list_inbox",
        plugin: "core",
    },
    ActionTool {
        id: "PROJECTS_LIST",
        tool: "list_projects",
        plugin: "core",
    },
    ActionTool {
        id: "INBOX_PROCESS",
        tool: "process_inbox_item",
        plugin: "core",
    },
];

/// The pilot tool called `name`, if it is one.
#[must_use]
pub fn tool(name: &str) -> Option<&'static ActionTool> {
    TOOLS.iter().find(|t| t.tool == name)
}

/// Who one call acts for, and where: what an action reads instead of
/// arguments it must never take from the caller (`created_by` comes
/// from the key, not from the model).
pub struct CallContext {
    /// The org the call acts in.
    pub org: Arc<crate::OrgAppState>,
    /// The person behind the session, for the activity feed's actor.
    pub principal: Option<String>,
    /// Who the records a call makes are attributed to.
    pub creator: Option<crate::attribution::Creator>,
}

thread_local! {
    static CONTEXT: RefCell<Option<Arc<CallContext>>> = const { RefCell::new(None) };
}

/// Puts back the context that was installed before, even when the call
/// unwinds.
struct Restore(Option<Arc<CallContext>>);

impl Drop for Restore {
    fn drop(&mut self) {
        let previous = self.0.take();
        CONTEXT.with(|c| *c.borrow_mut() = previous);
    }
}

/// Run `f` with `ctx` as this thread's call context.
fn with_context<R>(ctx: Arc<CallContext>, f: impl FnOnce() -> R) -> R {
    let previous = CONTEXT.with(|c| c.borrow_mut().replace(ctx));
    let _restore = Restore(previous);
    f()
}

/// The context of the call running on this thread.
fn current() -> Result<Arc<CallContext>, ToolFailure> {
    CONTEXT.with(|c| c.borrow().clone()).ok_or_else(|| {
        ToolFailure::Message("no org to act in: this action runs as an MCP tool call".into())
    })
}

/// The metadata of one Task action. `description` is written for the
/// model: it is the tool's description in `tools/list`.
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

/// What a Task action's handler is: its arguments decoded, the call's
/// context, a JSON answer the model reads.
type Run<A> = fn(&CallContext, A) -> Result<Value, ToolFailure>;

/// Register `run` as `meta`'s action, taking `A`.
///
/// The answer is plain JSON (the same rows the tools answered before),
/// so the action is registered through `register_dynamic` with `A` as
/// its params shape and no result shape; the decode is the one
/// `ActionList::register` does, so a bad call is refused with the
/// params schema before `run` is reached.
fn register<A>(
    list: &ActionList,
    meta: &'static ActionMeta,
    effect: Effect,
    run: Run<A>,
) -> Result<(), ActionError>
where
    A: Facet<'static>,
{
    let id = meta.id;
    let handler: Handler = Arc::new(move |args: &[u8]| {
        let decoded: A = facet_json::from_slice::<A>(args).map_err(|e| ActionError::BadArgs {
            id: id.to_owned(),
            message: e.to_string(),
            expected: Some(architect::schema::json_schema(A::SHAPE)),
        })?;
        let failed = |message: String| ActionError::Failed {
            id: id.to_owned(),
            message,
        };
        let ctx = current().map_err(|f| failed(f.into_message()))?;
        let out = run(&ctx, decoded).map_err(|f| failed(f.into_message()))?;
        serde_json::to_vec(&out).map_err(|e| failed(format!("its result did not encode: {e}")))
    });
    list.register_dynamic(meta, effect, Some(A::SHAPE), None, handler)
}

/// Put every Task action on `list`.
///
/// # Errors
///
/// [`ActionError::Duplicate`] when one is already there.
pub fn register_all(list: &ActionList) -> Result<(), ActionError> {
    tasks::register(list)?;
    projects::register(list)?;
    inbox::register(list)
}

/// The process's Action List, built on first use.
#[must_use]
pub fn list() -> &'static ActionList {
    static LIST: OnceLock<ActionList> = OnceLock::new();
    LIST.get_or_init(|| {
        let list = ActionList::new();
        if let Err(e) = register_all(&list) {
            // The registry test keeps this from shipping; a live server
            // serves what did register rather than refusing to start.
            tracing::error!(error = %e, "registering Task's actions");
        }
        list
    })
}

/// The list as an MCP server in named mode: each action is its own tool,
/// under the name in [`TOOLS`].
#[must_use]
pub fn server() -> &'static architect_mcp::Server {
    static SERVER: OnceLock<architect_mcp::Server> = OnceLock::new();
    SERVER.get_or_init(|| {
        TOOLS.iter().fold(
            architect_mcp::Server::new("task", task_release_version::get(), list().clone())
                .with_tools(architect_mcp::Tools::Named),
            |server, t| server.with_tool_name(t.id, t.tool),
        )
    })
}

/// One JSON-RPC exchange with [`server`], answered with its `result`
/// (or its error, as `Err(message)`).
fn exchange(method: &str, params: &Value) -> Result<Value, String> {
    let request = json!({ "jsonrpc": "2.0", "id": 0, "method": method, "params": params });
    let reply = server()
        .handle(&request.to_string())
        .ok_or_else(|| "the action server sent no reply".to_owned())?;
    let mut reply: Value = serde_json::from_str(&reply)
        .map_err(|e| format!("the action server's reply is not JSON: {e}"))?;
    if let Some(error) = reply.get("error") {
        return Err(error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("the action server refused the call")
            .to_owned());
    }
    Ok(reply
        .get_mut("result")
        .map(Value::take)
        .unwrap_or(Value::Null))
}

/// The pilot tools as `tools/list` entries, in [`TOOLS`] order, for the
/// plugins `enabled` lets through.
#[must_use]
pub fn tool_entries(enabled: impl Fn(&str) -> bool) -> Vec<Value> {
    let listed = match exchange("tools/list", &json!({})) {
        Ok(mut result) => match result.get_mut("tools").map(Value::take) {
            Some(Value::Array(tools)) => tools,
            _ => Vec::new(),
        },
        Err(e) => {
            tracing::error!(error = %e, "listing Task's actions as tools");
            Vec::new()
        }
    };
    TOOLS
        .iter()
        .filter(|t| enabled(t.plugin))
        .filter_map(|t| listed.iter().find(|l| l["name"] == t.tool).cloned())
        .collect()
}

/// Run pilot tool `name` with `args` for `ctx`: the MCP `tools/call`
/// result (`content` + `isError`) to send back.
///
/// `args` may carry the account lane's `org`, which routed the call and
/// is not an argument of the action. Runs the handler on this thread:
/// call it on the blocking pool, as every tool is.
#[must_use]
pub fn call(ctx: CallContext, name: &str, mut args: Value) -> Value {
    if let Some(fields) = args.as_object_mut() {
        fields.remove("org");
    }
    crate::mcp::note_task_writer(&ctx.org, ctx.principal.as_deref(), name, &args);
    let params = json!({ "name": name, "arguments": args });
    match with_context(Arc::new(ctx), || exchange("tools/call", &params)) {
        Ok(result) => result,
        Err(message) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    }
}

/// A list tool's row cap: the caller's value, clamped, defaulted (as
/// `mcp.rs`'s `arg_limit`).
fn limit(asked: Option<u64>) -> usize {
    asked.map_or(crate::mcp::DEFAULT_LIMIT, |n| {
        usize::try_from(n)
            .unwrap_or(usize::MAX)
            .clamp(1, crate::mcp::MAX_LIMIT)
    })
}

/// A string argument as the tools have always read one: trimmed, and
/// blank means absent.
fn text(arg: Option<String>) -> Option<String> {
    arg.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}

/// A required string argument: trimmed, blank refused.
fn required(arg: String, key: &str) -> Result<String, ToolFailure> {
    text(Some(arg)).ok_or_else(|| ToolFailure::Message(format!("`{key}` is required")))
}

/// A field to change: absent leaves it alone, blank clears it.
fn change(arg: Option<String>) -> Option<Option<String>> {
    arg.map(|s| text(Some(s)))
}

/// A filter that names a principal, where `me` is the caller's own ref.
fn who(arg: Option<String>, ctx: &CallContext) -> Option<String> {
    let raw = text(arg)?;
    if raw.eq_ignore_ascii_case("me") {
        Some(
            ctx.creator
                .as_ref()
                .map_or_else(|| "none".to_owned(), |c| c.r#ref.clone()),
        )
    } else {
        Some(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry test every app's list gets: sound ids and schemas,
    /// and no handler that panics on, or takes, malformed arguments.
    #[test]
    fn the_action_list_is_sound() {
        let list = ActionList::new();
        register_all(&list).expect("registers");
        list.validate().expect("valid");
        list.probe_hostile_args()
            .expect("refuses hostile arguments");
        assert_eq!(list.len(), TOOLS.len());
        for t in TOOLS {
            assert!(list.info(t.id).is_some(), "{} is registered", t.id);
        }
    }

    #[test]
    fn registering_twice_is_refused() {
        let list = ActionList::new();
        register_all(&list).expect("registers");
        assert!(matches!(
            register_all(&list),
            Err(ActionError::Duplicate(_))
        ));
        assert!(list.validate().is_err());
    }

    #[test]
    fn every_tool_is_listed_under_its_old_name() {
        let names: Vec<String> = tool_entries(|_| true)
            .iter()
            .filter_map(|t| t["name"].as_str().map(str::to_owned))
            .collect();
        let expected: Vec<&str> = TOOLS.iter().map(|t| t.tool).collect();
        assert_eq!(names, expected);
    }

    #[test]
    fn a_tool_run_with_no_call_context_refuses() {
        let out = exchange(
            "tools/call",
            &json!({ "name": "list_projects", "arguments": {} }),
        )
        .expect("an answer");
        assert_eq!(out["isError"], true, "{out}");
        assert!(out.to_string().contains("no org to act in"), "{out}");
    }

    #[test]
    fn string_arguments_read_as_before() {
        assert_eq!(text(Some("  x ".into())), Some("x".into()));
        assert_eq!(text(Some("   ".into())), None);
        assert_eq!(change(None), None);
        assert_eq!(change(Some(String::new())), Some(None));
        assert_eq!(
            change(Some(" 2026-01-02 ".into())),
            Some(Some("2026-01-02".into()))
        );
        assert_eq!(limit(None), crate::mcp::DEFAULT_LIMIT);
        assert_eq!(limit(Some(0)), 1);
        assert_eq!(limit(Some(10_000)), crate::mcp::MAX_LIMIT);
    }
}
