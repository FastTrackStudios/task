//! `AvailabilitySchedule` → markdown frontmatter.

use scheduling_proto::{AvailabilityRule, AvailabilitySchedule, Weekday};

use super::day_template::format_time;
use super::{WriteError, yaml_to_page};

pub fn serialize_schedule(s: &AvailabilitySchedule) -> Result<String, WriteError> {
    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "scheduling-schedule".into());
    map.insert("id".into(), s.id.0.clone().into());
    map.insert("name".into(), s.name.clone().into());
    if let Some(tz) = &s.timezone {
        map.insert("timezone".into(), tz.clone().into());
    }
    let rules: Vec<_> = s.rules.iter().map(serialize_rule).collect();
    map.insert("rules".into(), serde_yaml::Value::Sequence(rules));
    yaml_to_page(map)
}

fn serialize_rule(r: &AvailabilityRule) -> serde_yaml::Value {
    let mut m = serde_yaml::Mapping::new();
    let days: Vec<_> = r.days.iter().map(|d| weekday_label(*d).into()).collect();
    m.insert("days".into(), serde_yaml::Value::Sequence(days));
    m.insert("start".into(), format_time(r.start).into());
    m.insert("end".into(), format_time(r.end).into());
    serde_yaml::Value::Mapping(m)
}

fn weekday_label(d: Weekday) -> &'static str {
    match d {
        Weekday::Mon => "mon",
        Weekday::Tue => "tue",
        Weekday::Wed => "wed",
        Weekday::Thu => "thu",
        Weekday::Fri => "fri",
        Weekday::Sat => "sat",
        Weekday::Sun => "sun",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_schedule;
    use scheduling_proto::{ScheduleId, TimeOfDay};

    #[test]
    fn round_trip_working_hours() {
        let s = AvailabilitySchedule {
            id: ScheduleId("working-hours".into()),
            path: "schedules/working-hours.md".into(),
            name: "Working hours".into(),
            timezone: Some("America/Chicago".into()),
            rules: vec![AvailabilityRule {
                days: vec![
                    Weekday::Mon,
                    Weekday::Tue,
                    Weekday::Wed,
                    Weekday::Thu,
                    Weekday::Fri,
                ]
                .into(),
                start: TimeOfDay::new(9, 0),
                end: TimeOfDay::new(17, 0),
            }]
            .into(),
        };
        let md = serialize_schedule(&s).unwrap();
        let inner = md.trim_start_matches("---\n").trim_end_matches("---\n");
        let parsed = parse_schedule("schedules/working-hours.md", inner).unwrap();
        assert_eq!(parsed.rules.len(), 1);
        assert_eq!(parsed.rules[0].days.len(), 5);
    }
}
