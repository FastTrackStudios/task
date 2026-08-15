//! State + mutation vocabulary.
//!
//! `TableState` holds the rows AND the local view state (sort,
//! filters, group-by, column widths/visibility). For a real CRDT
//! integration only the row mutations need to be persisted — view
//! state is per-user, per-session.

use indexmap::IndexMap;
use serde::{Deserialize, Serialize};

use crate::types::{CellValue, Column, ColumnId, Row, RowId, SortDir};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TableState {
    pub columns: Vec<Column>,
    pub rows: Vec<Row>,
    /// `(column, dir)` for the active sort. `None` = original order.
    #[serde(default)]
    pub sort: Option<(ColumnId, SortDir)>,
    /// Per-column lowercase filter substring. Missing keys = no
    /// filter on that column.
    #[serde(default)]
    pub filters: IndexMap<ColumnId, String>,
    /// Optional group-by column. Group order follows sorted value.
    #[serde(default)]
    pub group_by: Option<ColumnId>,
    /// Collapsed group keys (lowercase string form of the cell's
    /// group value) so a parent UI can persist them across
    /// re-renders without owning the table's render state.
    #[serde(default)]
    pub collapsed_groups: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TableMutation {
    // Row CRUD
    AddRow {
        row: Row,
    },
    RemoveRow {
        id: RowId,
    },
    SetCell {
        row: RowId,
        column: ColumnId,
        value: CellValue,
    },
    // Column CRUD
    AddColumn {
        column: Column,
    },
    RemoveColumn {
        id: ColumnId,
    },
    RenameColumn {
        id: ColumnId,
        label: String,
    },
    SetColumnWidth {
        id: ColumnId,
        width_px: u16,
    },
    SetColumnHidden {
        id: ColumnId,
        hidden: bool,
    },
    // View state
    SetSort {
        sort: Option<(ColumnId, SortDir)>,
    },
    SetFilter {
        column: ColumnId,
        query: String,
    },
    SetGroupBy {
        column: Option<ColumnId>,
    },
    ToggleGroupCollapsed {
        key: String,
    },
}

pub fn apply(state: &mut TableState, mu: &TableMutation) {
    match mu {
        TableMutation::AddRow { row } => state.rows.push(row.clone()),
        TableMutation::RemoveRow { id } => state.rows.retain(|r| r.id != *id),
        TableMutation::SetCell { row, column, value } => {
            if let Some(r) = state.rows.iter_mut().find(|r| r.id == *row) {
                r.cells.insert(*column, value.clone());
            }
        }
        TableMutation::AddColumn { column } => state.columns.push(column.clone()),
        TableMutation::RemoveColumn { id } => {
            state.columns.retain(|c| c.id != *id);
            for r in &mut state.rows {
                r.cells.shift_remove(id);
            }
        }
        TableMutation::RenameColumn { id, label } => {
            if let Some(c) = state.columns.iter_mut().find(|c| c.id == *id) {
                c.label = label.clone();
            }
        }
        TableMutation::SetColumnWidth { id, width_px } => {
            if let Some(c) = state.columns.iter_mut().find(|c| c.id == *id) {
                c.width_px = (*width_px).max(60);
            }
        }
        TableMutation::SetColumnHidden { id, hidden } => {
            if let Some(c) = state.columns.iter_mut().find(|c| c.id == *id) {
                c.hidden = *hidden;
            }
        }
        TableMutation::SetSort { sort } => state.sort = *sort,
        TableMutation::SetFilter { column, query } => {
            let q = query.trim().to_ascii_lowercase();
            if q.is_empty() {
                state.filters.shift_remove(column);
            } else {
                state.filters.insert(*column, q);
            }
        }
        TableMutation::SetGroupBy { column } => state.group_by = *column,
        TableMutation::ToggleGroupCollapsed { key } => {
            if let Some(pos) = state.collapsed_groups.iter().position(|k| k == key) {
                state.collapsed_groups.swap_remove(pos);
            } else {
                state.collapsed_groups.push(key.clone());
            }
        }
    }
}
