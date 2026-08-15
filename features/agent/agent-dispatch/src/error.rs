use thiserror::Error;

#[derive(Debug, Error)]
pub enum DispatchError {
    #[error("agent: {0}")]
    Agent(agent_proto::AgentError),

    #[error("task write: {0}")]
    TaskWrite(#[from] task::WriteError),

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// The source task note couldn't be found on disk at
    /// `<vault_root>/<task.path>` — likely deleted out from
    /// under us between dispatch and complete.
    #[error("source task note missing: {path}")]
    SourceTaskMissing { path: String },

    #[error("invalid: {0}")]
    Invalid(String),
}

impl From<agent_proto::AgentError> for DispatchError {
    fn from(e: agent_proto::AgentError) -> Self {
        DispatchError::Agent(e)
    }
}
