//! Reactive store + event vocabulary.
//!
//! `KanbanState` is plain data. The root [`crate::components::Kanban`]
//! holds it in a `Signal<KanbanState>`. Mutations from inner
//! components bubble up as [`KanbanEvent`] through `on_event` — the
//! *consumer* decides whether to write through to a CRDT, debounce,
//! etc. (Per AGENTS.md: dumb components, data in, events out.)

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::types::{CardId, ColorTag, ColumnId, KanbanCard, KanbanColumn};

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KanbanState {
    pub columns: Vec<KanbanColumn>,
    /// Card payloads keyed by id. `IndexMap` preserves insertion
    /// order for stable debug/snapshot output; reorder lives on
    /// each column's `cards: Vec<CardId>`, not here.
    pub cards: IndexMap<CardId, KanbanCard>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KanbanEvent {
    /// Cross-column move OR intra-column reorder. `to_index` is the
    /// insertion index in the destination column's `cards` list,
    /// computed *after* the source has been removed from its origin.
    MoveCard {
        card: CardId,
        from_column: ColumnId,
        to_column: ColumnId,
        to_index: usize,
    },
    AddCard {
        column: ColumnId,
        card: KanbanCard,
    },
    RemoveCard {
        card: CardId,
    },
    RenameCard {
        card: CardId,
        title: String,
    },

    AddColumn {
        column: KanbanColumn,
    },
    RemoveColumn {
        column: ColumnId,
    },
    RenameColumn {
        column: ColumnId,
        title: String,
    },
    RecolorColumn {
        column: ColumnId,
        color: ColorTag,
    },
}

/// In-place reducer matching the event vocabulary. Used by the demo
/// route in task-ui as a drop-in stand-in for a CRDT-backed wrapper.
/// Invalid ids are silently ignored — the consumer is expected to
/// have filtered them at the wire boundary.
pub fn apply(state: &mut KanbanState, ev: &KanbanEvent) {
    match ev {
        KanbanEvent::MoveCard {
            card,
            from_column,
            to_column,
            to_index,
        } => {
            if let Some(src) = state.columns.iter_mut().find(|c| c.id == *from_column) {
                if let Some(pos) = src.cards.iter().position(|id| id == card) {
                    src.cards.remove(pos);
                }
            }
            if let Some(dst) = state.columns.iter_mut().find(|c| c.id == *to_column) {
                let idx = (*to_index).min(dst.cards.len());
                dst.cards.insert(idx, *card);
            }
        }
        KanbanEvent::AddCard { column, card } => {
            let id = card.id;
            state.cards.insert(id, card.clone());
            if let Some(col) = state.columns.iter_mut().find(|c| c.id == *column) {
                col.cards.push(id);
            }
        }
        KanbanEvent::RemoveCard { card } => {
            state.cards.shift_remove(card);
            for col in &mut state.columns {
                col.cards.retain(|id| id != card);
            }
        }
        KanbanEvent::RenameCard { card, title } => {
            if let Some(c) = state.cards.get_mut(card) {
                c.title = title.clone();
            }
        }
        KanbanEvent::AddColumn { column } => {
            state.columns.push(column.clone());
        }
        KanbanEvent::RemoveColumn { column } => {
            if let Some(pos) = state.columns.iter().position(|c| c.id == *column) {
                let col = state.columns.remove(pos);
                for id in col.cards {
                    state.cards.shift_remove(&id);
                }
            }
        }
        KanbanEvent::RenameColumn { column, title } => {
            if let Some(c) = state.columns.iter_mut().find(|c| c.id == *column) {
                c.title = title.clone();
            }
        }
        KanbanEvent::RecolorColumn { column, color } => {
            if let Some(c) = state.columns.iter_mut().find(|c| c.id == *column) {
                c.color = *color;
            }
        }
    }
}
