//! Per-type cell renderer + inline editor.

use chrono::NaiveDate;
use dioxus::prelude::*;
use architect_ui::prelude::*;

use crate::store::TableMutation;
use crate::types::{CellValue, Column, ColumnId, ColumnType, RowId, SelectOption};

#[derive(Props, Clone, PartialEq)]
pub struct CellViewProps {
    pub row: RowId,
    pub column: Column,
    pub value: CellValue,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<TableMutation>,
}

#[component]
pub fn CellView(props: CellViewProps) -> Element {
    let mut editing = use_signal(|| false);
    let col_id = props.column.id;
    let row_id = props.row;
    let on_event = props.on_event;
    let value = props.value.clone();
    let column = props.column.clone();

    // Display vs. edit branch. `editing` is set on double-click for
    // text/number/date; for checkbox, the click directly commits.
    if *editing.read() && !props.readonly {
        return rsx! {
            CellEditor {
                row: row_id,
                column: column.clone(),
                initial: value,
                on_commit: move |v: CellValue| {
                    on_event.call(TableMutation::SetCell { row: row_id, column: col_id, value: v });
                    editing.set(false);
                },
                on_cancel: move |()| editing.set(false),
            }
        };
    }

    let cell_class = "px-2 py-1 truncate text-sm";
    rsx! {
        div {
            class: "{cell_class}",
            ondoubleclick: move |_| {
                if !props.readonly && !matches!(column.ty, ColumnType::Checkbox) {
                    editing.set(true);
                }
            },
            {render_display(&value, &column, row_id, col_id, on_event, props.readonly)}
        }
    }
}

fn render_display(
    value: &CellValue,
    column: &Column,
    row_id: RowId,
    col_id: ColumnId,
    on_event: EventHandler<TableMutation>,
    readonly: bool,
) -> Element {
    match (value, column.ty) {
        (CellValue::Text(s), _) => rsx! { span { class: "text-foreground", "{s}" } },
        (CellValue::Number(n), _) => rsx! { span { class: "text-foreground tabular-nums", "{n}" } },
        (CellValue::Date(d), _) => rsx! {
            span { class: "text-foreground tabular-nums",
                "{d.format(\"%Y-%m-%d\")}"
            }
        },
        (CellValue::Select(v), _) => {
            let opt = column.options.iter().find(|o| o.value == *v);
            let stem = opt.and_then(|o| o.color.as_deref()).unwrap_or("slate");
            let chip = format!(
                "inline-flex items-center px-2 py-0.5 rounded-full text-[11px] bg-{stem}-500/20 text-{stem}-200 border border-{stem}-500/30"
            );
            rsx! { span { class: "{chip}", "{v}" } }
        }
        (CellValue::Checkbox(b), _) => rsx! {
            input {
                r#type: "checkbox",
                checked: *b,
                disabled: readonly,
                onchange: move |e: FormEvent| {
                    on_event.call(TableMutation::SetCell {
                        row: row_id,
                        column: col_id,
                        value: CellValue::Checkbox(e.value() == "true"),
                    });
                },
            }
        },
        (CellValue::Empty, ColumnType::Checkbox) => rsx! {
            input {
                r#type: "checkbox",
                checked: false,
                disabled: readonly,
                onchange: move |e: FormEvent| {
                    on_event.call(TableMutation::SetCell {
                        row: row_id,
                        column: col_id,
                        value: CellValue::Checkbox(e.value() == "true"),
                    });
                },
            }
        },
        (CellValue::Empty, _) => rsx! { span { class: "text-muted-foreground/60 italic", "—" } },
    }
}

#[derive(Props, Clone, PartialEq)]
struct CellEditorProps {
    row: RowId,
    column: Column,
    initial: CellValue,
    on_commit: EventHandler<CellValue>,
    on_cancel: EventHandler<()>,
}

