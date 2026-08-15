//! Day-plan frontmatter → `DayPlan`.

use scheduling_proto::{BlockAssignment, DayPlan, DayTemplateId, PlannedBlock, TimeBlockId};

use super::ParseError;
use super::day_template::{parse_category, parse_time};
use super::yaml::{parse_mapping, take_sequence, take_str};

/// Parse the frontmatter side of a day-plan markdown page.
pub fn parse_day_plan(path: &str, frontmatter_yaml: &str) -> Result<DayPlan, ParseError> {
    let map = parse_mapping(frontmatter_yaml)?;

    // `date` is the key; fall back to the file stem if missing.
    let date = take_str(&map, "date").unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string()
    });
    let from_template = take_str(&map, "from_template").map(DayTemplateId);

    let blocks_raw =
        take_sequence(&map, "blocks").ok_or(ParseError::MissingField { field: "blocks" })?;
    let mut blocks = Vec::with_capacity(blocks_raw.len());
    for raw in &blocks_raw {
        blocks.push(parse_block(raw)?);
    }

    Ok(DayPlan {
        date,
        from_template,
        blocks: blocks.into(),
    })
}

fn parse_block(raw: &serde_yaml::Value) -> Result<PlannedBlock, ParseError> {
    let m = raw
        .as_mapping()
        .ok_or_else(|| ParseError::Frontmatter("block must be mapping".into()))?;
    let get = |k: &str| {
        m.get(serde_yaml::Value::String(k.into()))
            .and_then(serde_yaml::Value::as_str)
    };
    let id = get("id").map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_string);
    let label = get("label")
        .ok_or(ParseError::MissingField { field: "label" })?
        .to_string();
    let start = get("start").ok_or(ParseError::MissingField { field: "start" })?;
    let end = get("end").ok_or(ParseError::MissingField { field: "end" })?;
    let category = get("category").map_or(scheduling_proto::BlockCategory::Other, parse_category);
    let note = get("note").map(str::to_string);
    let fixed = m
        .get(serde_yaml::Value::String("fixed".into()))
        .and_then(serde_yaml::Value::as_bool)
        .unwrap_or(false);

    // Assignment is a nested mapping: { kind, title, ref_id? }.
    let assignment = m
        .get(serde_yaml::Value::String("assignment".into()))
        .and_then(serde_yaml::Value::as_mapping)
        .and_then(|am| {
            let aget = |k: &str| {
                am.get(serde_yaml::Value::String(k.into()))
                    .and_then(serde_yaml::Value::as_str)
                    .map(str::to_string)
            };
            // A label assignment needs at least a title.
            let title = aget("title")?;
            Some(BlockAssignment {
                kind: aget("kind").unwrap_or_else(|| "label".into()),
                title,
                ref_id: aget("ref_id"),
            })
        });

    Ok(PlannedBlock {
        id: TimeBlockId(id),
        start: parse_time(start)?,
        end: parse_time(end)?,
        label,
        category,
        note,
        assignment,
        fixed,
    })
}
