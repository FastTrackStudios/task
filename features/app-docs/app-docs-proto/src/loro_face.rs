//! Reading and writing a face in a [`LoroDoc`].
//!
//! The reader is tolerant by contract: a field of the wrong type, a
//! property with no key or an unknown type, a link that names no doc —
//! each is skipped, and the rest of the face still reads. Nothing here
//! returns an error for what an app wrote.

use loro::{LoroDoc, LoroList, LoroMap, LoroResult, LoroValue};
use uuid::Uuid;

use crate::face::{DocFace, FaceLink, FaceProp, FacePropType, FaceValue};

/// The root map a face lives in.
pub const FACE: &str = "face";

/// The face `doc` keeps, or `None` when it keeps none (no `face` map, or
/// an empty one). Reads only the `face` map, never the rest of the doc.
#[must_use]
pub fn read_face(doc: &LoroDoc) -> Option<DocFace> {
    let LoroValue::Map(face) = doc.get_map(FACE).get_deep_value() else {
        return None;
    };
    if face.is_empty() {
        return None;
    }
    Some(DocFace {
        title: text(face.get("title")).unwrap_or_default(),
        subtitle: text(face.get("subtitle")),
        colour: text(face.get("colour")),
        props: list(face.get("props")).filter_map(prop).collect(),
        links: list(face.get("links")).filter_map(link).collect(),
    })
}

/// Write `face` into `doc`'s `face` map, replacing what was there: the
/// scalars are set (an absent optional removed), `props` and `links`
/// rebuilt in order. Does not commit: call it inside the transaction
/// that makes the change the face describes, then commit.
pub fn write_face(doc: &LoroDoc, face: &DocFace) -> LoroResult<()> {
    let map = doc.get_map(FACE);
    map.insert("title", face.title.as_str())?;
    set_or_delete(&map, "subtitle", face.subtitle.as_deref())?;
    set_or_delete(&map, "colour", face.colour.as_deref())?;
    let props = map.insert_container("props", LoroList::new())?;
    for (i, p) in face.props.iter().enumerate() {
        let row = props.insert_container(i, LoroMap::new())?;
        row.insert("key", p.key.as_str())?;
        row.insert("label", p.label.as_str())?;
        row.insert("type", p.ty.as_str())?;
        match &p.value {
            FaceValue::Text(s) => row.insert("value", s.as_str())?,
            FaceValue::Number(n) => row.insert("value", *n)?,
            FaceValue::Integer(n) => row.insert("value", *n)?,
            FaceValue::Bool(b) => row.insert("value", *b)?,
            FaceValue::Tags(tags) => {
                let list = row.insert_container("value", LoroList::new())?;
                for t in tags {
                    list.push(t.as_str())?;
                }
            }
        }
    }
    let links = map.insert_container("links", LoroList::new())?;
    for (i, l) in face.links.iter().enumerate() {
        let row = links.insert_container(i, LoroMap::new())?;
        row.insert("rel", l.rel.as_str())?;
        row.insert("label", l.label.as_str())?;
        row.insert("doc", format!("doc:{}", l.doc))?;
    }
    Ok(())
}

fn set_or_delete(map: &LoroMap, key: &str, value: Option<&str>) -> LoroResult<()> {
    match value {
        Some(v) => map.insert(key, v),
        None => map.delete(key),
    }
}

/// A string field; anything else is absent.
fn text(v: Option<&LoroValue>) -> Option<String> {
    match v {
        Some(LoroValue::String(s)) => Some(s.to_string()),
        _ => None,
    }
}

/// The entries of a list field; anything else is an empty list.
fn list(v: Option<&LoroValue>) -> impl Iterator<Item = &LoroValue> {
    let items: &[LoroValue] = match v {
        Some(LoroValue::List(l)) => l,
        _ => &[],
    };
    items.iter()
}

