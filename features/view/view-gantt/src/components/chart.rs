//! Timeline pane — header + cell-grid + bars + links + markers.
//!
//! Pointer handlers for drag/resize live on the gantt-root (see
//! [`super::gantt::Gantt`]) — the chart pane itself is layout-only.

use chrono::Utc;
use dioxus::prelude::*;

use crate::scales::{ScaleGrid, x_for_date};
use crate::store::{LaidOutLink, LaidOutTask};

use super::bars::Bars;
use super::gantt::{GanttContext, ScrollContext};
use super::links::LinkLayer;
use super::timescale::TimeScale;

#[component]
pub fn Chart() -> Element {
    let ctx = use_context::<GanttContext>();
    let state = ctx.state;

    let s = state.read();
    let grid: ScaleGrid = s.build_grid();
    let (rows, links): (Vec<LaidOutTask>, Vec<LaidOutLink>) = s.layout(&grid);
    let total_w = grid.total_width;
    let row_h = s.row_height;
    let scale_h = row_h * grid.rows.len() as f32;
    let body_h = (row_h * rows.len() as f32).max(row_h);
    let total_h = scale_h + body_h;
    let markers = s.markers.clone();
    drop(s);

    // Virtualization — drop bars far outside the viewport. The
    // sticky timescale header costs us `scale_h` worth of "top
    // offset" inside the outer scroll, so the visible row range in
    // bar-local y coords is `scroll_top - scale_h ± buffer`.
    let scroll = use_context::<ScrollContext>();
    let st = *scroll.scroll_top.read();
    let vh = *scroll.viewport_h.read();
    let buffer = 400.0_f32;
    let cull_top = (st - scale_h - buffer).max(0.0);
    let cull_bot = st + vh - scale_h + buffer;
    let visible_rows: Vec<LaidOutTask> = rows
        .into_iter()
        .filter(|r| r.y + r.h >= cull_top && r.y <= cull_bot)
        .collect();
    let rows = visible_rows;

    // Pre-render vertical cell-grid lines from the smallest scale row.
    let bottom_row = grid.rows.last().cloned();
    let mut x_acc = 0.0_f32;
    let mut cell_lines: Vec<(f32, bool, f32)> = Vec::new();
    if let Some(br) = bottom_row {
        use chrono::Datelike;
        for cell in &br.cells {
            let weekend = matches!(
                cell.start.weekday(),
                chrono::Weekday::Sat | chrono::Weekday::Sun
            );
            cell_lines.push((x_acc, weekend, cell.width));
            x_acc += cell.width;
        }
    }

    let today_x = x_for_date(&grid, Utc::now());

    rsx! {
        div {
            class: "relative gantt-chart select-none",
            style: "width: {total_w}px; min-height: {total_h}px;",
            // Header
            TimeScale { grid: grid.clone(), row_height: row_h }

            // Body container — relative to position bars/links over cells.
            div {
                class: "relative",
                style: "width: {total_w}px; height: {body_h}px;",

                // Weekend shading + vertical grid lines.
                for (i, (x, weekend, cw)) in cell_lines.iter().copied().enumerate() {
                    {
                        let weekend_cls = "absolute top-0 bottom-0 bg-muted/40 pointer-events-none";
                        let line_cls = "absolute top-0 bottom-0 border-l border-border/40 pointer-events-none";
                        if weekend {
                            rsx!(div {
                                key: "we-{i}",
                                class: "{weekend_cls}",
                                style: "left: {x}px; width: {cw}px;",
                            })
                        } else {
                            let lx = x;
                            rsx!(div {
                                key: "gl-{i}",
                                class: "{line_cls}",
                                style: "left: {lx}px; width: 1px;",
                            })
                        }
                    }
                }

                // Horizontal row separators.
                for i in 0..rows.len() {
                    {
                        let y = (i as f32 + 1.0) * row_h;
                        rsx!(div {
                            key: "rh-{i}",
                            class: "absolute left-0 right-0 border-b border-border/40 pointer-events-none",
                            style: "top: {y}px; height: 0;",
                        })
                    }
                }

                // Today vertical line.
                div {
                    class: "absolute top-0 bottom-0 pointer-events-none",
                    style: "left: {today_x}px; width: 2px; background: rgba(239,68,68,0.7);",
                }

                // Custom markers.
                for (i, m) in markers.iter().enumerate() {
                    {
                        let mx = x_for_date(&grid, m.start);
                        rsx!(
                            div {
                                key: "m-{i}",
                                class: "absolute top-0 bottom-0 w-px bg-accent-foreground/60 pointer-events-none",
                                style: "left: {mx}px;",
                                title: "{m.text}",
                            }
                        )
                    }
                }

                // Dependency arrows behind the bars.
                LinkLayer { links: links.clone(), width: total_w, height: body_h }

                // Bars (with drag affordances).
                Bars {
                    rows: rows.clone(),
                    grid: grid.clone(),
                }
            }
        }
    }
}
