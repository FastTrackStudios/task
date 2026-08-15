//! Proto type → markdown frontmatter. One module per entity.

pub mod booking;
pub mod cal_event;
pub mod day_plan;
pub mod day_template;
pub mod event_type;
pub mod schedule;

use thiserror::Error;

pub use booking::serialize_booking;
pub use cal_event::serialize_cal_event;
pub use day_plan::serialize_day_plan;
pub use day_template::serialize_day_template;
pub use event_type::serialize_event_type;
pub use schedule::serialize_schedule;

#[derive(Debug, Error)]
pub enum WriteError {
    #[error("yaml serialize: {0}")]
    Yaml(String),
}

pub(crate) fn yaml_to_page(map: serde_yaml::Mapping) -> Result<String, WriteError> {
    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(map))
        .map_err(|e| WriteError::Yaml(e.to_string()))?;
    Ok(format!("---\n{yaml}---\n"))
}