#[component]
fn CellEditor(props: CellEditorProps) -> Element {
    let _ = props.row; // currently only the commit value matters

    let initial = props.initial.clone();
    let column = props.column.clone();
    let on_commit = props.on_commit;
    let on_cancel = props.on_cancel;

    match column.ty {
        ColumnType::Text => {
            let mut draft = use_signal(|| match &initial {
                CellValue::Text(s) => s.clone(),
                _ => String::new(),
            });
            rsx! {
                input {
                    class: "w-full px-2 py-1 text-sm bg-background border border-ring outline-none",
                    value: "{draft}",
                    "autofocus": true,
                    oninput: move |e: FormEvent| draft.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter {
                            on_commit.call(CellValue::Text(draft()));
                        } else if e.key() == Key::Escape {
                            on_cancel.call(());
                        }
                    },
                    onfocusout: move |_| on_commit.call(CellValue::Text(draft())),
                }
            }
        }
        ColumnType::Number => {
            let mut draft = use_signal(|| match &initial {
                CellValue::Number(n) => format!("{n}"),
                _ => String::new(),
            });
            rsx! {
                input {
                    r#type: "number",
                    class: "w-full px-2 py-1 text-sm bg-background border border-ring outline-none tabular-nums",
                    value: "{draft}",
                    "autofocus": true,
                    oninput: move |e: FormEvent| draft.set(e.value()),
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Enter {
                            let v = draft().parse::<f64>().ok();
                            on_commit.call(v.map_or(CellValue::Empty, CellValue::Number));
                        } else if e.key() == Key::Escape {
                            on_cancel.call(());
                        }
                    },
                    onfocusout: move |_| {
                        let v = draft().parse::<f64>().ok();
                        on_commit.call(v.map_or(CellValue::Empty, CellValue::Number));
                    },
                }
            }
        }
        ColumnType::Date => {
            let init_str = match &initial {
                CellValue::Date(d) => d.format("%Y-%m-%d").to_string(),
                _ => String::new(),
            };
            rsx! {
                input {
                    r#type: "date",
                    class: "w-full px-2 py-1 text-sm bg-background border border-ring outline-none",
                    value: "{init_str}",
                    "autofocus": true,
                    onchange: move |e: FormEvent| {
                        let parsed = NaiveDate::parse_from_str(&e.value(), "%Y-%m-%d").ok();
                        on_commit.call(parsed.map_or(CellValue::Empty, CellValue::Date));
                    },
                    onkeydown: move |e: KeyboardEvent| {
                        if e.key() == Key::Escape {
                            on_cancel.call(());
                        }
                    },
                }
            }
        }
        ColumnType::Select => rsx! {
            SelectMenu {
                options: column.options.clone(),
                current: match &initial { CellValue::Select(s) => Some(s.clone()), _ => None },
                on_pick: move |v: String| on_commit.call(CellValue::Select(v)),
                on_cancel,
            }
        },
        ColumnType::Checkbox => {
            // Checkbox edits commit directly from the display
            // renderer — the editor branch is unreachable, but we
            // return a friendly fallback rather than panicking.
            on_cancel.call(());
            rsx! {}
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct SelectMenuProps {
    options: Vec<SelectOption>,
    current: Option<String>,
    on_pick: EventHandler<String>,
    on_cancel: EventHandler<()>,
}

#[component]
fn SelectMenu(props: SelectMenuProps) -> Element {
    rsx! {
        Dropdown {
            default_open: true,
            on_open_change: move |open: bool| if !open { props.on_cancel.call(()) },
            DropdownTrigger {
                class: "px-2 py-1 text-sm w-full text-left border border-ring",
                "{props.current.clone().unwrap_or_else(|| String::from(\"—\"))}"
            }
            DropdownContent {
                for (idx, opt) in props.options.iter().enumerate() {
                    {
                        let v = opt.value.clone();
                        let on_pick = props.on_pick;
                        rsx! {
                            DropdownItem {
                                key: "{v}",
                                value: v.clone(),
                                index: idx,
                                on_select: move |_| on_pick.call(v.clone()),
                                "{opt.value}"
                            }
                        }
                    }
                }
            }
        }
    }
}
