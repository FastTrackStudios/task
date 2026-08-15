//! Multi-row time-scale header.

use dioxus::prelude::*;

use crate::scales::ScaleGrid;

#[derive(Props, PartialEq, Clone)]
pub struct TimeScaleProps {
    pub grid: ScaleGrid,
    pub row_height: f32,
}

#[component]
pub fn TimeScale(props: TimeScaleProps) -> Element {
    let total_w = props.grid.total_width;
    let rows = props.grid.rows.clone();
    let h = props.row_height;

    rsx! {
        div {
            class: "gantt-timescale sticky top-0 z-20 flex flex-col bg-card border-b border-border",
            style: "width: {total_w}px;",
            for (i, row) in rows.iter().enumerate() {
                div {
                    key: "{i}",
                    class: "flex flex-row border-b border-border last:border-b-0",
                    style: "height: {h}px;",
                    for (j, cell) in row.cells.iter().enumerate() {
                        div {
                            key: "{j}",
                            class: "flex items-center justify-center text-xs text-muted-foreground border-r border-border/60 select-none",
                            style: "width: {cell.width}px;",
                            "{cell.label}"
                        }
                    }
                }
            }
        }
    }
}
