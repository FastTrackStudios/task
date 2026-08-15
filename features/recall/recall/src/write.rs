//! [`RecallCard`] → markdown.
//!
//! Structured fields become YAML frontmatter (with a `recall-card`
//! `type:` discriminator so vault tooling can recognize the file; FSRS
//! state under `sr-*` keys); the front + back become the markdown body,
//! separated by a `<!-- back -->` marker.

use recall_proto::RecallCard;

use crate::parse::BACK_MARKER;

pub use vault_entity::WriteError;

pub fn serialize_recall_card(card: &RecallCard) -> Result<String, WriteError> {
    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), "recall-card".into());
    map.insert("id".into(), card.id.clone().into());
    map.insert("project".into(), card.project.clone().into());
    map.insert("card_type".into(), card.card_type.clone().into());
    if let Some(src) = &card.source_note {
        map.insert("source_note".into(), src.clone().into());
    }
    // FSRS memory state.
    map.insert("sr-stability".into(), card.stability.into());
    map.insert("sr-difficulty".into(), card.difficulty.into());
    map.insert("sr-reps".into(), card.reps.into());
    map.insert("sr-lapses".into(), card.lapses.into());
    if let Some(due) = &card.due {
        map.insert("sr-due".into(), due.clone().into());
    }
    if let Some(last) = &card.last_review {
        map.insert("sr-last-review".into(), last.clone().into());
    }
    map.insert("archived".into(), card.archived.into());
    map.insert("created".into(), card.created.clone().into());

    let yaml = serde_yaml::to_string(&serde_yaml::Value::Mapping(map))
        .map_err(|e| WriteError::Yaml(e.to_string()))?;
    Ok(format!(
        "---\n{yaml}---\n\n{}\n\n{BACK_MARKER}\n\n{}\n",
        card.front, card.back
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse::{frontmatter_split, parse_recall_card};
    use recall_proto::CardType;

    #[test]
    fn round_trip_recall_card() {
        let mut card = RecallCard::create(
            "abc-123",
            "Bible",
            CardType::REFERENCE_TO_VERSE,
            "John 3:16",
            "For God so loved the world,\nthat he gave his only Son…",
            "2026-07-16T09:00:00Z",
        );
        card.stability = 12.5;
        card.difficulty = 5.2;
        card.reps = 3;
        card.lapses = 1;
        card.due = Some("2026-07-28".into());
        card.last_review = Some("2026-07-16".into());
        card.source_note = Some("Wiki/John.md".into());

        let page = serialize_recall_card(&card).unwrap();
        let (fm, body) = frontmatter_split(&page).expect("frontmatter");
        let parsed = parse_recall_card("Records/recall/abc-123.md", fm, body).expect("parse");
        assert_eq!(parsed, card);
    }

    #[test]
    fn front_only_card_round_trips() {
        let card = RecallCard::create(
            "f1",
            "",
            CardType::FREE,
            "just a front",
            "",
            "2026-07-16T09:00:00Z",
        );
        let page = serialize_recall_card(&card).unwrap();
        let (fm, body) = frontmatter_split(&page).expect("frontmatter");
        let parsed = parse_recall_card("Records/recall/f1.md", fm, body).expect("parse");
        assert_eq!(parsed.front, "just a front");
        assert_eq!(parsed.back, "");
    }
}
