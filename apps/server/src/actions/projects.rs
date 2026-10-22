//! Projects: list.

use std::sync::Arc;

use architect::action::{ActionError, ActionList, ActionMeta, Effect};
use facet::Facet;
use project::ProjectService as _;

use super::{Ctx, meta, register_blocking};
use crate::mcp::clamp_limit;

pub(super) static LIST: ActionMeta = meta(
    "PROJECT_LIST",
    "list_projects",
    "List projects",
    "projects",
    "The user's projects. Use it to attach work to the right project, or to answer 'what \
     am I working on'.",
);

pub(super) fn register(list: &ActionList, ctx: Option<&Arc<Ctx>>) -> Result<(), ActionError> {
    register_blocking(list, &LIST, Effect::ReadOnly, ctx, list_projects)
}

#[derive(Facet, Debug, Clone, Copy)]
pub struct ListProjects {
    /// Max rows (default 50, max 200).
    limit: Option<u64>,
}

#[derive(Facet, Debug)]
pub struct ProjectList {
    count: usize,
    projects: Vec<ProjectRow>,
}

#[derive(Facet, Debug)]
pub struct ProjectRow {
    id: String,
    title: String,
    status: String,
    path: String,
}

fn list_projects(ctx: &Ctx, args: ListProjects) -> Result<ProjectList, String> {
    let rows = ctx.org.projects.list().map_err(|e| format!("{e:?}"))?;
    let projects: Vec<ProjectRow> = rows
        .iter()
        .take(clamp_limit(args.limit))
        .map(|p| ProjectRow {
            id: p.id.to_string(),
            title: p.title.clone(),
            status: p.status.clone(),
            path: p.path.clone(),
        })
        .collect();
    Ok(ProjectList {
        count: projects.len(),
        projects,
    })
}
