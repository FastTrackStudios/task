//! `DayTemplate` → markdown frontmatter.

use scheduling_proto::{BlockCategory, DayTemplate, TimeBlock, TimeOfDay};

use super::{WriteError, yaml_to_page};

pub fn serialize_day_template(dt: &DayTemplate) -> Result<String, WriteError> {
    let mut blocks = Vec::with_capacity(dt.blocks.len());
    for b in dt.blocks.iter() {
        blocks.push(serialize_block(b));
    }

    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "scheduling-day-template".into());
    map.insert("id".into(), dt.id.0.clone().into());
    map.insert("name".into(), dt.name.clone().into());
    if let Some(d) = &dt.description {
        map.insert("description".into(), d.clone().into());
    }
    map.insert("blocks".into(), serde_yaml::Value::Sequence(blocks));

    yaml_to_page(map)
}

fn serialize_block(b: &TimeBlock) -> serde_yaml::Value {
    let mut m = serde_yaml::Mapping::new();
    m.insert("id".into(), b.id.0.clone().into());
    m.insert("label".into(), b.label.clone().into());
    m.insert("start".into(), format_time(b.start).into());
    m.insert("end".into(), format_time(b.end).into());
    m.insert("category".into(), category_label(b.category).into());
    if let Some(n) = &b.note {
        m.insert("note".into(), n.clone().into());
    }
    serde_yaml::Value::Mapping(m)
}

pub(crate) fn format_time(t: TimeOfDay) -> String {
    format!("{:02}:{:02}", t.hours(), t.minutes())
}

pub(crate) fn category_label(c: BlockCategory) -> &'static str {
    match c {
        BlockCategory::Reset => "reset",
        BlockCategory::Spiritual => "spiritual",
        BlockCategory::Meal => "meal",
        BlockCategory::Exercise => "exercise",
        BlockCategory::Hygiene => "hygiene",
        BlockCategory::Allocatable => "allocatable",
        BlockCategory::Maintenance => "maintenance",
        BlockCategory::WindDown => "winddown",
        BlockCategory::Sleep => "sleep",
        BlockCategory::Other => "other",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::parse_day_template;
    use scheduling_proto::{DayTemplateId, TimeBlockId};

    #[test]
    fn round_trip_preserves_blocks() {
        let dt = DayTemplate {
            path: "Projects/Scheduling/templates/weekday.md".into(),
            id: DayTemplateId("weekday".into()),
            name: "Weekday".into(),
            description: Some("Default".into()),
            blocks: vec![TimeBlock {
                id: TimeBlockId("morning".into()),
                start: TimeOfDay::new(6, 0),
                end: TimeOfDay::new(6, 30),
                label: "Morning Reset".into(),
                category: BlockCategory::Reset,
                note: None,
            }]
            .into(),
        };
        let md = serialize_day_template(&dt).unwrap();
        let inner = md
            .trim_start_matches("---\n")
            .trim_end_matches("---\n")
            .trim_end_matches("---");
        let parsed = parse_day_template("p", inner).unwrap();
        assert_eq!(parsed.name, "Weekday");
        assert_eq!(parsed.blocks.len(), 1);
        assert!(matches!(parsed.blocks[0].category, BlockCategory::Reset));
    }
}
