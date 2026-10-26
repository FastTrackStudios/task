//! The decoded face of an app doc, and a catalog row carrying it.

use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// What a doc shows of itself: the `face` map at its root, decoded.
#[derive(Debug, Clone, Default, PartialEq, Facet, Serialize, Deserialize)]
pub struct DocFace {
    /// What a list shows. Empty when the face has none.
    pub title: String,
    /// A second line ("Harbor & Pine · 2026").
    pub subtitle: Option<String>,
    /// An accent, as the app wrote it (`"#22c55e"`). Not validated.
    pub colour: Option<String>,
    /// The property sheet, in the app's order.
    pub props: Vec<FaceProp>,
    /// Links to other docs, in the app's order.
    pub links: Vec<FaceLink>,
}

/// One row of a face's property sheet.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct FaceProp {
    /// The app's stable key (`"key"`, `"tempo"`).
    pub key: String,
    /// What a person reads. Falls back to `key` when the app gave none.
    pub label: String,
    /// How to render [`Self::value`].
    pub ty: FacePropType,
    pub value: FaceValue,
}

/// How a property renders. A face naming any other type has that
/// property skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Facet, Serialize, Deserialize)]
#[repr(u8)]
pub enum FacePropType {
    Text,
    Number,
    /// A length of time; the value is seconds (a number) or the app's
    /// own text ("4:12").
    Duration,
    /// A date or date-time, as the app wrote it (ISO 8601 by convention).
    Date,
    /// One of a set the app knows; rendered as a chip.
    Enum,
    Tags,
    Bool,
}

impl FacePropType {
    /// The word a face spells it with.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Duration => "duration",
            Self::Date => "date",
            Self::Enum => "enum",
            Self::Tags => "tags",
            Self::Bool => "bool",
        }
    }

    /// The type a face's word names; `None` for a word this build does
    /// not know (the property is then skipped).
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "text" => Self::Text,
            "number" => Self::Number,
            "duration" => Self::Duration,
            "date" => Self::Date,
            "enum" => Self::Enum,
            "tags" => Self::Tags,
            "bool" => Self::Bool,
            _ => return None,
        })
    }
}

/// A property's value, as the face stored it.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
#[repr(C)]
pub enum FaceValue {
    Text(String),
    Number(f64),
    Integer(i64),
    Bool(bool),
    Tags(Vec<String>),
}

impl FaceValue {
    /// The value in words, for a list or a diff ("120", "A, B").
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Text(s) => s.clone(),
            Self::Number(n) => n.to_string(),
            Self::Integer(n) => n.to_string(),
            Self::Bool(b) => if *b { "yes" } else { "no" }.to_owned(),
            Self::Tags(t) => t.join(", "),
        }
    }
}

/// A link from a face to another doc.
#[derive(Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct FaceLink {
    /// The app's word for the relation (`"plays"`, `"part-of"`).
    pub rel: String,
    /// What a person reads; may be empty.
    pub label: String,
    /// The target doc's id, from its `doc:<uuid>` reference.
    pub doc: Uuid,
}

/// A catalog row with its face: what a list of an org's docs shows.
#[derive(Debug, Clone, PartialEq, Facet, Serialize, Deserialize)]
pub struct DocCard {
    pub doc_id: Uuid,
    /// The app's word for what the doc is. Opaque to Task.
    pub kind: String,
    /// The catalog's name for it.
    pub name: String,
    /// What to show: the face's title, else the catalog's name.
    pub title: String,
    /// The catalog row's last change, microseconds since the Unix epoch.
    /// (An edit to the content does not move it.)
    pub updated_us: i64,
    /// The face, when the doc keeps one; `None` when it has none (or its
    /// content could not be read, which the span records).
    pub face: Option<DocFace>,
}

impl DocCard {
    /// A row and its face, titled by the face when it names one.
    #[must_use]
    pub fn new(
        doc_id: Uuid,
        kind: String,
        name: String,
        updated_us: i64,
        face: Option<DocFace>,
    ) -> Self {
        let title = face
            .as_ref()
            .map(|f| f.title.trim())
            .filter(|t| !t.is_empty())
            .map_or_else(|| name.clone(), str::to_owned);
        Self {
            doc_id,
            kind,
            name,
            title,
            updated_us,
            face,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_takes_the_face_title_else_the_name() {
        let id = Uuid::new_v4();
        let face = DocFace {
            title: "Reckless Love".into(),
            ..DocFace::default()
        };
        let card = DocCard::new(id, "k".into(), "untitled".into(), 0, Some(face));
        assert_eq!(card.title, "Reckless Love");
        let blank = DocFace {
            title: "  ".into(),
            ..DocFace::default()
        };
        assert_eq!(
            DocCard::new(id, "k".into(), "untitled".into(), 0, Some(blank)).title,
            "untitled"
        );
        assert_eq!(
            DocCard::new(id, "k".into(), "untitled".into(), 0, None).title,
            "untitled"
        );
    }

    #[test]
    fn every_type_word_round_trips() {
        for ty in [
            FacePropType::Text,
            FacePropType::Number,
            FacePropType::Duration,
            FacePropType::Date,
            FacePropType::Enum,
            FacePropType::Tags,
            FacePropType::Bool,
        ] {
            assert_eq!(FacePropType::parse(ty.as_str()), Some(ty));
        }
        assert_eq!(FacePropType::parse("link"), None);
    }
}