fn prop(v: &LoroValue) -> Option<FaceProp> {
    let LoroValue::Map(m) = v else { return None };
    let key = text(m.get("key")).filter(|k| !k.is_empty())?;
    let ty = FacePropType::parse(&text(m.get("type"))?)?;
    let value = value(m.get("value")?)?;
    let label = text(m.get("label"))
        .filter(|l| !l.is_empty())
        .unwrap_or_else(|| key.clone());
    Some(FaceProp {
        key,
        label,
        ty,
        value,
    })
}

fn value(v: &LoroValue) -> Option<FaceValue> {
    Some(match v {
        LoroValue::String(s) => FaceValue::Text(s.to_string()),
        LoroValue::Double(n) => FaceValue::Number(*n),
        LoroValue::I64(n) => FaceValue::Integer(*n),
        LoroValue::Bool(b) => FaceValue::Bool(*b),
        LoroValue::List(items) => {
            FaceValue::Tags(items.iter().filter_map(|t| text(Some(t))).collect())
        }
        _ => return None,
    })
}

fn link(v: &LoroValue) -> Option<FaceLink> {
    let LoroValue::Map(m) = v else { return None };
    let doc = text(m.get("doc"))?;
    let doc = Uuid::parse_str(doc.strip_prefix("doc:").unwrap_or(&doc)).ok()?;
    Some(FaceLink {
        rel: text(m.get("rel")).unwrap_or_default(),
        label: text(m.get("label")).unwrap_or_default(),
        doc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> DocFace {
        DocFace {
            title: "Reckless Love".into(),
            subtitle: Some("Harbor & Pine · 2026".into()),
            colour: Some("#22c55e".into()),
            props: vec![
                FaceProp {
                    key: "key".into(),
                    label: "Key".into(),
                    ty: FacePropType::Enum,
                    value: FaceValue::Text("A".into()),
                },
                FaceProp {
                    key: "tempo".into(),
                    label: "Tempo".into(),
                    ty: FacePropType::Number,
                    value: FaceValue::Number(120.5),
                },
                FaceProp {
                    key: "length".into(),
                    label: "Length".into(),
                    ty: FacePropType::Duration,
                    value: FaceValue::Integer(252),
                },
                FaceProp {
                    key: "live".into(),
                    label: "Live".into(),
                    ty: FacePropType::Bool,
                    value: FaceValue::Bool(true),
                },
                FaceProp {
                    key: "themes".into(),
                    label: "Themes".into(),
                    ty: FacePropType::Tags,
                    value: FaceValue::Tags(vec!["grace".into(), "love".into()]),
                },
            ],
            links: vec![FaceLink {
                rel: "plays".into(),
                label: "Sunday".into(),
                doc: Uuid::new_v4(),
            }],
        }
    }

    #[test]
    fn a_written_face_reads_back_whole() {
        let doc = LoroDoc::new();
        let face = sample();
        write_face(&doc, &face).unwrap();
        doc.commit();
        assert_eq!(read_face(&doc), Some(face));
    }

    #[test]
    fn a_face_survives_a_snapshot() {
        let doc = LoroDoc::new();
        write_face(&doc, &sample()).unwrap();
        doc.commit();
        let copy = LoroDoc::new();
        copy.import(&doc.export(loro::ExportMode::Snapshot).unwrap())
            .unwrap();
        assert_eq!(read_face(&copy), read_face(&doc));
    }

    #[test]
    fn rewriting_drops_an_optional_and_replaces_the_lists() {
        let doc = LoroDoc::new();
        write_face(&doc, &sample()).unwrap();
        let smaller = DocFace {
            title: "Reckless Love".into(),
            ..DocFace::default()
        };
        write_face(&doc, &smaller).unwrap();
        doc.commit();
        assert_eq!(read_face(&doc), Some(smaller));
    }

    #[test]
    fn a_doc_without_a_face_has_none() {
        let doc = LoroDoc::new();
        doc.get_map("song").insert("anything", 1).unwrap();
        doc.commit();
        assert_eq!(read_face(&doc), None);
    }

    #[test]
    fn wrong_shapes_and_unknown_types_are_skipped_not_errors() {
        let doc = LoroDoc::new();
        let target = Uuid::new_v4();
        let face = doc.get_map(FACE);
        face.insert("title", 42).unwrap(); // not a string: no title
        face.insert("subtitle", "Live at the Barn").unwrap();
        face.insert("colour", true).unwrap(); // not a string: no colour
        let props = face.insert_container("props", LoroList::new()).unwrap();
        props.push("not a map").unwrap();
        let ok = props.insert_container(1, LoroMap::new()).unwrap();
        ok.insert("key", "ccli").unwrap();
        ok.insert("type", "text").unwrap();
        ok.insert("value", "7084130").unwrap(); // no label: the key reads
        let unknown = props.insert_container(2, LoroMap::new()).unwrap();
        unknown.insert("key", "x").unwrap();
        unknown.insert("type", "hologram").unwrap();
        unknown.insert("value", "?").unwrap();
        let keyless = props.insert_container(3, LoroMap::new()).unwrap();
        keyless.insert("type", "text").unwrap();
        keyless.insert("value", "?").unwrap();
        let mixed = props.insert_container(4, LoroMap::new()).unwrap();
        mixed.insert("key", "tags").unwrap();
        mixed.insert("type", "tags").unwrap();
        let tags = mixed.insert_container("value", LoroList::new()).unwrap();
        tags.push("a").unwrap();
        tags.push(3).unwrap(); // not a string: dropped from the list
        let valueless = props.insert_container(5, LoroMap::new()).unwrap();
        valueless.insert("key", "empty").unwrap();
        valueless.insert("type", "text").unwrap();
        let links = face.insert_container("links", LoroList::new()).unwrap();
        let bad = links.insert_container(0, LoroMap::new()).unwrap();
        bad.insert("rel", "plays").unwrap();
        bad.insert("doc", "doc:not-a-uuid").unwrap();
        let bare = links.insert_container(1, LoroMap::new()).unwrap();
        bare.insert("doc", target.to_string()).unwrap(); // no `doc:` prefix
        face.insert("unknown", "ignored").unwrap();
        doc.commit();

        let read = read_face(&doc).expect("a face");
        assert_eq!(read.title, "");
        assert_eq!(read.subtitle.as_deref(), Some("Live at the Barn"));
        assert_eq!(read.colour, None);
        assert_eq!(
            read.props,
            vec![
                FaceProp {
                    key: "ccli".into(),
                    label: "ccli".into(),
                    ty: FacePropType::Text,
                    value: FaceValue::Text("7084130".into()),
                },
                FaceProp {
                    key: "tags".into(),
                    label: "tags".into(),
                    ty: FacePropType::Tags,
                    value: FaceValue::Tags(vec!["a".into()]),
                },
            ]
        );
        assert_eq!(
            read.links,
            vec![FaceLink {
                rel: String::new(),
                label: String::new(),
                doc: target,
            }]
        );
    }

    #[test]
    fn plain_values_read_like_containers() {
        // An app may write the lists as plain values, not containers.
        let doc = LoroDoc::new();
        let face = doc.get_map(FACE);
        face.insert("title", "Sunday").unwrap();
        let row: LoroValue = loro::LoroValue::Map(
            [
                ("key".to_owned(), LoroValue::from("venue")),
                ("type".to_owned(), LoroValue::from("text")),
                ("value".to_owned(), LoroValue::from("The Barn")),
            ]
            .into_iter()
            .collect::<std::collections::HashMap<_, _>>()
            .into(),
        );
        face.insert("props", vec![row]).unwrap();
        doc.commit();
        let read = read_face(&doc).unwrap();
        assert_eq!(read.props.len(), 1);
        assert_eq!(read.props[0].value, FaceValue::Text("The Barn".into()));
    }
}
