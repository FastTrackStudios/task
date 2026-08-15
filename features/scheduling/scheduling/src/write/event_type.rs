//! `EventType` → markdown frontmatter.

use scheduling_proto::{EventType, EventTypeLocation};

use super::{WriteError, yaml_to_page};

pub fn serialize_event_type(et: &EventType) -> Result<String, WriteError> {
    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "scheduling-event-type".into());
    map.insert("id".into(), et.id.0.clone().into());
    map.insert("title".into(), et.title.clone().into());
    map.insert("slug".into(), et.slug.clone().into());
    if let Some(d) = &et.description {
        map.insert("description".into(), d.clone().into());
    }
    map.insert(
        "duration_min".into(),
        serde_yaml::Value::Number(serde_yaml::Number::from(et.duration_min)),
    );
    if et.buffer_min > 0 {
        map.insert(
            "buffer_min".into(),
            serde_yaml::Value::Number(serde_yaml::Number::from(et.buffer_min)),
        );
    }
    if let Some(sid) = &et.schedule_id {
        map.insert("schedule_id".into(), sid.0.clone().into());
    }
    map.insert("published".into(), et.published.into());
    map.insert("location".into(), serialize_location(&et.location));

    yaml_to_page(map)
}

fn serialize_location(loc: &EventTypeLocation) -> serde_yaml::Value {
    let mut m = serde_yaml::Mapping::new();
    match loc {
        EventTypeLocation::Phone => {
            m.insert("kind".into(), "phone".into());
        }
        EventTypeLocation::Tbd => {
            m.insert("kind".into(), "tbd".into());
        }
        EventTypeLocation::InPerson { address } => {
            m.insert("kind".into(), "in_person".into());
            m.insert("address".into(), address.clone().into());
        }
        EventTypeLocation::Link { url } => {
            m.insert("kind".into(), "link".into());
            m.insert("url".into(), url.clone().into());
        }
    }
    serde_yaml::Value::Mapping(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_event_type;
    use scheduling_proto::{EventTypeId, ScheduleId};

    #[test]
    fn round_trip_consult() {
        let et = EventType {
            id: EventTypeId("consult-30".into()),
            path: "event-types/consult-30.md".into(),
            title: "30-minute consultation".into(),
            slug: "consult-30".into(),
            description: None,
            duration_min: 30,
            buffer_min: 5,
            location: EventTypeLocation::Link {
                url: "https://meet.example/room/abc".into(),
            },
            schedule_id: Some(ScheduleId("working-hours".into())),
            published: true,
        };
        let md = serialize_event_type(&et).unwrap();
        let inner = md.trim_start_matches("---\n").trim_end_matches("---\n");
        let parsed = parse_event_type("event-types/consult-30.md", inner).unwrap();
        assert_eq!(parsed.duration_min, 30);
        assert!(matches!(parsed.location, EventTypeLocation::Link { .. }));
    }
}
