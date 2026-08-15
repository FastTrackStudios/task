//! Structured questions from an agent, and the answers that unblock
//! them.
//!
//! An agent that needs a decision **stops and asks**. It never
//! answers on the human's behalf — a human-in-the-loop question
//! resolves only through the human, and there is deliberately no
//! code path here that resolves one without an answer.
//!
//! The grill queue is [`Questions::unresolved_questions`]: asking and
//! seeing are one mechanism rather than two, so a question cannot be
//! raised without appearing.

use uuid::Uuid;

use crate::error::AgentError;
use crate::question::{AskQuestion, QuestionAnswer, QuestionRequest};

#[architect::rpc]
pub trait Questions {
    /// Raise a question against a ticket.
    ///
    /// The caller is responsible for moving the ticket to
    /// `needs-input`; this records the question itself.
    async fn ask_question(&self, ask: AskQuestion) -> Result<QuestionRequest, AgentError>;

    /// Every unresolved question — the grill queue.
    async fn unresolved_questions(&self) -> Result<Vec<QuestionRequest>, AgentError>;

    /// Unresolved questions on one ticket.
    async fn questions_for_ticket(&self, ticket: Uuid) -> Result<Vec<QuestionRequest>, AgentError>;

    /// Legacy session-scoped listing, kept for the conversational
    /// backends that already speak it.
    async fn list_pending_questions(
        &self,
        session_id: String,
    ) -> Result<Vec<QuestionRequest>, AgentError>;

    /// Answer a question, resolving it.
    ///
    /// Answering an already-resolved question is an error rather
    /// than a silent overwrite: the first answer is the one the agent
    /// acted on.
    async fn answer_question(
        &self,
        request_id: String,
        answers: Vec<QuestionAnswer>,
    ) -> Result<QuestionRequest, AgentError>;

    /// The ticket a question belongs to, if any.
    async fn question_ticket(&self, request_id: String) -> Result<Option<Uuid>, AgentError>;
}
