//! `Wiki/_state/review.json` — review queue items raised by
//! the LLM that need curator approval. Persisted as plain JSON
//! so a curator can hand-edit (and so `agent-wiki` can write
//! new items between server restarts).
//!
//! Mirrors the shape of `findings.rs`. The wire-side type is
//! [`wiki_proto::review::ReviewItem`]; this module stores a
//! local mirror keyed on `id` + offers `enqueue / list /
//! mark_resolved / mark_dismissed` verbs the backend dispatches
//! against.
//!
//! Suggestions on each item carry a [`wiki_proto::review::ReviewAction`]
//! — the curator picks one, the backend applies the side-effects
//! (`RewritePage` overwrites the named page; `AppendNote` appends;
//! `Research` promotes to a [`wiki_proto::research::ResearchPlan`]).

use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;
use wiki_proto::paths;
use wiki_proto::review::{
    ReviewAction as ProtoReviewAction, ReviewItem as ProtoReviewItem,
    ReviewKind as ProtoReviewKind, ReviewStatus as ProtoReviewStatus,
    ReviewSuggestion as ProtoReviewSuggestion,
};

use crate::error::WikiLiveError;
use crate::state::StateFile;
use crate::vault::WikiLive;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewKind {
    Contradiction,
    DestructiveEdit,
    Uncertain,
    SchemaConflict,
    PeerDivergence,
}

