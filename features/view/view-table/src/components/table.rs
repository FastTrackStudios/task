//! Root Table component — composes header / filter row / body
//! into a single CSS grid that keeps columns aligned.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{ChevronDown, ChevronRight, Plus};
use architect_ui::prelude::*;

use crate::layout::{DisplayRow, layout};
use crate::store::{TableMutation, TableState};
use crate::types::{Column, Row};

use super::cell::CellView;
use super::column_menu::ColumnMenu;
use super::header::{FilterRow, HeaderRow};

#[derive(Props, Clone, PartialEq)]
pub struct TableProps {
    pub state: TableState,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<TableMutation>,
}

#[component]
pub fn Table(props: TableProps) -> Element {
    let state = props.state;
    let on_event = props.on_event;

    let visible_cols: Vec<Column> = state
        .columns
        .iter()
        .filter(|c| !c.hidden)
        .cloned()
        .collect();
    let display = layout(&state);
    let filters_vec: Vec<_> = state.filters.iter().map(|(k, v)| (*k, v.clone())).collect();
    let grid_template = grid_template(&visible_cols);

    rsx! {
        div { class: "flex flex-col h-full w-full",
            // Toolbar
            div { class: "flex items-center gap-2 px-2 py-1.5 border-b border-border/40",
                Text { variant: TextVariant::Muted, class: "text-xs",
                    "{state.rows.len()} rows"
                }
                Spacer {}
                ColumnMenu {
                    columns: state.columns.clone(),
                    group_by: state.group_by,
                    on_event,
                }
                if !props.readonly {
                    Button {
                        size: ButtonSize::Small,
                        on_click: move |_| on_event.call(TableMutation::AddRow { row: Row::new() }),
                        Plus { size: 14 }
                        "Add row"
                    }
                }
            }
            // Scrollable grid
            div { class: "flex-1 min-h-0 overflow-auto",
                div {
                    class: "grid",
                    style: "grid-template-columns: {grid_template};",
                    HeaderRow {
                        columns: visible_cols.clone(),
                        sort: state.sort,
                        on_event,
                    }
                    FilterRow {
                        columns: visible_cols.clone(),
                        filters: filters_vec,
                        on_event,
                    }
                    for entry in display.iter() {
                        match entry {
                            DisplayRow::GroupHeader { key, label, count, collapsed } => {
                                {
                                    let key = key.clone();
                                    let label = label.clone();
                                    let count = *count;
                                    let collapsed = *collapsed;
                                    let cols = visible_cols.len();
                                    rsx! {
                                        div {
                                            key: "g-{key}",
                                            class: "px-2 py-1 bg-card/60 border-b border-border/40 flex items-center gap-2 cursor-pointer hover:bg-card",
                                            style: "grid-column: span {cols};",
                                            onclick: move |_| {
                                                on_event.call(TableMutation::ToggleGroupCollapsed { key: key.clone() });
                                            },
                                            if collapsed {
                                                ChevronRight { size: 14 }
                                            } else {
                                                ChevronDown { size: 14 }
                                            }
                                            span { class: "text-xs font-medium", "{label}" }
                                            Text { variant: TextVariant::Muted, class: "text-[10px]", "{count}" }
                                        }
                                    }
                                }
                            }
                            DisplayRow::Row(r) => {
                                {
                                    let r = r.clone();
                                    let cols = visible_cols.clone();
                                    rsx! {
                                        BodyRow {
                                            key: "{r.id}",
                                            row: r,
                                            columns: cols,
                                            readonly: props.readonly,
                                            on_event,
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct BodyRowProps {
    row: Row,
    columns: Vec<Column>,
    readonly: bool,
    on_event: EventHandler<TableMutation>,
}

/// One data row — each cell is a grid item; `display: contents`
/// lets the row's `key` apply without breaking the grid layout.
#[component]
fn BodyRow(props: BodyRowProps) -> Element {
    let row_id = props.row.id;
    rsx! {
        div { class: "contents",
            for column in props.columns.iter() {
                {
                    let column = column.clone();
                    let col_id = column.id;
                    let value = props
                        .row
                        .cells
                        .get(&col_id)
                        .cloned()
                        .unwrap_or(crate::types::CellValue::Empty);
                    let on_event = props.on_event;
                    rsx! {
                        div {
                            key: "{col_id}",
                            class: "border-b border-r border-border/30 min-w-0",
                            CellView {
                                row: row_id,
                                column,
                                value,
                                readonly: props.readonly,
                                on_event,
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Build a CSS `grid-template-columns` string from the visible
/// columns' widths.
fn grid_template(columns: &[Column]) -> String {
    columns
        .iter()
        .map(|c| format!("{}px", c.width_px))
        .collect::<Vec<_>>()
        .join(" ")
}
