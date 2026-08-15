//! Scale / header computation — port of svar `store/src/scales.ts`.
//!
//! `build_scales` walks each row of the active zoom level, generating
//! one [`ScaleCell`] per step. The cell's pixel width is derived from
//! its real duration in the *minimum* unit times the chart's base
//! cell width.

use chrono::{DateTime, Utc};

use crate::time::{WeekStart, add, diff_f, min_unit, unit_start};
use crate::types::{LengthUnit, ScaleConfig};

#[derive(Clone, Debug, PartialEq)]
pub struct ScaleCell {
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub width: f32,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScaleRow {
    pub unit: LengthUnit,
    pub cells: Vec<ScaleCell>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ScaleGrid {
    pub rows: Vec<ScaleRow>,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub min_unit: LengthUnit,
    pub min_unit_width: f32,
    pub total_width: f32,
}

#[must_use]
pub fn build_scales(
    scales: &[ScaleConfig],
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    cell_width: f32,
    week_start: WeekStart,
) -> ScaleGrid {
    let mu = min_unit(&scales.iter().map(|s| s.unit).collect::<Vec<_>>());
    let snapped_start = unit_start(mu, start, week_start);
    let snapped_end = {
        let s = unit_start(mu, end, week_start);
        if s < end { add(mu, s, 1) } else { s }
    };
    let total_units = diff_f(mu, snapped_end, snapped_start).max(1.0);
    let total_width = total_units as f32 * cell_width;

    let rows = scales
        .iter()
        .map(|s| build_row(s, snapped_start, snapped_end, mu, cell_width, week_start))
        .collect();

    ScaleGrid {
        rows,
        start: snapped_start,
        end: snapped_end,
        min_unit: mu,
        min_unit_width: cell_width,
        total_width,
    }
}

fn build_row(
    scale: &ScaleConfig,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    base_unit: LengthUnit,
    cell_width: f32,
    week_start: WeekStart,
) -> ScaleRow {
    let mut cells = Vec::new();
    let mut date = unit_start(scale.unit, start, week_start);
    while date < end {
        let next = add(scale.unit, date, i64::from(scale.step));
        let cell_start = if date < start { start } else { date };
        let cell_end = if next > end { end } else { next };
        let units = diff_f(base_unit, cell_end, cell_start);
        let width = (units as f32) * cell_width;
        let label = format_cell(&scale.format, cell_start);
        cells.push(ScaleCell {
            start: cell_start,
            end: cell_end,
            width,
            label,
        });
        date = next;
    }
    ScaleRow {
        unit: scale.unit,
        cells,
    }
}

/// Format a cell's start date. Supports a few non-standard tokens
/// chrono doesn't ship out of the box (`%q` for quarter number).
fn format_cell(fmt: &str, date: DateTime<Utc>) -> String {
    use chrono::Datelike;
    let quarter = ((date.month() - 1) / 3) + 1;
    let with_q = fmt.replace("%q", &quarter.to_string());
    date.format(&with_q).to_string()
}

/// Pixel offset for `date` inside the grid (from `grid.start`).
#[must_use]
pub fn x_for_date(grid: &ScaleGrid, date: DateTime<Utc>) -> f32 {
    diff_f(grid.min_unit, date, grid.start) as f32 * grid.min_unit_width
}

/// Inverse — date for a pixel x offset.
#[must_use]
pub fn date_for_x(grid: &ScaleGrid, x: f32) -> DateTime<Utc> {
    let units = f64::from(x / grid.min_unit_width);
    let whole = units.floor() as i64;
    let frac = units - whole as f64;
    let base = add(grid.min_unit, grid.start, whole);
    let next = add(grid.min_unit, base, 1);
    let span_ms = (next - base).num_milliseconds() as f64;
    base + chrono::Duration::milliseconds((span_ms * frac) as i64)
}
