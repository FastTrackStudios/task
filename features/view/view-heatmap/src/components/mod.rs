//! Heatmap component tree — root [`Heatmap`] dispatches on
//! [`HeatmapStyle`] to either the GitHub-style grid or the
//! weekly-bar chart.

mod bars;
mod cyclic;
mod grid;

use chrono::NaiveDate;
use dioxus::prelude::*;

use crate::intensity::ColorTag;

/// Which renderer to use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum HeatmapStyle {
    /// 53 weeks × 7 days, color by intensity.
    #[default]
    Grid,
    /// One bar per day for the visible week. Internal `chrono::Local`
    /// signal controls the visible week + chevron nav.
    Bars,
    /// Cyclic-planning year: 4 quarters × (3 cycles + reset week).
    /// Each cycle = 4 weeks. Optional 53rd "bonus / week 0" on
    /// cyclic leap years. See [`crate::cyclic`] for the math.
    Cyclic,
}

#[derive(Props, Clone, PartialEq)]
pub struct HeatmapProps {
    /// (Date, count) pairs. Dates outside the visible window are
    /// silently ignored; duplicates for the same date are summed.
    pub points: Vec<(NaiveDate, u32)>,
    /// Accent color stem used for cells / bars.
    #[props(default)]
    pub color: ColorTag,
    #[props(default)]
    pub style: HeatmapStyle,
    /// Grid-only: anchor date used to position the trailing edge of
    /// the year window. Defaults to today.
    #[props(default)]
    pub anchor: Option<NaiveDate>,
}

#[component]
pub fn Heatmap(props: HeatmapProps) -> Element {
    let today = chrono::Local::now().date_naive();
    let anchor = props.anchor.unwrap_or(today);
    match props.style {
        HeatmapStyle::Grid => rsx! {
            grid::GridView {
                anchor,
                points: props.points,
                color: props.color,
            }
        },
        HeatmapStyle::Bars => rsx! {
            bars::BarsView {
                anchor,
                points: props.points,
                color: props.color,
            }
        },
        HeatmapStyle::Cyclic => rsx! {
            cyclic::CyclicView {
                anchor,
                points: props.points,
                color: props.color,
            }
        },
    }
}
