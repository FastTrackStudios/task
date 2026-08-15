//! Column-visibility + group-by menu, anchored to a toolbar button.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::Columns3;
use architect_ui::prelude::*;

use crate::store::TableMutation;
use crate::types::{Column, ColumnId};

#[derive(Props, Clone, PartialEq)]
pub struct ColumnMenuProps {
    pub columns: Vec<Column>,
    pub group_by: Option<ColumnId>,
    pub on_event: EventHandler<TableMutation>,
}

#[component]
pub fn ColumnMenu(props: ColumnMenuProps) -> Element {
    let on_event = props.on_event;
    rsx! {
        Dropdown {
            DropdownTrigger {
                class: "inline-flex items-center gap-1 px-2 py-1 text-xs rounded border border-border/60 hover:bg-accent",
                Columns3 { size: 14 }
                "Columns"
            }
            DropdownContent {
                div { class: "px-2 py-1 text-[10px] uppercase tracking-wide text-muted-foreground", "Visibility" }
                for (idx, column) in props.columns.iter().enumerate() {
                    {
                        let id = column.id;
                        let label = column.label.clone();
                        let hidden = column.hidden;
                        rsx! {
                            DropdownItem {
                                key: "vis-{id}",
                                value: id.to_string(),
                                index: idx,
                                on_select: move |_| {
                                    on_event.call(TableMutation::SetColumnHidden { id, hidden: !hidden });
                                },
                                input { r#type: "checkbox", checked: !hidden, class: "mr-2" }
                                "{label}"
                            }
                        }
                    }
                }
                DropdownSeparator {}
                div { class: "px-2 py-1 text-[10px] uppercase tracking-wide text-muted-foreground", "Group by" }
                {
                    let group_by = props.group_by;
                    let none_idx = props.columns.len();
                    rsx! {
                        DropdownItem {
                            key: "{none_idx}-none",
                            value: "none".to_string(),
                            index: none_idx,
                            on_select: move |_| on_event.call(TableMutation::SetGroupBy { column: None }),
                            input { r#type: "radio", checked: group_by.is_none(), class: "mr-2" }
                            "None"
                        }
                        for (idx, column) in props.columns.iter().enumerate() {
                            {
                                let id = column.id;
                                let label = column.label.clone();
                                let active = group_by == Some(id);
                                rsx! {
                                    DropdownItem {
                                        key: "group-{id}",
                                        value: id.to_string(),
                                        index: none_idx + 1 + idx,
                                        on_select: move |_| on_event.call(TableMutation::SetGroupBy { column: Some(id) }),
                                        input { r#type: "radio", checked: active, class: "mr-2" }
                                        "{label}"
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
