//! Project actions: `list_projects`.
//!
//! Backed by the org's `project::ProjectService`, a per-org rpc trait,
//! so registered by hand (see the module docs of [`super`]).

use architect::action::{ActionError, ActionList, ActionMeta, Effect};
use facet::Facet;
use serde_json::{Value, json};

use super::{CallContext, limit, meta};
use crate::mcp::ToolFailure;

/// `list_projects`.
pub static LIST: ActionMeta = meta(
    "PROJECTS_LIST",
    "list",
    "List projects",
    "Projects",
    "The user's projects. Use it to attach work to the right project, or to answer 'what am I \
     working on'.",
);

/// How many projects to list.
#[derive(Facet)]
pub struct ListArgs {
    /// Max rows (default 50, max 200).
    pub limit: Option<u64>,
}

/// Put the project actions on `list`.
///
/// # Errors
///
/// [`ActionError::Duplicate`] when one is already there.
pub fn register(list: &ActionList) -> Result<(), ActionError> {
    super::register(list, &LIST, Effect::ReadOnly, list_projects)
}

fn list_projects(ctx: &CallContext, args: ListArgs) -> Result<Value, ToolFailure> {
    use project::ProjectService as _;
    let rows = ctx
        .org
        .projects
        .list()
        .map_err(|e| ToolFailure::Message(format!("{e:?}")))?;
    let out: Vec<Value> = rows
        .iter()
        .take(limit(args.limit))
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
