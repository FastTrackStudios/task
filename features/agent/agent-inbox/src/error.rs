use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentInboxError {
    /// LLM response didn't contain the expected
    /// `---ITEM: <id>---` block structure. Excerpt in `.1`.
    #[error("malformed response (expected {0}): {1}")]
    MalformedResponse(&'static str, String),

    /// An ITEM block declared an `ACTION:` value outside
    /// `task` / `note` / `skip`.
    #[error("unknown action `{0}` for item {1}")]
    UnknownAction(String, String),

    /// An ITEM block was missing a field its action requires
    /// (`TITLE` for `task`, `PATH` for `note`).
    #[error("item {1}: missing required field {0}")]
    MissingField(&'static str, String),

    /// Bridge-level orchestration failure (backend spawn,
    /// turn error, timeout).
    #[error("bridge: {0}")]
    Bridge(String),
}
