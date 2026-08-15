//! Root Kanban component — owns drag context, lays columns out in a
//! horizontal flex row, forwards `KanbanEvent` upward, and offers a
//! trailing "+ Add column" affordance.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::Plus;

use crate::store::KanbanEvent;
use crate::types::{ColorTag, KanbanCard, KanbanColumn};

use super::column::ColumnView;
use super::drag::DragContext;

#[derive(Props, Clone, PartialEq)]
pub struct KanbanProps {
    pub columns: Vec<KanbanColumn>,
    /// Card payloads keyed in any order — columns pluck out their
    /// own by id. Accepts a `Vec` (not the `IndexMap` from
    /// `KanbanState`) so the prop stays trivially clonable and
    /// `PartialEq`.
    pub cards: Vec<KanbanCard>,
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<KanbanEvent>,
}

#[component]
pub fn Kanban(props: KanbanProps) -> Element {
    // Drag state lives at the root and is read by every card +
    // drop-slot in the tree.
    use_context_provider(|| DragContext {
        drag: Signal::new(None),
        hint: Signal::new(None),
    });

    let on_event = props.on_event;

    rsx! {
        div {
            class: "h-full w-full overflow-x-auto",
            div {
                class: "flex gap-3 h-full p-3 items-stretch min-w-max",
                for column in props.columns.iter() {
                    {
                        let column = column.clone();
                        let key = column.id.to_string();
                        let cards: Vec<KanbanCard> = column
                            .cards
                            .iter()
                            .filter_map(|id| props.cards.iter().find(|c| c.id == *id).cloned())
                            .collect();
                        rsx! {
                            ColumnView {
                                key: "{key}",
                                column: column,
                                cards: cards,
                                readonly: props.readonly,
                                on_event,
                            }
                        }
                    }
                }
                if !props.readonly {
                    AddColumnButton { on_event }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct AddColumnButtonProps {
    on_event: EventHandler<KanbanEvent>,
}

/// Trailing placeholder column. Click → emit `AddColumn` with a
/// default-titled neutral column. The user then double-clicks the
/// new column's title to rename.
#[component]
fn AddColumnButton(props: AddColumnButtonProps) -> Element {
    let on_event = props.on_event;
    rsx! {
        button {
            r#type: "button",
            class: "w-72 shrink-0 h-full rounded-lg border border-dashed border-border/60 \
                    text-muted-foreground hover:text-foreground hover:border-border \
                    hover:bg-card/40 transition-colors flex items-center justify-center gap-2 text-sm",
            onclick: move |_| {
                let column = KanbanColumn::new("New column", ColorTag::Neutral);
                on_event.call(KanbanEvent::AddColumn { column });
            },
            Plus { size: 16 }
            "Add column"
        }
    }
}