impl ReviewKind {
    fn from_proto(k: ProtoReviewKind) -> Self {
        match k {
            ProtoReviewKind::Contradiction => Self::Contradiction,
            ProtoReviewKind::DestructiveEdit => Self::DestructiveEdit,
            ProtoReviewKind::Uncertain => Self::Uncertain,
            ProtoReviewKind::SchemaConflict => Self::SchemaConflict,
            ProtoReviewKind::PeerDivergence => Self::PeerDivergence,
        }
    }
    fn to_proto(self) -> ProtoReviewKind {
        match self {
            Self::Contradiction => ProtoReviewKind::Contradiction,
            Self::DestructiveEdit => ProtoReviewKind::DestructiveEdit,
            Self::Uncertain => ProtoReviewKind::Uncertain,
            Self::SchemaConflict => ProtoReviewKind::SchemaConflict,
            Self::PeerDivergence => ProtoReviewKind::PeerDivergence,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewStatus {
    Open,
    Resolved,
    Dismissed,
}

impl ReviewStatus {
    fn from_proto(s: ProtoReviewStatus) -> Self {
        match s {
            ProtoReviewStatus::Open => Self::Open,
            ProtoReviewStatus::Resolved => Self::Resolved,
            ProtoReviewStatus::Dismissed => Self::Dismissed,
        }
    }
    fn to_proto(self) -> ProtoReviewStatus {
        match self {
            Self::Open => ProtoReviewStatus::Open,
            Self::Resolved => ProtoReviewStatus::Resolved,
            Self::Dismissed => ProtoReviewStatus::Dismissed,
        }
    }
}

/// Local (JSON-persisted) suggestion. We serialize the action
/// kind tag-only — `RewritePage` / `AppendNote` payloads are
/// re-constructed at read time when handing the item back to
/// the curator. (For long markdown blobs we'd prefer not to
/// double-store on disk; the LLM-proposed bodies live in the
/// review.json itself via the `details` field below.)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewSuggestion {
    pub label: String,
    pub action: ReviewActionMirror,
}

impl ReviewSuggestion {
    fn from_proto(s: ProtoReviewSuggestion) -> Self {
        Self {
            label: s.label,
            action: ReviewActionMirror::from_proto(s.action),
        }
    }
    #[allow(clippy::wrong_self_convention)]
    fn to_proto(self) -> ProtoReviewSuggestion {
        ProtoReviewSuggestion {
            label: self.label,
            action: self.action.to_proto(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReviewActionMirror {
    RewritePage { path: String, markdown: String },
    AppendNote { path: String, body: String },
    Research { query: String },
    AcceptNoOp,
    Dismiss { reason: String },
}

impl ReviewActionMirror {
    fn from_proto(a: ProtoReviewAction) -> Self {
        match a {
            ProtoReviewAction::RewritePage { path, markdown } => {
                Self::RewritePage { path, markdown }
            }
            ProtoReviewAction::AppendNote { path, body } => Self::AppendNote { path, body },
            ProtoReviewAction::Research { query } => Self::Research { query },
            ProtoReviewAction::AcceptNoOp => Self::AcceptNoOp,
            ProtoReviewAction::Dismiss { reason } => Self::Dismiss { reason },
        }
    }
    #[allow(clippy::wrong_self_convention)]
    fn to_proto(self) -> ProtoReviewAction {
        match self {
            Self::RewritePage { path, markdown } => {
                ProtoReviewAction::RewritePage { path, markdown }
            }
            Self::AppendNote { path, body } => ProtoReviewAction::AppendNote { path, body },
            Self::Research { query } => ProtoReviewAction::Research { query },
            Self::AcceptNoOp => ProtoReviewAction::AcceptNoOp,
            Self::Dismiss { reason } => ProtoReviewAction::Dismiss { reason },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewItem {
    pub id: String,
    pub kind: ReviewKind,
    pub subjects: Vec<String>,
    pub message: String,
    pub suggestions: Vec<ReviewSuggestion>,
    pub raised_at: chrono::DateTime<Utc>,
    pub status: ReviewStatus,
}

impl ReviewItem {
    pub fn from_proto(p: ProtoReviewItem) -> Self {
        Self {
            id: p.id,
            kind: ReviewKind::from_proto(p.kind),
            subjects: p.subjects,
            message: p.message,
            suggestions: p
                .suggestions
                .into_iter()
                .map(ReviewSuggestion::from_proto)
                .collect(),
            raised_at: p.raised_at,
            status: ReviewStatus::from_proto(p.status),
        }
    }
    pub fn to_proto(self) -> ProtoReviewItem {
        ProtoReviewItem {
            id: self.id,
            kind: self.kind.to_proto(),
            subjects: self.subjects,
            message: self.message,
            suggestions: self
                .suggestions
                .into_iter()
                .map(ReviewSuggestion::to_proto)
                .collect(),
            raised_at: self.raised_at,
            status: self.status.to_proto(),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub(crate) struct ReviewFile {
    #[serde(default)]
    pub(crate) items: Vec<ReviewItem>,
}

impl StateFile for ReviewFile {
    const FILENAME: &'static str = paths::REVIEW_JSON;
}

impl WikiLive {
    /// Push (or upsert by id) a review item onto the queue.
    /// Stamps `id` when empty + defaults `raised_at` to now.
    pub fn enqueue_review(&self, item: ProtoReviewItem) -> Result<(), WikiLiveError> {
        let mut file: ReviewFile = self.load_state()?;
        let mut local = ReviewItem::from_proto(item);
        if local.id.is_empty() {
            local.id = format!("review-{}", Uuid::new_v4().simple());
        }
        if let Some(existing) = file.items.iter_mut().find(|i| i.id == local.id) {
            *existing = local;
        } else {
            file.items.push(local);
        }
        self.save_state(&file)?;
        Ok(())
    }

    pub fn list_review(&self) -> Result<Vec<ProtoReviewItem>, WikiLiveError> {
        let file: ReviewFile = self.load_state()?;
        Ok(file.items.into_iter().map(ReviewItem::to_proto).collect())
    }

    /// Look up + return the action attached to one of an
    /// item's suggestions. Used by the backend to find the
    /// payload before applying.
    pub fn review_action(
        &self,
        item_id: &str,
        suggestion_label: &str,
    ) -> Result<ProtoReviewAction, WikiLiveError> {
        let file: ReviewFile = self.load_state()?;
        let item = file
            .items
            .iter()
            .find(|i| i.id == item_id)
            .ok_or_else(|| WikiLiveError::TaskNotFound(format!("review {item_id}")))?;
        let sug = item
            .suggestions
            .iter()
            .find(|s| s.label == suggestion_label)
            .ok_or_else(|| {
                WikiLiveError::TaskNotFound(format!(
                    "suggestion `{suggestion_label}` on review {item_id}"
                ))
            })?;
        Ok(sug.action.clone().to_proto())
    }

    pub fn mark_review_resolved(&self, item_id: &str) -> Result<(), WikiLiveError> {
        let mut file: ReviewFile = self.load_state()?;
        let item = file
            .items
            .iter_mut()
            .find(|i| i.id == item_id)
            .ok_or_else(|| WikiLiveError::TaskNotFound(format!("review {item_id}")))?;
        item.status = ReviewStatus::Resolved;
        self.save_state(&file)?;
        Ok(())
    }

    pub fn mark_review_dismissed(&self, item_id: &str) -> Result<(), WikiLiveError> {
        let mut file: ReviewFile = self.load_state()?;
        let item = file
            .items
            .iter_mut()
            .find(|i| i.id == item_id)
            .ok_or_else(|| WikiLiveError::TaskNotFound(format!("review {item_id}")))?;
        item.status = ReviewStatus::Dismissed;
        self.save_state(&file)?;
        Ok(())
    }
}
