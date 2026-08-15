//! One kanban column — header (color circle + inline-rename title +
//! count + menu) and a vertical list of [`super::card::CardTile`]s
//! separated by [`DropSlot`] gaps.
//!
//! Menu: color swatches at the top, then Delete. Rename is
//! double-click on the title (mirrors janhesters/shadcn-kanban-board
//! UX — no rename menu item, no extra modal).

use architect_ui::lucide_dioxus::{Ellipsis, Trash2};
use architect_ui::prelude::*;
use dioxus::prelude::*;
use uuid::Uuid;

use crate::store::KanbanEvent;
use crate::types::{ColorTag, ColumnId, KanbanCard, KanbanColumn};

use super::card::{CardTile, dt_mime};
use super::drag::{DropHint, use_drag_context};

const COLOR_OPTIONS: &[ColorTag] = &[
    ColorTag::Neutral,
    ColorTag::Primary,
    ColorTag::Success,
    ColorTag::Warning,
    ColorTag::Danger,
    ColorTag::Info,
];

#[derive(Props, Clone, PartialEq)]
pub struct ColumnViewProps {
    pub column: KanbanColumn,
    /// Card payloads in this column's `cards` order. Resolved by the
    /// root [`super::kanban::Kanban`] so the column stays free of
    /// the global lookup map.
    pub cards: Vec<KanbanCard>,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<KanbanEvent>,
}

#[component]
pub fn ColumnView(props: ColumnViewProps) -> Element {
    let column_id: ColumnId = props.column.id;
    let card_count = props.cards.len();
    let cards = props.cards.clone();
    let on_event = props.on_event;
    let current_color = props.column.color;
    let current_title = props.column.title.clone();

    let dot_class = format!(
        "inline-block w-2.5 h-2.5 rounded-full bg-{}-500",
        current_color.stem()
    );

    rsx! {
        div {
            class: "flex flex-col w-72 shrink-0 h-full bg-card border border-border/60 rounded-lg",
            // Header
            div { class: "flex items-center gap-2 px-3 py-2 border-b border-border/40",
                span { class: "{dot_class} shrink-0" }
                div { class: "flex-1 min-w-0",
                    InlineEdit {
                        value: current_title,
                        class: "text-sm font-semibold w-full text-left",
                        on_commit: move |title: String| {
                            on_event.call(KanbanEvent::RenameColumn { column: column_id, title });
                        },
                    }
                }
                Text { variant: TextVariant::Muted, class: "text-xs shrink-0", "{card_count}" }
                if !props.readonly {
                    ColumnMenu {
                        column_id,
                        current_color,
                        on_event,
                    }
                }
            }
            // Card stack — each slot before / between / after cards
            // becomes a drop target.
            div { class: "flex-1 min-h-0 overflow-y-auto p-2 flex flex-col gap-2",
                DropSlot { column: column_id, index: 0, on_event }
                for (i, card) in cards.iter().enumerate() {
                    {
                        let key = card.id.to_string();
                        rsx! {
                            div { key: "{key}",
                                CardTile {
                                    card: card.clone(),
                                    column: column_id,
                                    readonly: props.readonly,
                                    on_event,
                                }
                                DropSlot { column: column_id, index: i + 1, on_event }
                            }
                        }
                    }
                }
            }
            // Footer
            if !props.readonly {
                div { class: "p-2 border-t border-border/40",
                    Button {
                        class: "w-full justify-start text-muted-foreground",
                        variant: ButtonVariant::Ghost,
                        size: ButtonSize::Small,
                        on_click: move |_| {
                            let card = KanbanCard::new("New card");
                            on_event.call(KanbanEvent::AddCard { column: column_id, card });
                        },
                        "+ Add card"
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ColumnMenuProps {
    column_id: ColumnId,
    current_color: ColorTag,
    on_event: EventHandler<KanbanEvent>,
}

/// Three-dot menu on the column header. Color row (swatches act as
/// items), separator, destructive Delete.
#[component]
fn ColumnMenu(props: ColumnMenuProps) -> Element {
    let column_id = props.column_id;
    let on_event = props.on_event;
    let current = props.current_color;

    rsx! {
        Dropdown {
            DropdownTrigger {
                class: "p-1 rounded hover:bg-accent text-muted-foreground",
                Ellipsis { size: 14 }
            }
            DropdownContent {
                // Color row — each swatch is a dropdown item with a
                // ring on the currently-active color.
                div { class: "flex items-center gap-1 px-2 py-1.5",
                    for (i, color) in COLOR_OPTIONS.iter().enumerate() {
                        {
                            let c = *color;
                            let stem = c.stem();
                            let ring = if c == current { "ring-2 ring-foreground/60" } else { "" };
                            let key = format!("swatch-{stem}");
                            rsx! {
                                button {
                                    key: "{key}",
                                    r#type: "button",
                                    class: "w-5 h-5 rounded-full bg-{stem}-500 {ring} transition-shadow",
                                    title: "{stem}",
                                    onclick: move |_| {
                                        on_event.call(KanbanEvent::RecolorColumn { column: column_id, color: c });
                                    },
                                }
                                // suppress unused-var warning for `i`
                                {let _ = i; rsx!{}}
                            }
                        }
                    }
                }
                DropdownSeparator {}
                DropdownItem {
                    value: "delete".to_string(),
                    index: 0,
                    destructive: true,
                    icon: rsx! { Trash2 { size: 14 } },
                    on_select: move |_| {
                        on_event.call(KanbanEvent::RemoveColumn { column: column_id });
                    },
                    "Delete column"
                }
            }
        }
    }
}

/// Invisible insertion zone between cards. Highlights when the drag
/// hint points at this `(column, index)` slot, and emits the
/// [`KanbanEvent::MoveCard`] on drop.
#[derive(Props, Clone, PartialEq)]
struct DropSlotProps {
    column: ColumnId,
    index: usize,
    on_event: EventHandler<KanbanEvent>,
}

#[component]
fn DropSlot(props: DropSlotProps) -> Element {
    let ctx = use_drag_context();
    let mut hint = ctx.hint;
    let drag_snapshot = *ctx.drag.read();
    let active = hint.read().as_ref()
        == Some(&DropHint {
            column: props.column,
            index: props.index,
        });
    let dragging = drag_snapshot.is_some();
    let column = props.column;
    let index = props.index;
    let on_event = props.on_event;

    let h = if dragging { "h-2" } else { "h-0" };
    let bg = if active { "bg-primary/50 rounded" } else { "" };

    rsx! {
        div {
            class: "{h} {bg} transition-colors",
            ondragover: move |e: Event<DragData>| {
                if !dragging { return; }
                e.prevent_default();
                let next = DropHint { column, index };
                if hint.peek().as_ref() != Some(&next) {
                    hint.set(Some(next));
                }
            },
            ondrop: move |e: Event<DragData>| {
                e.prevent_default();
                let dt = e.data().data_transfer();
                let raw = dt.get_data(dt_mime()).unwrap_or_default();
                let Ok(card_id) = raw.parse::<Uuid>() else { return };
                let Some(from) = drag_snapshot.map(|d| d.from_column) else { return };
                on_event.call(KanbanEvent::MoveCard {
                    card: card_id,
                    from_column: from,
                    to_column: column,
                    to_index: index,
                });
                hint.set(None);
            },
        }
    }
}
