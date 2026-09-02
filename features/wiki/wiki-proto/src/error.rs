//! Trait-boundary error type for `WikiService`. Backends map
//! their internal errors (filesystem IO, malformed YAML, missing
//! state files, etc.) into this enum.

use facet::Facet;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq, Facet, Error)]
#[repr(C)]
pub enum WikiError {
    /// The named wiki doesn't exist (no `<vault>/Wiki/` folder
    /// or no entry for `wiki_id` on the server).
    #[error("wiki not found: {0}")]
    WikiNotFound(String),

    /// A wiki page, source, or state file doesn't exist.
    #[error("not found: {0}")]
    NotFound(String),

    /// `Wiki/schema.md` or `Wiki/purpose.md` is missing — the
    /// LLM contract is undefined. Callers should bootstrap one
    /// (see [`crate::schema::default_schema_doc`]) before
    /// proceeding.
    #[error("schema missing: {0}")]
    SchemaMissing(String),

    /// Frontmatter parse failure (bad YAML, missing required
    /// field). The `String` carries the file path; the second
    /// the parser detail.
    #[error("malformed frontmatter in {0}: {1}")]
    MalformedFrontmatter(String, String),

    /// An ingest / review / research task id doesn't match any
    /// known item in the queue.
    #[error("unknown task: {0}")]
    UnknownTask(String),

    /// A method was called with a payload that doesn't satisfy
    /// the state machine (e.g. `complete_ingest` on a task that
    /// never reached the `Generated` state).
    #[error("illegal state: {0}")]
    IllegalState(String),

    /// The caller may not do this — not an Editor, not a member of the
    /// owning org, or a wiki that has closed requests. Distinct from
    /// [`Self::IllegalState`] because it is about *who*, not *when*,
    /// and a client shows it as a refusal with the reason
    /// (`wiki.boundary.no-subscribe`, `wiki.edit.gate`).
    #[error("refused: {0}")]
    Refused(String),

    /// Two sides changed the same lines. Carries the paths so both
    /// parties can be shown what to resolve; never resolved by
    /// recency (`wiki.edit.rebase`, `wiki.subscribe.refresh`).
    #[error("conflict: {0}")]
    Conflict(String),

    /// Backend IO failure (disk full, permission denied, etc.).
    #[error("io: {0}")]
    Io(String),

    /// Backend-internal invariant violation. Should never
    /// happen; if it does, it's a backend bug.
    #[error("backend: {0}")]
    Backend(String),
}
