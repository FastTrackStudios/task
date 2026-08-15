//! [`InboxItem`] → markdown.
//!
//! Structured fields become YAML frontmatter (with an `inbox-item`
//! `type:` discriminator so vault tooling can recognize the file);
//! `body` becomes the markdown body verbatim.

use inbox_proto::InboxItem;

pub use vault_entity::WriteError;

pub fn serialize_inbox_item(item: &InboxItem) -> Result<String, WriteError> {
    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "inbox-item".into());
    map.insert("id".into(), item.id.clone().into());
    map.insert("kind".into(), item.kind.clone().into());
    map.insert("status".into(), item.status.clone().into());
    map.insert("source".into(), item.source.clone().into());
    map.insert("created".into(), item.created.clone().into());
    if let Some(r) = &item.resurface_on {
        map.insert("resurface_on".into(), r.clone().into());
    }
    if let Some(p) = &item.processed_into {
        map.insert("processed_into".into(), p.clone().into());
    }
    // Spaced-repetition state (obsidian-spaced-repetition SM-2).
    map.insert("sr-ease".into(), item.ease.into());
    map.insert("sr-interval".into(), item.interval.into());
    map.insert("sr-reviews".into(), item.reviews.into());

    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(map))
        .map_err(|e| WriteError::Yaml(e.to_string()))?;
    Ok(format!("---\n{yaml}---\n\n{}\n", item.body))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{frontmatter_split, parse_inbox_item};

    #[test]
    fn round_trip_inbox_item() {
        let item = InboxItem {
            id: "abc-123".into(),
            body: "Idea: render the daily plan as faded calendar blocks".into(),
            kind: "fleeting".into(),
            status: "open".into(),
            source: "cli".into(),
            created: "2026-05-31T09:00:00Z".into(),
            resurface_on: Some("2026-06-07".into()),
            processed_into: None,
            ease: 250,
            interval: 3,
            reviews: 1,
        };
        let page = serialize_inbox_item(&item).unwrap();
        let (fm, body) = frontmatter_split(&page).expect("frontmatter");
        let parsed = parse_inbox_item("Records/inbox/abc-123.md", fm, body).expect("parse");
        assert_eq!(parsed, item);
    }
}
