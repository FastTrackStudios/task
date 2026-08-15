//! Live run state — what a monitor watches while an agent works.
//!
//! # Why a stream plus a snapshot, and not a CRDT document
//!
//! The requirement is that a viewer sees run state change without
//! refreshing, and that a viewer joining late sees recent output
//! rather than an empty pane. A CRDT document would give both, at the
//! cost of carrying every keystroke of agent output in a synced doc.
//!
//! The same two properties fall out of **fetch once, then fold**:
//! [`RunSnapshot`] is the once, [`RunEvent`] is the fold. It is the
//! contract the existing agent event stream already documents, there
//! is no merge to reconcile because runs have exactly one writer —
//! the runner that owns them — and output stays bounded because the
//! snapshot keeps only a tail.
//!
//! Output is **ephemeral**: it rides the stream and lives in the
//! bounded tail, and is never written to the vault. Full agent
//! transcripts would bloat a git-backed store fast.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::run::RunStatus;

/// How much output a snapshot carries for a late joiner.
pub const TAIL_BYTES: usize = 8 * 1024;

/// One thing that happened to a run.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub enum RunEvent {
    /// The run moved state.
    Status(RunStatus),
    /// Output arrived. Append to the tail.
    Output(String),
    /// What the agent is doing right now, for a one-line "current
    /// activity" display.
    Activity(String),
    /// The verify command returned.
    Verdict { passed: bool, exit_code: Option<i32> },
    /// The run asked a human something and is now blocked.
    Blocked { question_id: String },
}

/// A run event, tagged with the run it belongs to.
///
/// One subscription carries every run; each envelope names its run so
/// a monitor showing one run filters client-side and a fleet view
/// keeps them all — the same shape as the agent event stream.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct RunEventEnvelope {
    pub run: Uuid,
    pub ticket: Uuid,
    pub event: RunEvent,
    pub at: DateTime<Utc>,
}

/// Everything a viewer needs on arrival, before folding the stream.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct RunSnapshot {
    pub run: Uuid,
    pub ticket: Uuid,
    pub status: RunStatus,
    /// What the agent was last doing.
    pub activity: String,
    /// The last [`TAIL_BYTES`] of output.
    pub tail: String,
}

/// Append to a tail, keeping it under [`TAIL_BYTES`].
///
/// Trims on a char boundary — a tail cut mid-codepoint is not a
/// string, and this runs on every chunk.
pub fn append_tail(tail: &mut String, chunk: &str) {
    tail.push_str(chunk);
    if tail.len() <= TAIL_BYTES {
        return;
    }
    let mut start = tail.len() - TAIL_BYTES;
    while start < tail.len() && !tail.is_char_boundary(start) {
        start += 1;
    }
    *tail = tail[start..].to_string();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tail_grows_until_the_cap() {
        let mut t = String::new();
        append_tail(&mut t, "hello ");
        append_tail(&mut t, "world");
        assert_eq!(t, "hello world");
    }

    #[test]
    fn a_tail_stays_bounded_across_many_chunks() {
        let mut t = String::new();
        for _ in 0..1000 {
            append_tail(&mut t, &"x".repeat(100));
        }
        assert!(t.len() <= TAIL_BYTES, "{}", t.len());
    }

    #[test]
    fn the_tail_keeps_the_end_not_the_beginning() {
        let mut t = String::new();
        append_tail(&mut t, &"a".repeat(TAIL_BYTES));
        append_tail(&mut t, "THE-END");
        assert!(t.ends_with("THE-END"), "recent output is what matters");
        assert!(t.len() <= TAIL_BYTES);
    }

    #[test]
    fn trimming_never_splits_a_character() {
        let mut t = String::new();
        // Multi-byte throughout, so a naive byte cut would panic or
        // produce invalid UTF-8.
        for _ in 0..2000 {
            append_tail(&mut t, "é日");
        }
        assert!(t.len() <= TAIL_BYTES);
        assert!(t.chars().all(|c| c == 'é' || c == '日'));
    }
}
