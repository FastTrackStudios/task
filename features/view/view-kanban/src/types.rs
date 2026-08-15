//! Core data types for the kanban board.
//!
//! Mirrors janhesters/shadcn-kanban-board's `Card` / `Column` shape
//! but Rust-flavored: ids are `Uuid`, colors are an enum that maps to
//! architect-ui status/role tokens at render time, descriptions are
//! optional. Internal layout state stays out of these types — they're
//! the wire shape a consumer feeds in via props.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type CardId = Uuid;
pub type ColumnId = Uuid;

/// Column accent — rendered as a small circle next to the title and
/// (optionally) tinting the column rule. Names mirror architect-ui's
/// status-token vocabulary so the same enum can drive both kanban
/// chrome and other status surfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColorTag {
    #[default]
    Neutral,
    Primary,
    Success,
    Warning,
    Danger,
    Info,
}

impl ColorTag {
    /// Tailwind color stem (e.g. `"emerald"`). Used by the renderer
    /// to compose `bg-{stem}-500` / `text-{stem}-500` class names
    /// against the theme. Kept here so consumers can reuse it for
    /// chips, badges, etc.
    #[must_use]
    pub fn stem(self) -> &'static str {
        match self {
            Self::Neutral => "slate",
            Self::Primary => "violet",
            Self::Success => "emerald",
            Self::Warning => "amber",
            Self::Danger => "rose",
            Self::Info => "sky",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanCard {
    pub id: CardId,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl KanbanCard {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            description: None,
        }
    }
}

/// A column's identity + accent + ordered card-id list. Cards
/// themselves live on [`crate::store::KanbanState::cards`] keyed by
/// `CardId`; this keeps moves O(1) on the card payload.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanColumn {
    pub id: ColumnId,
    pub title: String,
    #[serde(default)]
    pub color: ColorTag,
    #[serde(default)]
    pub cards: Vec<CardId>,
}

impl KanbanColumn {
    pub fn new(title: impl Into<String>, color: ColorTag) -> Self {
        Self {
            id: Uuid::new_v4(),
            title: title.into(),
            color,
            cards: Vec::new(),
        }
    }
}
