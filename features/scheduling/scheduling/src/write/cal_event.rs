//! `CalEvent` → markdown frontmatter.

use scheduling_proto::CalEvent;

use super::{WriteError, yaml_to_page};

pub fn serialize_cal_event(e: &CalEvent) -> Result<String, WriteError> {
    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "scheduling-cal-event".into());
    map.insert("id".into(), e.id.clone().into());
    map.insert("title".into(), e.title.clone().into());
    map.insert("start".into(), e.start.clone().into());
    map.insert("end".into(), e.end.clone().into());
    map.insert("all_day".into(), e.all_day.into());
    map.insert("color".into(), e.color.clone().into());
    if let Some(d) = &e.description {
        map.insert("description".into(), d.clone().into());
    }
    if let Some(r) = &e.recurrence {
        map.insert("recurrence".into(), r.clone().into());
    }
    yaml_to_page(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_cal_event() {
        let e = CalEvent {
            id: "evt-1".into(),
            title: "Standup".into(),
            start: "2026-05-30T16:00:00Z".into(),
            end: "2026-05-30T16:30:00Z".into(),
            all_day: false,
            color: "info".into(),
            description: Some("Daily sync".into()),
            recurrence: None,
        };
        let page = serialize_cal_event(&e).unwrap();
        let (fm, _) = crate::scan::frontmatter_split(&page).expect("frontmatter");
        let parsed = crate::parse::parse_cal_event("Records/events/evt-1.md", fm).expect("parse");
        assert_eq!(parsed, e);
    }
}
