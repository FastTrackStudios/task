//! Availability-schedule frontmatter → `AvailabilitySchedule`.

use scheduling_proto::{AvailabilityRule, AvailabilitySchedule, ScheduleId, Weekday};

use super::ParseError;
use super::day_template::parse_time;
use super::yaml::{parse_mapping, require_str, take_sequence, take_str};

pub fn parse_schedule(
    path: &str,
    frontmatter_yaml: &str,
) -> Result<AvailabilitySchedule, ParseError> {
    let map = parse_mapping(frontmatter_yaml)?;

    let id = take_str(&map, "id").unwrap_or_else(|| path.to_string());
    let name = require_str(&map, "name")?;
    let timezone = take_str(&map, "timezone");

    let rules_raw =
        take_sequence(&map, "rules").ok_or(ParseError::MissingField { field: "rules" })?;
    let mut rules = Vec::with_capacity(rules_raw.len());
    for raw in &rules_raw {
        rules.push(parse_rule(raw)?);
    }

    Ok(AvailabilitySchedule {
        id: ScheduleId(id),
        path: path.to_string(),
        name,
        timezone,
        rules: rules.into(),
    })
}

fn parse_rule(raw: &serde_yaml::Value) -> Result<AvailabilityRule, ParseError> {
    let m = raw.as_mapping().ok_or_else(|| ParseError::Invalid {
        field: "rules",
        reason: "rule must be mapping".into(),
    })?;
    let days_raw = m
        .get(serde_yaml::Value::String("days".into()))
        .and_then(serde_yaml::Value::as_sequence)
        .ok_or(ParseError::MissingField {
            field: "rules[].days",
        })?;
    let mut days = Vec::with_capacity(days_raw.len());
    for d in days_raw {
        days.push(parse_weekday(d.as_str().ok_or(ParseError::Invalid {
            field: "rules[].days",
            reason: "expected string day name".into(),
        })?)?);
    }
    let start = m
        .get(serde_yaml::Value::String("start".into()))
        .and_then(serde_yaml::Value::as_str)
        .ok_or(ParseError::MissingField {
            field: "rules[].start",
        })?;
    let end = m
        .get(serde_yaml::Value::String("end".into()))
        .and_then(serde_yaml::Value::as_str)
        .ok_or(ParseError::MissingField {
            field: "rules[].end",
        })?;
    Ok(AvailabilityRule {
        days: days.into(),
        start: parse_time(start)?,
        end: parse_time(end)?,
    })
}

fn parse_weekday(value: &str) -> Result<Weekday, ParseError> {
    Ok(match value.to_ascii_lowercase().as_str() {
        "mon" | "monday" => Weekday::Mon,
        "tue" | "tuesday" => Weekday::Tue,
        "wed" | "wednesday" => Weekday::Wed,
        "thu" | "thursday" => Weekday::Thu,
        "fri" | "friday" => Weekday::Fri,
        "sat" | "saturday" => Weekday::Sat,
        "sun" | "sunday" => Weekday::Sun,
        other => {
            return Err(ParseError::Invalid {
                field: "rules[].days",
                reason: format!("unknown weekday '{other}'"),
            });
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_working_hours() {
        let yaml = r#"
type: scheduling-schedule
id: working-hours
name: Working hours
timezone: America/Chicago
rules:
  - days: [mon, tue, wed, thu, fri]
    start: "09:00"
    end: "12:00"
  - days: [mon, tue, wed, thu, fri]
    start: "13:00"
    end: "17:00"
"#;
        let s = parse_schedule("schedules/working-hours.md", yaml).unwrap();
        assert_eq!(s.name, "Working hours");
        assert_eq!(s.rules.len(), 2);
        assert_eq!(s.rules[0].days.len(), 5);
        assert!(matches!(s.rules[0].days[0], Weekday::Mon));
    }
}
