//! Frontmatter → proto type. One module per entity.

pub mod booking;
pub mod cal_event;
pub mod day_plan;
pub mod day_template;
pub mod event_type;
pub mod schedule;
mod yaml;

use thiserror::Error;

pub use booking::parse_booking;
pub use cal_event::parse_cal_event;
pub use day_plan::parse_day_plan;
pub use day_template::parse_day_template;
pub use event_type::parse_event_type;
pub use schedule::parse_schedule;

#[derive(Debug, Error)]
pub enum ParseError {
    #[error("no frontmatter at start of file")]
    NoFrontmatter,
    #[error("invalid frontmatter: {0}")]
    Frontmatter(String),
    #[error("invalid time string '{value}' (expected HH:MM)")]
    Time { value: String },
    #[error("missing required field: {field}")]
    MissingField { field: &'static str },
    #[error("invalid {field}: {reason}")]
    Invalid { field: &'static str, reason: String },
}
