//! Calendar-event frontmatter → `CalEvent`.

use scheduling_proto::CalEvent;

use super::ParseError;
use super::yaml::{parse_mapping, require_str, take_bool, take_str};

pub fn parse_cal_event(path: &str, frontmatter_yaml: &str) -> Result<CalEvent, ParseError> {
    let map = parse_mapping(frontmatter_yaml)?;
    let id = take_str(&map, "id").unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string()
    });
    Ok(CalEvent {
        id,
        title: take_str(&map, "title").unwrap_or_default(),
        start: require_str(&map, "start")?,
        end: require_str(&map, "end")?,
        all_day: take_bool(&map, "all_day").unwrap_or(false),
        color: take_str(&map, "color").unwrap_or_else(|| "primary".into()),
        description: take_str(&map, "description"),
        recurrence: take_str(&map, "recurrence"),
    })
}
