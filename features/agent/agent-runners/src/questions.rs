//! Persistence for agent questions — the grill queue.
//!
//! One table. The `QuestionRequest` rides as JSON beside the columns
//! that are actually queried: the ticket it blocks, and whether it is
//! resolved.
//!
//! There is no code path here that resolves a question without an
//! answer. That is the point: an agent must never stand in for the
//! human side of a decision, and the storage layer is the last place
//! that rule can be quietly broken.

use agent_proto::error::AgentError;
use agent_proto::question::{AskQuestion, QuestionAnswer, QuestionRequest};
use agent_proto::service::questions::Questions;
use chrono::Utc;
use sea_orm::{ConnectionTrait, DatabaseConnection, Statement, Value};
use uuid::Uuid;

#[derive(Clone, Debug)]
pub struct QuestionStore {
    conn: DatabaseConnection,
}

impl QuestionStore {
    #[must_use]
    pub fn new(conn: DatabaseConnection) -> Self {
        Self { conn }
    }

    fn backend(&self) -> sea_orm::DatabaseBackend {
        self.conn.get_database_backend()
    }

    async fn exec(&self, sql: &str, values: Vec<Value>) -> Result<(), AgentError> {
        self.conn
            .execute(Statement::from_sql_and_values(self.backend(), sql, values))
            .await
            .map_err(|e| AgentError::Backend(format!("questions: {e}")))?;
        Ok(())
    }

    async fn rows(
        &self,
        sql: &str,
        values: Vec<Value>,
    ) -> Result<Vec<(QuestionRequest, Option<Uuid>)>, AgentError> {
        let rows = self
            .conn
            .query_all(Statement::from_sql_and_values(self.backend(), sql, values))
            .await
            .map_err(|e| AgentError::Backend(format!("questions: {e}")))?;

        let mut out = Vec::with_capacity(rows.len());
        for r in rows {
            let json: String = r
                .try_get("", "json")
                .map_err(|e| AgentError::Backend(format!("question json: {e}")))?;
            let ticket: String = r
                .try_get("", "ticket")
                .map_err(|e| AgentError::Backend(format!("question ticket: {e}")))?;
            match serde_json::from_str::<QuestionRequest>(&json) {
                Ok(q) => out.push((q, Uuid::parse_str(&ticket).ok())),
                Err(e) => tracing::warn!(error = %e, "skipping undecodable question"),
            }
        }
        Ok(out)
    }

    async fn one(&self, id: &str) -> Result<(QuestionRequest, Option<Uuid>), AgentError> {
        self.rows(
            "SELECT json, ticket FROM agent_questions WHERE id = ?",
            vec![id.into()],
        )
        .await?
        .pop()
        .ok_or_else(|| AgentError::AgentTaskNotFound(id.to_string()))
    }

    async fn save(&self, q: &QuestionRequest, ticket: Option<Uuid>) -> Result<(), AgentError> {
        let json = serde_json::to_string(q)
            .map_err(|e| AgentError::Backend(format!("encode question: {e}")))?;
        self.exec(
            "INSERT INTO agent_questions (id, ticket, resolved, json) VALUES (?,?,?,?) \
             ON CONFLICT(id) DO UPDATE SET resolved = excluded.resolved, json = excluded.json",
            vec![
                q.id.clone().into(),
                ticket.map(|t| t.to_string()).unwrap_or_default().into(),
                i32::from(q.resolved_at.is_some()).into(),
                json.into(),
            ],
        )
        .await
    }
}

impl Questions for QuestionStore {
    async fn ask_question(&self, ask: AskQuestion) -> Result<QuestionRequest, AgentError> {
        if ask.questions.is_empty() {
            return Err(AgentError::Invalid("a question with no questions".into()));
        }
        let request = QuestionRequest {
            id: Uuid::new_v4().to_string(),
            // The run is the session for agent-lane questions; a
            // conversational backend fills this with its own.
            session_id: ask.run.map(|r| r.to_string()).unwrap_or_default(),
            message_id: String::new(),
            questions: ask.questions,
            created_at: Utc::now(),
            answers: Vec::new(),
            resolved_at: None,
        };
        self.save(&request, Some(ask.ticket)).await?;
        Ok(request)
    }

    async fn unresolved_questions(&self) -> Result<Vec<QuestionRequest>, AgentError> {
        Ok(self
            .rows(
                "SELECT json, ticket FROM agent_questions WHERE resolved = 0 ORDER BY id",
                vec![],
            )
            .await?
            .into_iter()
            .map(|(q, _)| q)
            .collect())
    }

    async fn questions_for_ticket(&self, ticket: Uuid) -> Result<Vec<QuestionRequest>, AgentError> {
        Ok(self
            .rows(
                "SELECT json, ticket FROM agent_questions WHERE ticket = ? AND resolved = 0",
                vec![ticket.to_string().into()],
            )
            .await?
            .into_iter()
            .map(|(q, _)| q)
            .collect())
    }

    async fn list_pending_questions(
        &self,
        session_id: String,
    ) -> Result<Vec<QuestionRequest>, AgentError> {
        Ok(self
            .unresolved_questions()
            .await?
            .into_iter()
            .filter(|q| q.session_id == session_id)
            .collect())
    }

    async fn answer_question(
        &self,
        request_id: String,
        answers: Vec<QuestionAnswer>,
    ) -> Result<QuestionRequest, AgentError> {
        let (mut q, ticket) = self.one(&request_id).await?;
        if q.resolved_at.is_some() {
            return Err(AgentError::Conflict(format!(
                "question {request_id} is already answered"
            )));
        }
        if answers.is_empty() {
            // Resolving with nothing would be the agent answering
            // itself by another name.
            return Err(AgentError::Invalid(
                "an answer must actually answer something".into(),
            ));
        }
        q.answers = answers;
        q.resolved_at = Some(Utc::now());
        self.save(&q, ticket).await?;
        Ok(q)
    }

    async fn question_ticket(&self, request_id: String) -> Result<Option<Uuid>, AgentError> {
        Ok(self.one(&request_id).await?.1)
    }
}
