//! Column, row, cell value, and view-state types.
//!
//! The cell value is a small tagged enum — not a `serde_json::Value`
//! — so the renderer can dispatch on type without re-parsing every
//! cell, and so the editor doesn't have to guess what UI to show.

use chrono::NaiveDate;
use indexmap::IndexMap;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub type ColumnId = Uuid;
pub type RowId = Uuid;

/// What kind of data a column holds. Drives the cell renderer + the
/// inline editor and constrains the filter input.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ColumnType {
    #[default]
    Text,
    Number,
    Date,
    Select,
    Checkbox,
}

/// One choice in a `Select` column. Order is meaningful for
/// the dropdown.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SelectOption {
    pub value: String,
    /// Optional tailwind color stem ("emerald", "amber", …) for a
    /// chip background. None = default neutral chip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Column {
    pub id: ColumnId,
    pub label: String,
    pub ty: ColumnType,
    /// Pixel width. The default depends on `ty`; the table writes
    /// this when the user drags a column edge.
    #[serde(default = "default_width")]
    pub width_px: u16,
    #[serde(default)]
    pub hidden: bool,
    /// Pre-defined choices for `ColumnType::Select`. Ignored for
    /// other types.
    #[serde(default)]
    pub options: Vec<SelectOption>,
}

fn default_width() -> u16 {
    160
}

impl Column {
    pub fn new(label: impl Into<String>, ty: ColumnType) -> Self {
        Self {
            id: Uuid::new_v4(),
            label: label.into(),
            ty,
            width_px: default_width(),
            hidden: false,
            options: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "ty", content = "v", rename_all = "snake_case")]
pub enum CellValue {
    Text(String),
    Number(f64),
    Date(NaiveDate),
    /// Stores the *option value* (not the label). The column carries
    /// the option list; the cell just holds the chosen key.
    Select(String),
    Checkbox(bool),
    /// Render-only sentinel for missing data. Editors interpret it
    /// as "no value yet" and emit a typed `CellValue` on first
    /// commit.
    Empty,
}

impl CellValue {
    /// Best-effort string for sort + filter. Date/number/select all
    /// flatten to a canonical string so the layout pass can stay
    /// type-agnostic.
    #[must_use]
    pub fn as_sort_key(&self) -> String {
        match self {
            Self::Text(s) | Self::Select(s) => s.to_ascii_lowercase(),
            Self::Number(n) => format!("{n:020.6}"),
            Self::Date(d) => d.format("%Y-%m-%d").to_string(),
            Self::Checkbox(b) => if *b { "1" } else { "0" }.into(),
            Self::Empty => String::new(),
        }
    }

    /// Lowercase substring source for the per-column filter.
    #[must_use]
    pub fn as_filter_str(&self) -> String {
        match self {
            Self::Text(s) | Self::Select(s) => s.to_ascii_lowercase(),
            Self::Number(n) => format!("{n}"),
            Self::Date(d) => d.format("%Y-%m-%d").to_string(),
            Self::Checkbox(b) => if *b { "true" } else { "false" }.into(),
            Self::Empty => String::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    pub id: RowId,
    /// Cell values keyed by column id. Columns missing from the
    /// map render as `CellValue::Empty`.
    #[serde(default)]
    pub cells: IndexMap<ColumnId, CellValue>,
}

impl Row {
    #[must_use]
    pub fn new() -> Self {
        Self {
            id: Uuid::new_v4(),
            cells: IndexMap::new(),
        }
    }

    #[must_use]
    pub fn with(mut self, col: ColumnId, value: CellValue) -> Self {
        self.cells.insert(col, value);
        self
    }
}

impl Default for Row {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

impl SortDir {
    #[must_use]
    pub fn toggle(self) -> Self {
        match self {
            Self::Asc => Self::Desc,
            Self::Desc => Self::Asc,
        }
    }
}
