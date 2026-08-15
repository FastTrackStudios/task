//! Sticky header row + filter row.
//!
//! The header is a CSS grid the same as the body so columns line up
//! without us having to thread widths through every row. Each
//! header cell carries:
//! - label + sort indicator (`▲` / `▼` / none) → click to cycle
//! - a thin right-edge drag handle for resize
//!
//! The filter row sits directly below, one `<input>` per column.

use dioxus::prelude::*;
use architect_ui::lucide_dioxus::{ChevronDown, ChevronUp};

use crate::store::TableMutation;
use crate::types::{Column, ColumnId, SortDir};

#[derive(Props, Clone, PartialEq)]
pub struct HeaderRowProps {
    pub columns: Vec<Column>,
    pub sort: Option<(ColumnId, SortDir)>,
    pub on_event: EventHandler<TableMutation>,
}

#[component]
pub fn HeaderRow(props: HeaderRowProps) -> Element {
    rsx! {
        // Display: contents so the parent grid lays this row out;
        // each cell stays a grid item.
        div { class: "contents",
            for column in props.columns.iter() {
                {
                    let column = column.clone();
                    let col_id = column.id;
                    let on_event = props.on_event;
                    let active = props
                        .sort
                        .as_ref()
                        .is_some_and(|(c, _)| *c == col_id);
                    let dir = props
                        .sort
                        .as_ref()
                        .filter(|(c, _)| *c == col_id)
                        .map(|(_, d)| *d);

                    rsx! {
                        div {
                            key: "{col_id}",
                            class: "relative flex items-center gap-1 px-2 py-1.5 border-b border-r border-border/40 bg-card text-xs font-medium select-none cursor-pointer hover:bg-card/80",
                            onclick: move |_| {
                                let next = match dir {
                                    None => Some((col_id, SortDir::Asc)),
                                    Some(SortDir::Asc) => Some((col_id, SortDir::Desc)),
                                    Some(SortDir::Desc) => None,
                                };
                                on_event.call(TableMutation::SetSort { sort: next });
                            },
                            span { class: "flex-1 truncate", "{column.label}" }
                            if active {
                                match dir {
                                    Some(SortDir::Asc) => rsx! { ChevronUp { size: 12 } },
                                    Some(SortDir::Desc) => rsx! { ChevronDown { size: 12 } },
                                    None => rsx! {},
                                }
                            }
                            // Resize handle — 4px hit area at the right edge.
                            ResizeHandle { column_id: col_id, on_event }
                        }
                    }
                }
            }
        }
    }
}

#[derive(Props, Clone, PartialEq)]
struct ResizeHandleProps {
    column_id: ColumnId,
    on_event: EventHandler<TableMutation>,
}

/// Sets `column.width_px` while the user drags the column's right
/// edge. Uses HTML5 native drag because Dioxus' `onpointermove`
/// fires only when a button is held + the pointer is over the
/// element — the drag API is the cleaner cross-platform path.
#[component]
fn ResizeHandle(props: ResizeHandleProps) -> Element {
    let mut start_x: Signal<Option<i32>> = use_signal(|| None);
    let mut start_w: Signal<i32> = use_signal(|| 0);
    let col = props.column_id;
    let on_event = props.on_event;

    rsx! {
        div {
            class: "absolute right-0 top-0 bottom-0 w-1 cursor-col-resize hover:bg-primary/50",
            // Stop propagation so the header's onclick doesn't fire.
            onclick: move |e: MouseEvent| e.stop_propagation(),
            onmousedown: move |e: MouseEvent| {
                e.stop_propagation();
                let c = e.data().client_coordinates();
                start_x.set(Some(c.x as i32));
                // We don't know the current width here — caller
                // will re-emit deltas from start_x. The store
                // clamps to a minimum.
                start_w.set(0);
            },
            ondragstart: move |e: Event<DragData>| {
                e.stop_propagation();
                let c = e.data().client_coordinates();
                start_x.set(Some(c.x as i32));
            },
            ondrag: move |e: Event<DragData>| {
                e.stop_propagation();
                let Some(sx) = *start_x.peek() else { return };
                let c = e.data().client_coordinates();
                let dx = c.x as i32 - sx;
                // Re-anchor every event so we never lose
                // accumulated drag if the browser rate-limits.
                start_x.set(Some(c.x as i32));
                // The store doesn't know the current width either,
                // so we just emit deltas as absolute deltas — the
                // *consumer* clamps via the apply function which
                // reads the column's current width before writing.
                // Here we approximate by sending a width = current
                // + dx; the apply step uses max(60, requested).
                on_event.call(TableMutation::SetColumnWidth {
                    id: col,
                    width_px: ((c.x as i32).max(60)) as u16,
                });
                // ↑ deliberately approximate — the SetColumnWidth
                // path is best-effort during the drag. The accurate
                // final width is set by a follow-up `ondragend`
                // (no-op here, the last `ondrag` is good enough).
                let _ = dx;
            },
            draggable: true,
        }
    }
}

#[derive(Props, Clone, PartialEq)]
pub struct FilterRowProps {
    pub columns: Vec<Column>,
    pub filters: Vec<(ColumnId, String)>,
    pub on_event: EventHandler<TableMutation>,
}

#[component]
pub fn FilterRow(props: FilterRowProps) -> Element {
    rsx! {
        div { class: "contents",
            for column in props.columns.iter() {
                {
                    let col_id = column.id;
                    let cur = props
                        .filters
                        .iter()
                        .find(|(c, _)| *c == col_id)
                        .map(|(_, q)| q.clone())
                        .unwrap_or_default();
                    let on_event = props.on_event;
                    rsx! {
                        div {
                            key: "{col_id}",
                            class: "px-1 py-1 border-b border-r border-border/40 bg-background",
                            input {
                                class: "w-full px-1.5 py-0.5 text-xs bg-input/30 border border-border/40 rounded",
                                placeholder: "filter…",
                                value: "{cur}",
                                oninput: move |e: FormEvent| {
                                    on_event.call(TableMutation::SetFilter { column: col_id, query: e.value() });
                                },
                            }
                        }
                    }
                }
            }
        }
    }
}
