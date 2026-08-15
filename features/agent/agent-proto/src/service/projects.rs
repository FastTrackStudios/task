//! Workspace/project registry (sessions live under projects).

use crate::error::AgentError;
use crate::project::Project;

#[architect::rpc]
pub trait Projects {
    fn upsert_project(&self, project: Project) -> Result<Project, AgentError>;
    fn remove_project(&self, project_id: &str) -> Result<(), AgentError>;
    fn list_projects(&self) -> Result<Vec<Project>, AgentError>;
    fn read_project(&self, project_id: &str) -> Result<Project, AgentError>;
}
