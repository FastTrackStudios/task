//! `BaseView` — executed Obsidian Base views, server-rendered for the
//! in-app `/bases` table renderer.
//!
//! A `.base` file's views are run server-side (the query engine is
//! native — it can't run on `wasm32`), and the result is projected to
//! this slim, wasm-clean shape: column headers + grouped rows, each row
//! a vault page reduced to the view's columns. The client just paints a
//! table and links each row back to its `path`.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// One row of an executed view — a vault page projected to the view's
/// columns.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct BaseRowView {
    /// Vault-relative `.md` path — the open/navigate target.
    pub path: String,
    /// Filename without extension.
    pub basename: String,
    /// Frontmatter `title`, else the basename.
    pub title: String,
    /// Cell values aligned 1:1 with [`BaseView::columns`].
    pub cells: Vec<String>,
}

/// A group of rows under a `groupBy` label (`""` when the view isn't
/// grouped).
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct BaseGroup {
    pub label: String,
    pub rows: Vec<BaseRowView>,
}

/// One executed view of a base — its name, kind, column headers, and
/// grouped rows.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct BaseView {
    pub name: String,
    /// Canonical view kind (`table` / `board` / …).
    pub view_type: String,
    pub columns: Vec<String>,
    pub groups: Vec<BaseGroup>,
}

#[cfg(feature = "vox")]
#[allow(unsafe_code)]
mod reborrow_impls {
    use super::{BaseGroup, BaseRowView, BaseView};
    unsafe impl vox_types::Reborrow for BaseRowView {
        type Ref<'a> = BaseRowView;
    }
    unsafe impl vox_types::Reborrow for BaseGroup {
        type Ref<'a> = BaseGroup;
    }
    unsafe impl vox_types::Reborrow for BaseView {
        type Ref<'a> = BaseView;
    }
}
