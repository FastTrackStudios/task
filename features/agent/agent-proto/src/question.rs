//! Structured multi-choice questions the agent asks
//! mid-turn. Distinct from [`crate::approval::Approval`]:
//! approvals are yes/no-ish (permission to act);
//! questions are open-ended clarifications with N labeled
//! options.
//!
//! Maps to `CodexMonitor`'s `RequestUserInputParams.questions`
//! and the `AskUserQuestion` CLI tool pattern. Multi-question
//! requests are supported (one round-trip can carry several
//! related questions).

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct QuestionRequest {
    pub id: String,
    pub session_id: String,
    pub message_id: String,
    /// One or more questions. Resolved together.
    pub questions: Vec<Question>,
    pub created_at: DateTime<Utc>,
    /// Replies indexed by question id; empty until
    /// resolved.
    pub answers: Vec<QuestionAnswer>,
    pub resolved_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct Question {
    pub id: String,
    /// Short chip-style label (`"Auth method"`).
    pub header: String,
    /// Full question text. Ends with `?`.
    pub text: String,
    pub options: Vec<QuestionOption>,
    /// Whether multiple options can be selected.
    pub multi_select: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct QuestionOption {
    pub label: String,
    /// Longer explanation rendered alongside the label.
    pub description: String,
    /// Optional preview block (ASCII / code) rendered
    /// side-by-side on focus.
    pub preview: String,
}

/// A runner asking on a ticket's behalf.
///
/// The agent-lane counterpart to the session-scoped flow: a question
/// here belongs to a **ticket**, because that is what gets
/// `needs-input` and what a human answers. `run` ties it back to the
/// attempt that raised it, so answering can resume the right session.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct AskQuestion {
    pub ticket: uuid::Uuid,
    /// The run that raised it. `None` when asked outside a run.
    pub run: Option<uuid::Uuid>,
    /// One or more questions, resolved together.
    pub questions: Vec<Question>,
}

#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub struct QuestionAnswer {
    /// Matches `Question.id`.
    pub question_id: String,
    /// Picked option labels (or free-form "Other" text).
    /// Single-select carries one entry.
    pub selected: Vec<String>,
    /// Free-form notes the user added.
    pub notes: String,
}
