//! Import sessions from an external CLI's on-disk logs.
//! Implemented by backends that watch external agent state
//! (Codex CLI reads `~/.codex/sessions/`, Claude CLI reads
//! `~/.claude/...`).

use crate::error::AgentError;
use crate::session::Session;

#[architect::rpc]
pub trait ExternalImport {
    /// Import an external-CLI session log into Task. Backend
    /// reads the log, materializes a [`Session`] + its
    /// messages, returns the new session. Idempotent: re-
    /// importing the same log returns the existing session
    /// id.
    fn import_external_session(
        &self,
        backend_id: &str,
        log_path: &str,
        project_id: &str,
    ) -> Result<Session, AgentError>;
}
