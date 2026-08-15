//! Event-type frontmatter → `EventType`.

use scheduling_proto::{EventType, EventTypeId, EventTypeLocation, ScheduleId};

use super::ParseError;
use super::yaml::{parse_mapping, require_str, take_bool, take_str, take_u16};

pub fn parse_event_type(path: &str, frontmatter_yaml: &str) -> Result<EventType, ParseError> {
    let map = parse_mapping(frontmatter_yaml)?;

    let id = take_str(&map, "id").unwrap_or_else(|| path.to_string());
    let title = require_str(&map, "title")?;
    let slug = require_str(&map, "slug")?;
    let description = take_str(&map, "description");
    let duration_min = take_u16(&map, "duration_min").ok_or(ParseError::MissingField {
        field: "duration_min",
    })?;
    let buffer_min = take_u16(&map, "buffer_min").unwrap_or(0);
    let schedule_id = take_str(&map, "schedule_id").map(ScheduleId);
    let published = take_bool(&map, "published").unwrap_or(true);
    let location = parse_location(&map)?;

    Ok(EventType {
        id: EventTypeId(id),
        path: path.to_string(),
        title,
        slug,
        description,
        duration_min,
        buffer_min,
        location,
        schedule_id,
        published,
    })
}

/// Location is `location: { kind: phone | in_person | link | tbd, address?, url? }`.
/// We prefer a nested mapping over flat keys so the type
/// discriminator + payload travel together.
fn parse_location(map: &serde_yaml::Mapping) -> Result<EventTypeLocation, ParseError> {
    let loc_val = map.get(serde_yaml::Value::String("location".into()));
    let Some(loc) = loc_val else {
        return Ok(EventTypeLocation::Tbd);
    };
    let m = loc.as_mapping().ok_or_else(|| ParseError::Invalid {
        field: "location",
        reason: "expected mapping".into(),
    })?;
    let kind = m
        .get(serde_yaml::Value::String("kind".into()))
        .and_then(serde_yaml::Value::as_str)
        .ok_or(ParseError::MissingField {
            field: "location.kind",
        })?
        .to_ascii_lowercase();
    match kind.as_str() {
        "phone" => Ok(EventTypeLocation::Phone),
        "tbd" | "" => Ok(EventTypeLocation::Tbd),
        "in_person" | "in-person" | "inperson" => {
            let address = m
                .get(serde_yaml::Value::String("address".into()))
                .and_then(serde_yaml::Value::as_str)
                .ok_or(ParseError::MissingField {
                    field: "location.address",
                })?
                .to_string();
            Ok(EventTypeLocation::InPerson { address })
        }
        "link" | "url" => {
            let url = m
                .get(serde_yaml::Value::String("url".into()))
                .and_then(serde_yaml::Value::as_str)
                .ok_or(ParseError::MissingField {
                    field: "location.url",
                })?
                .to_string();
            Ok(EventTypeLocation::Link { url })
        }
        other => Err(ParseError::Invalid {
            field: "location.kind",
            reason: format!("unknown kind '{other}'"),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_consultation() {
        let yaml = r"
type: scheduling-event-type
id: consult-30
title: 30-minute consultation
slug: consult-30
duration_min: 30
buffer_min: 5
published: true
schedule_id: working-hours
location:
  kind: link
  url: https://meet.example/room/abc
";
        let et = parse_event_type("event-types/consult-30.md", yaml).unwrap();
        assert_eq!(et.title, "30-minute consultation");
        assert_eq!(et.duration_min, 30);
        assert_eq!(et.buffer_min, 5);
        assert!(matches!(et.location, EventTypeLocation::Link { .. }));
    }

    #[test]
    fn missing_duration_errors() {
        let yaml = r"
type: scheduling-event-type
title: x
slug: x
";
        assert!(matches!(
            parse_event_type("p", yaml),
            Err(ParseError::MissingField {
                field: "duration_min"
            })
        ));
    }
}
