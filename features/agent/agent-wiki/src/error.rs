use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum AgentWikiError {
    /// LLM response didn't start with the expected
    /// block-format header (e.g. `---FILE:` for ingest
    /// step 2). Body in `.1`.
    #[error("malformed response (expected {0}): {1}")]
    MalformedResponse(&'static str, String),

    /// A FILE block declared a target path the parser
    /// can't sanity-check (empty, outside `Wiki/`,
    /// missing extension).
    #[error("invalid file target {0}: {1}")]
    InvalidFileTarget(String, &'static str),

    /// A REVIEW block listed an unknown `type:` value.
    #[error("unknown review kind: {0}")]
    UnknownReviewKind(String),

    /// A LINT block listed an unknown `type:` value.
    #[error("unknown lint kind: {0}")]
    UnknownLintKind(String),

    /// Bridge-level orchestration failure.
    #[error("bridge: {0}")]
    Bridge(String),
}
