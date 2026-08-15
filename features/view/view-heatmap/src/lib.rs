//! Heatmap — habit / activity visualization.
//!
//! Two styles:
//! - [`HeatmapStyle::Grid`] — GitHub contribution-graph grid
//!   (53 weeks × 7 days, color by intensity bucket).
//! - [`HeatmapStyle::Bars`] — CodexMonitor-style weekly bars with
//!   prev/next chevrons.
//!
//! Both consume the same `points: Vec<(NaiveDate, u32)>` input and
//! the same `color: ColorTag` accent. Pick the style at the call
//! site:
//!
//! ```ignore
//! Heatmap { style: HeatmapStyle::Grid, points: my_data(), color: ColorTag::Success }
//! ```

pub mod components;
pub mod cyclic;
pub mod intensity;

pub use components::{Heatmap, HeatmapProps, HeatmapStyle};
pub use cyclic::{CyclicConfig, WeekCoord, WeekSlot};
pub use intensity::{ColorTag, IntensityBucket, bucket_for};
