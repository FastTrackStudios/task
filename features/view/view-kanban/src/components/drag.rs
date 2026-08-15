//! Drag state shared between columns and cards.
//!
//! Lives at the root [`super::kanban::Kanban`] and is provided via
//! context so children can mark themselves draggable + light up drop
//! targets without prop-drilling. HTML5 native drag carries the
//! card id in `DataTransfer`, but signal state is the source of
//! truth for visual feedback (`DataTransfer` is opaque mid-drag in
//! most browsers).

use dioxus::prelude::*;

use crate::types::{CardId, ColumnId};

/// What the user is currently dragging.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DragState {
    pub card: CardId,
    pub from_column: ColumnId,
}

/// Where the dragged card would land if the user released now.
/// `index` is the insertion slot — `0` = before the first card,
/// `column.cards.len()` = at the end.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DropHint {
    pub column: ColumnId,
    pub index: usize,
}

#[derive(Clone, Copy)]
pub struct DragContext {
    pub drag: Signal<Option<DragState>>,
    pub hint: Signal<Option<DropHint>>,
}

pub fn use_drag_context() -> DragContext {
    use_context::<DragContext>()
}
