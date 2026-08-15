//! `DayPlan` → markdown frontmatter.

use scheduling_proto::{BlockAssignment, DayPlan, PlannedBlock};

use super::day_template::{category_label, format_time};
use super::{WriteError, yaml_to_page};

pub fn serialize_day_plan(plan: &DayPlan) -> Result<String, WriteError> {
    let blocks: Vec<serde_yaml::Value> = plan.blocks.iter().map(serialize_block).collect();

    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "scheduling-day-plan".into());
    map.insert("date".into(), plan.date.clone().into());
    if let Some(t) = &plan.from_template {
        map.insert("from_template".into(), t.0.clone().into());
    }
    map.insert("blocks".into(), serde_yaml::Value::Sequence(blocks));

    yaml_to_page(map)
}

fn serialize_block(b: &PlannedBlock) -> serde_yaml::Value {
    let mut m = serde_yaml::Mapping::new();
    m.insert("id".into(), b.id.0.clone().into());
    m.insert("label".into(), b.label.clone().into());
    m.insert("start".into(), format_time(b.start).into());
    m.insert("end".into(), format_time(b.end).into());
    m.insert("category".into(), category_label(b.category).into());
    if b.fixed {
        m.insert("fixed".into(), true.into());
    }
    if let Some(n) = &b.note {
        m.insert("note".into(), n.clone().into());
    }
    if let Some(a) = &b.assignment {
        m.insert("assignment".into(), serialize_assignment(a));
    }
    serde_yaml::Value::Mapping(m)
}

fn serialize_assignment(a: &BlockAssignment) -> serde_yaml::Value {
    let mut m = serde_yaml::Mapping::new();
    m.insert("kind".into(), a.kind.clone().into());
    m.insert("title".into(), a.title.clone().into());
    if let Some(r) = &a.ref_id {
        m.insert("ref_id".into(), r.clone().into());
    }
    serde_yaml::Value::Mapping(m)
}

#[cfg(test)]
mod tests {
    use super::*;
    use scheduling_proto::{BlockCategory, DayTemplateId, TimeBlockId, TimeOfDay};

    #[test]
    fn round_trip_day_plan() {
        let plan = DayPlan {
            date: "2026-05-30".into(),
            from_template: Some(DayTemplateId("weekday".into())),
            blocks: vec![
                PlannedBlock {
                    id: TimeBlockId("block-1".into()),
                    start: TimeOfDay::new(9, 30),
                    end: TimeOfDay::new(12, 30),
                    label: "Block 1".into(),
                    category: BlockCategory::Allocatable,
                    note: None,
                    assignment: Some(BlockAssignment {
                        kind: "task".into(),
                        title: "Ship the sync engine".into(),
                        ref_id: Some("abc-123".into()),
                    }),
                    fixed: true,
                },
                PlannedBlock {
                    id: TimeBlockId("gym".into()),
                    start: TimeOfDay::new(8, 0),
                    end: TimeOfDay::new(9, 0),
                    label: "Gym".into(),
                    category: BlockCategory::Exercise,
                    note: Some("long run".into()),
                    assignment: None,
                    fixed: false,
                },
            ]
            .into(),
        };
        let page = serialize_day_plan(&plan).unwrap();
        let (fm, _) = crate::scan::frontmatter_split(&page).expect("frontmatter");
        let parsed =
            crate::parse::parse_day_plan("Records/dayplans/2026-05-30.md", fm).expect("parse");
        assert_eq!(parsed, plan);
    }
}
