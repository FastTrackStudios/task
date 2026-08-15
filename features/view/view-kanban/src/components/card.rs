//! Draggable card tile. Dumb: title + optional description.
//!
//! Title is double-click-to-rename via architect-ui [`InlineEdit`].
//! While the input is focused, drag is suppressed so the user can
//! select text inside the input.

use dioxus::prelude::*;
use architect_ui::prelude::*;

use crate::store::KanbanEvent;
use crate::types::{CardId, ColumnId, KanbanCard};

use super::drag::{DragState, use_drag_context};

const DT_MIME: &str = "text/x-kanban-card-id";

#[derive(Props, Clone, PartialEq)]
pub struct CardTileProps {
    pub card: KanbanCard,
    pub column: ColumnId,
    /// Read-only mode disables drag + edit affordances.
    #[props(default = false)]
    pub readonly: bool,
    pub on_event: EventHandler<KanbanEvent>,
}

#[component]
pub fn CardTile(props: CardTileProps) -> Element {
    let ctx = use_drag_context();
    let mut drag = ctx.drag;
    let card_id: CardId = props.card.id;
    let from_column = props.column;
    let is_dragging = drag.read().is_some_and(|d| d.card == card_id);

    // Drag must be off while the inline-edit input is focused —
    // otherwise text selection inside the input starts a card drag
    // instead of selecting characters.
    let mut editing = use_signal(|| false);
    let draggable = !props.readonly && !*editing.read();

    let opacity = if is_dragging { "opacity: 0.4;" } else { "" };
    let on_event = props.on_event;

    rsx! {
        div {
            class: "select-none",
            style: "{opacity}",
            draggable,
            ondragstart: move |e: Event<DragData>| {
                if !draggable { return; }
                let dt = e.data().data_transfer();
                let _ = dt.set_data(DT_MIME, &card_id.to_string());
                drag.set(Some(DragState { card: card_id, from_column }));
            },
            ondragend: move |_| {
                drag.set(None);
                ctx.hint.clone().set(None);
            },
            Card {
                class: "px-3 py-2 cursor-grab active:cursor-grabbing hover:border-border",
                InlineEdit {
                    value: props.card.title.clone(),
                    class: "text-sm font-medium w-full text-left",
                    on_editing_change: move |on: bool| editing.set(on),
                    on_commit: move |title: String| {
                        on_event.call(KanbanEvent::RenameCard { card: card_id, title });
                    },
                }
                if let Some(desc) = props.card.description.as_ref() {
                    CardDescription { class: "text-xs mt-1", "{desc}" }
                }
            }
        }
    }
}

/// MIME used in `DataTransfer`. Exposed so the column drop handler
/// can parse the dragged id without re-declaring the constant.
pub(crate) const fn dt_mime() -> &'static str {
    DT_MIME
}
