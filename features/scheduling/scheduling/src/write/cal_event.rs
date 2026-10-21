//! `CalEvent` → markdown frontmatter.

use scheduling_proto::{CalEvent, EventLink};

use super::{WriteError, yaml_to_page};

/// The event's own frontmatter, as a mapping. Links are grouped by
/// relation (first appearance order), each relation a list of
/// wikilinks — the Obsidian list-property shape, so the vault link
/// index sees every target as an edge and a `.base` sees `note.for`.
// t[impl scheduling.event.links]
fn event_mapping(e: &CalEvent, kind: serde_yaml::Value) -> serde_yaml::Mapping {
    let mut map = serde_yaml::Mapping::new();
    map.insert("type".into(), kind);
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
    if let Some(l) = &e.location {
        map.insert("location".into(), l.clone().into());
    }
    if let Some(r) = &e.external_ref {
        map.insert("external_ref".into(), r.to_string().into());
    }
    if let Some(t) = &e.tombstone {
        map.insert("tombstone".into(), t.clone().into());
    }
    let mut relations: Vec<&str> = Vec::new();
    for l in e.links.iter() {
        if EventLink::valid_relation(&l.relation) && !relations.contains(&l.relation.as_str()) {
            relations.push(&l.relation);
        }
    }
    for rel in relations {
        let targets: Vec<serde_yaml::Value> = e
            .links
            .iter()
            .filter(|l| l.relation == rel)
            .map(|l| EventLink::new(rel, &l.target).target.into())
            .collect();
        map.insert(rel.into(), serde_yaml::Value::Sequence(targets));
    }
    map
}

pub fn serialize_cal_event(e: &CalEvent) -> Result<String, WriteError> {
    yaml_to_page(event_mapping(e, "scheduling-cal-event".into()))
}

/// Rewrite an event file that may already exist, keeping what isn't
/// the event's to own: the markdown body, the file's `type:` (seeded
/// events are `type: event`), and every frontmatter property that is
/// neither a reserved event key nor a link relation — a tag, a
/// `status` a `.base` set, an integration's own bookkeeping. Without
/// this a reschedule from the calendar would wipe the setlist out of
/// an event note.
// t[impl scheduling.event.rewrite-keeps]
pub fn merge_cal_event_page(e: &CalEvent, existing: Option<&str>) -> Result<String, WriteError> {
    let Some((fm, body)) = existing.and_then(crate::scan::frontmatter_split) else {
        return serialize_cal_event(e);
    };
    let old: serde_yaml::Mapping = serde_yaml::from_str(fm).unwrap_or_default();
    let kind = old
        .get("type")
        .cloned()
        .unwrap_or_else(|| "scheduling-cal-event".into());
    let mut map = event_mapping(e, kind);
    for (k, v) in &old {
        let Some(key) = k.as_str() else { continue };
        // Reserved, but Obsidian's rather than the event's: kept.
        let obsidian = matches!(key, "tags" | "aliases" | "cssclasses");
        let owned = CalEvent::RESERVED_KEYS.contains(&key) && !obsidian;
        let is_link = EventLink::valid_relation(key)
            && crate::parse::cal_event::wikilink_values(v).is_some();
        if owned || is_link || map.contains_key(k) {
            continue;
        }
        map.insert(k.clone(), v.clone());
    }
    let head = yaml_to_page(map)?;
    Ok(format!("{head}{body}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use scheduling_proto::ExternalRef;

    #[test]
    fn round_trip_cal_event() {
        let mut e = CalEvent::new(
            "evt-1",
            "Standup",
            "2026-05-30T16:00:00Z",
            "2026-05-30T16:30:00Z",
        );
        e.color = "info".into();
        e.description = Some("Daily sync".into());
        e.location = Some("Room 2".into());
        e.links = vec![
            EventLink::new("for", "Gigs/caa-2026"),
            EventLink::new("venue", "[[The Hall]]"),
            EventLink::new("for", "[[Projects/CAA]]"),
        ]
        .into();
        e.external_ref = ExternalRef::parse("pco:plan_time:123");
        let page = serialize_cal_event(&e).unwrap();
        assert!(page.contains("[[Gigs/caa-2026]]") && page.contains("[[Projects/CAA]]"), "{page}");
        let (fm, _) = crate::scan::frontmatter_split(&page).expect("frontmatter");
        let parsed = crate::parse::parse_cal_event("Records/events/evt-1.md", fm).expect("parse");
        // Grouped by relation on disk, so the order comes back grouped.
        let mut want = e.clone();
        want.links = vec![
            EventLink::new("for", "Gigs/caa-2026"),
            EventLink::new("for", "[[Projects/CAA]]"),
            EventLink::new("venue", "[[The Hall]]"),
        ]
        .into();
        assert_eq!(parsed, want);
    }

    #[test]
    fn a_rewrite_keeps_the_body_the_type_and_foreign_properties() {
        let existing = "---\ntitle: Sunday Service\ntype: event\nexperience: setlist\n\
                        tags: [church]\nfor: \"[[Old Note]]\"\nstart: 2026-08-16T09:00:00Z\n\
                        end: 2026-08-16T11:00:00Z\n---\n\n# Sunday Service\n\n1. [[Rise and Shine]]\n";
        let (fm, _) = crate::scan::frontmatter_split(existing).unwrap();
        let mut e = crate::parse::parse_cal_event("Records/events/Sunday Service.md", fm).unwrap();
        e.start = "2026-08-16T10:00:00Z".into();
        e.links = vec![EventLink::new("for", "Gigs/sunday")].into();
        let page = merge_cal_event_page(&e, Some(existing)).unwrap();
        let (fm, body) = crate::scan::frontmatter_split(&page).unwrap();
        assert_eq!(body, "\n# Sunday Service\n\n1. [[Rise and Shine]]\n");
        let map: serde_yaml::Mapping = serde_yaml::from_str(fm).unwrap();
        assert_eq!(map.get("type").and_then(|v| v.as_str()), Some("event"));
        assert_eq!(map.get("experience").and_then(|v| v.as_str()), Some("setlist"));
        assert!(map.get("tags").is_some());
        let back = crate::parse::parse_cal_event("Records/events/Sunday Service.md", fm).unwrap();
        assert_eq!(back.start, "2026-08-16T10:00:00Z");
        // The old link is replaced, not kept beside the new one.
        assert_eq!(back.links.len(), 1);
        assert_eq!(back.links[0].target, "[[Gigs/sunday]]");
    }
}
