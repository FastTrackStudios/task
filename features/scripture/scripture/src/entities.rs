//! Parse the BradyStephenson **BibleData** entity tables (CC BY 4.0) —
//! people and places with the verses that mention them.
//!
//! This is *only the parser*. Biblical entities live as **wiki pages**
//! (the wiki's `type: entity`), so the install path turns each [`Entity`]
//! into a `<org>/wiki/Knowledge/Entities/<Name>.md` page whose body links
//! the verses (`[[Genesis 1:1]]`) — the existing wiki + link/graph
//! machinery then handles them. We don't add a parallel entity store.
//!
//! Source: normalized CSVs `Person` + `PersonVerse`, `Place` +
//! `PlaceVerse`; references are `BOOK c:v` (e.g. `GEN 1:1`).

use scripture_proto::VerseId;

/// One parsed person or place with its verse references (OSIS).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    /// BibleData id, e.g. `Adam_1` / `heaven_1`.
    pub id: String,
    pub name: String,
    /// `person` or `place`.
    pub kind: String,
    pub description: String,
    /// Person sex, or place type.
    pub attribute: String,
    /// OSIS ids of the verses mentioning the entity, canonical order.
    pub refs: Vec<String>,
}

/// Normalize a `BOOK c:v` reference to OSIS (`GEN 1:1` → `Gen.1.1`).
fn osis(reference: &str) -> Option<String> {
    VerseId::parse(reference).ok().map(|id| id.osis())
}

/// Build entities from the four BibleData CSV files (raw text), each
/// carrying its (deduped, canonically-sorted) verse references.
#[must_use]
pub fn from_bible_data(
    persons_csv: &str,
    person_verse_csv: &str,
    places_csv: &str,
    place_verse_csv: &str,
) -> Vec<Entity> {
    use std::collections::BTreeMap;
    let mut map: BTreeMap<String, Entity> = BTreeMap::new();

    for r in rows(persons_csv) {
        // person_id, person_name, surname, unique_attribute, sex, …
        if let (Some(id), Some(name)) = (r.first(), r.get(1)) {
            map.insert(
                id.clone(),
                Entity {
                    id: id.clone(),
                    name: name.clone(),
                    kind: "person".into(),
                    description: r.get(3).cloned().unwrap_or_default(),
                    attribute: r.get(4).cloned().unwrap_or_default(),
                    refs: Vec::new(),
                },
            );
        }
    }
    for r in rows(places_csv) {
        // place_id, place_name, place_type, modern_equivalent, place_notes, …
        if let (Some(id), Some(name)) = (r.first(), r.get(1)) {
            map.insert(
                id.clone(),
                Entity {
                    id: id.clone(),
                    name: name.clone(),
                    kind: "place".into(),
                    description: r.get(4).cloned().unwrap_or_default(),
                    attribute: r.get(2).cloned().unwrap_or_default(),
                    refs: Vec::new(),
                },
            );
        }
    }
    // *Verse tables: reference_id is col 1, entity id is col 3.
    for csv in [person_verse_csv, place_verse_csv] {
        for r in rows(csv) {
            if let (Some(reference), Some(id)) = (r.get(1), r.get(3)) {
                if let (Some(e), Some(o)) = (map.get_mut(id), osis(reference)) {
                    if !e.refs.contains(&o) {
                        e.refs.push(o);
                    }
                }
            }
        }
    }
    let mut out: Vec<Entity> = map.into_values().collect();
    for e in &mut out {
        e.refs
            .sort_by_key(|r| VerseId::parse(r).map(|v| v.numeric()).unwrap_or(0));
    }
    out
}

/// Read a CSV (with header) into owned string rows.
fn rows(csv: &str) -> Vec<Vec<String>> {
    csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(true)
        .from_reader(csv.as_bytes())
        .records()
        .filter_map(Result::ok)
        .map(|rec| rec.iter().map(str::to_string).collect())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_persons_places_and_refs() {
        let persons = "person_id,person_name,surname,unique_attribute,sex\n\
                       Adam_1,Adam,,first man,male\n";
        let person_verse = "person_verse_id,reference_id,person_label_id,person_id\n\
                            x,GEN 1:1,y,Adam_1\n\
                            x,GEN 2:19,y,Adam_1\n";
        let places = "place_id,place_name,place_type,modern_equivalent,place_notes\n\
                      Eden_1,Eden,region,,the garden\n";
        let place_verse = "place_verse_id,reference_id,place_label_id,place_id\n\
                           x,GEN 2:8,y,Eden_1\n";

        let ents = from_bible_data(persons, person_verse, places, place_verse);
        assert_eq!(ents.len(), 2);
        let adam = ents.iter().find(|e| e.id == "Adam_1").unwrap();
        assert_eq!(adam.name, "Adam");
        assert_eq!(adam.kind, "person");
        assert_eq!(adam.attribute, "male");
        assert_eq!(adam.refs, vec!["Gen.1.1", "Gen.2.19"]); // OSIS, sorted
        let eden = ents.iter().find(|e| e.id == "Eden_1").unwrap();
        assert_eq!(eden.kind, "place");
        assert_eq!(eden.refs, vec!["Gen.2.8"]);
    }
}
