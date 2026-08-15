//! Table — Dioxus port of an Airtable / Notion / Bases-style data
//! grid. Use it as the "list view" for any collection of typed
//! rows: tasks, contacts, meetings, vault notes, etc.
//!
//! # Shape
//!
//! - [`types::Column`] describes a column: id, label, type, width,
//!   visibility. A row is keyed by column id → [`types::CellValue`].
//! - [`types::Row`] = `{ id, cells: IndexMap<ColumnId, CellValue> }`.
//! - State is a flat list of rows + a snapshot of view state (sort,
//!   filters, group-by, column widths, hidden columns).
//!
//! # Features (v1)
//!
//! - Sort by column (asc / desc / unsorted), one column at a time.
//! - Per-column filter input.
//! - Group-by any column with collapsible group headers.
//! - Inline cell editing — text / number / date / select / checkbox.
//! - Resizable + hideable columns (state owned by the table).
//!
//! Wired against a hand-rolled `Signal<TableState>` in the demo
//! route. Real consumers swap in a CRDT-backed wrapper.

pub mod components;
pub mod layout;
pub mod store;
pub mod types;

pub use components::{Table, TableProps};
pub use store::{TableMutation, TableState};
pub use types::{CellValue, Column, ColumnId, ColumnType, Row, RowId, SelectOption, SortDir};
