//! Calendar-event frontmatter → `CalEvent`.
//!
//! Every key in [`CalEvent::RESERVED_KEYS`] is the event's own. Any
//! other key whose value is a wikilink, or a list of nothing but
//! wikilinks, is a link relation (`for: ["[[Gigs/caa-2026]]"]`). The
//! rest are properties somebody else put there — the writer keeps
//! them (see `write::cal_event::merge_cal_event_page`).

use scheduling_proto::{CalEvent, EventLink, EventLinks, ExternalRef};

use super::ParseError;
use super::yaml::{parse_mapping, require_str, take_bool, take_str};

pub fn parse_cal_event(path: &str, frontmatter_yaml: &str) -> Result<CalEvent, ParseError> {
    let map = parse_mapping(frontmatter_yaml)?;
    let id = take_str(&map, "id").unwrap_or_else(|| {
        std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(path)
            .to_string()
    });
    Ok(CalEvent {
        id,
        title: take_str(&map, "title").unwrap_or_default(),
        start: require_str(&map, "start")?,
        end: require_str(&map, "end")?,
        all_day: take_bool(&map, "all_day").unwrap_or(false),
        color: take_str(&map, "color").unwrap_or_else(|| "primary".into()),
        description: take_str(&map, "description"),
        recurrence: take_str(&map, "recurrence"),
        location: take_str(&map, "location"),
        links: link_properties(&map),
        external_ref: take_str(&map, "external_ref").and_then(|s| ExternalRef::parse(&s)),
        tombstone: take_str(&map, "tombstone"),
    })
}

/// The link relations on an event file, in frontmatter order.
fn link_properties(map: &serde_yaml::Mapping) -> EventLinks {
    let mut out = Vec::new();
    for (k, v) in map {
        let Some(key) = k.as_str() else { continue };
        if !EventLink::valid_relation(key) {
            continue;
        }
        if let Some(targets) = wikilink_values(v) {
            out.extend(targets.into_iter().map(|t| EventLink::new(key, &t)));
        }
    }
    out.into()
}

/// `Some(targets)` when `v` is a wikilink or a non-empty list of
/// nothing but wikilinks; `None` for anything else.
pub(crate) fn wikilink_values(v: &serde_yaml::Value) -> Option<Vec<String>> {
    fn one(v: &serde_yaml::Value) -> Option<String> {
        let s = v.as_str()?.trim();
        (s.starts_with("[[") && s.ends_with("]]") && s.len() > 4).then(|| s.to_owned())
    }
    match v {
        serde_yaml::Value::Sequence(items) if !items.is_empty() => {
            items.iter().map(one).collect::<Option<Vec<_>>>()
        }
        other => one(other).map(|s| vec![s]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wikilink_properties_are_links_and_other_properties_are_not() {
        let fm = "type: event\nstart: 2026-11-06T19:00:00Z\nend: 2026-11-06T22:00:00Z\n\
                  for: \"[[Gigs/caa-2026]]\"\nvenue:\n  - \"[[The Hall]]\"\n  - \"[[Side Room]]\"\n\
                  experience: setlist\nmixed:\n  - \"[[A]]\"\n  - plain\n\
                  external_ref: pco:plan_time:123\ntombstone: 2026-10-21T00:00:00Z\n";
        let e = parse_cal_event("Records/events/Show.md", fm).expect("parse");
        assert_eq!(e.id, "Show");
        let links: Vec<(&str, &str)> = e
            .links
            .iter()
            .map(|l| (l.relation.as_str(), l.target.as_str()))
            .collect();
        assert_eq!(
            links,
            vec![
                ("for", "[[Gigs/caa-2026]]"),
                ("venue", "[[The Hall]]"),
                ("venue", "[[Side Room]]"),
            ]
        );
        let r = e.external_ref.expect("external_ref");
        assert_eq!((r.system.as_str(), r.id.as_str()), ("pco", "plan_time:123"));
        assert_eq!(e.tombstone.as_deref(), Some("2026-10-21T00:00:00Z"));
    }
}
