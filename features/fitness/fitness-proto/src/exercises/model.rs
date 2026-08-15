//! `Exercise` — typed view of one exercise page.
//!
//! Exercises live as markdown files under
//! `<vault>/Wiki/Exercises/<slug>.md` so they ride on the wiki
//! feature: bodies wikilink anatomy concepts (`[[pectoralis
//! major]]`), technique cues (`[[scapular retraction]]`), and
//! the wiki graph picks exercises up as just-another-page.
//!
//! Modeled on wger's `Exercise` + `Category` + `Muscle` +
//! `Equipment` + `ExerciseAlias` tables, flattened onto a
//! single page per exercise. Muscles + equipment are
//! free-form strings (with canonical enums naming the
//! recognized set) so custom values round-trip.

use chrono::{DateTime, Utc};
use facet::Facet;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Shared `Vec<String>` newtype — JSON column under the
/// SeaORM emission. Used for `aliases`, `primary_muscles`,
/// `secondary_muscles`, `equipment`, `instructions`, `tags`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct StringList(pub Vec<String>);

impl StringList {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<String>> for StringList {
    fn from(v: Vec<String>) -> Self {
        Self(v)
    }
}

impl FromIterator<String> for StringList {
    fn from_iter<I: IntoIterator<Item = String>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for StringList {
    type Target = Vec<String>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Serialize, Deserialize, Facet)]
#[architect(table_name = "exercises", repo)]
pub struct Exercise {
    #[serde(skip)]
    #[architect(filterable, sortable)]
    pub path: String,

    #[architect(primary_key, auto_increment = false, on_create = Uuid::new_v4())]
    pub id: Uuid,

    /// Canonical display name — `"Bench Press"`, `"Romanian
    /// Deadlift"`. Falls back to filename basename when
    /// missing.
    #[architect(filterable, sortable, fulltext)]
    pub name: String,

    /// Other names this exercise goes by — `"BB Bench"`,
    /// `"Flat Barbell Bench"`. Searches + recipe-style
    /// pantry-match fuzz hits these too.
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub aliases: StringList,

    /// One-line summary for index views.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub description: Option<String>,

    /// Movement category — `"chest"`, `"back"`, `"legs"`,
    /// `"shoulders"`, `"arms"`, `"core"`, `"cardio"`,
    /// `"mobility"`. Canonical set in [`Category`].
    #[serde(default = "default_category")]
    #[architect(filterable)]
    pub category: String,

    /// Primary muscles trained. Convention: wikilink the
    /// page-worthy ones so the wiki graph picks them up
    /// (`"[[pectoralis-major]]"`, `"[[triceps-brachii]]"`);
    /// raw strings are fine.
    #[serde(
        skip_serializing_if = "StringList::is_empty",
        default,
        rename = "primaryMuscles"
    )]
    #[architect(json)]
    pub primary_muscles: StringList,

    /// Synergist + stabilizer muscles.
    #[serde(
        skip_serializing_if = "StringList::is_empty",
        default,
        rename = "secondaryMuscles"
    )]
    #[architect(json)]
    pub secondary_muscles: StringList,

    /// Equipment required — `"barbell"`, `"dumbbell"`,
    /// `"cable"`, `"machine"`, `"bodyweight"`, `"band"`,
    /// `"kettlebell"`. Multiple entries fine. Canonical set
    /// in [`Equipment`].
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub equipment: StringList,

    /// `"compound"` (multi-joint — squat, bench, row) or
    /// `"isolation"` (single-joint — curl, leg extension).
    /// Free-form; canonical set in [`Mechanics`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub mechanics: Option<String>,

    /// `"push"` / `"pull"` / `"static"`. Free-form;
    /// canonical set in [`Force`].
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub force: Option<String>,

    /// Ordered technique cues. Each step is a markdown
    /// string; embed wikilinks freely.
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub instructions: StringList,

    /// Optional video — URL or vault-relative path.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "videoUrl")]
    pub video_url: Option<String>,

    /// Optional product picture / form-cue image.
    #[serde(skip_serializing_if = "Option::is_none", default, rename = "imageUrl")]
    pub image_url: Option<String>,

    /// Free-form tags — `"warmup"`, `"main-lift"`,
    /// `"accessory"`, `"hypertrophy"`. Drives queries.
    #[serde(skip_serializing_if = "StringList::is_empty", default)]
    #[architect(json)]
    pub tags: StringList,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateCreated"
    )]
    pub date_created: Option<DateTime<Utc>>,

    #[serde(
        skip_serializing_if = "Option::is_none",
        default,
        rename = "dateModified"
    )]
    pub date_modified: Option<DateTime<Utc>>,

    /// Markdown body — narrative, variations, common
    /// mistakes, anatomy notes, wikilinks.
    #[serde(skip)]
    pub details: String,
}

fn default_category() -> String {
    Category::Other.as_str().to_string()
}

/// Canonical movement categories. Parsing accepts any
/// string; unknown values round-trip as raw strings on
/// `Exercise::category`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Category {
    Chest,
    Back,
    Legs,
    Glutes,
    Shoulders,
    Arms,
    Core,
    Cardio,
    Mobility,
    Other,
}

impl Category {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Chest => "chest",
            Self::Back => "back",
            Self::Legs => "legs",
            Self::Glutes => "glutes",
            Self::Shoulders => "shoulders",
            Self::Arms => "arms",
            Self::Core => "core",
            Self::Cardio => "cardio",
            Self::Mobility => "mobility",
            Self::Other => "other",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "chest" | "push-chest" => Some(Self::Chest),
            "back" | "pull-back" => Some(Self::Back),
            "legs" | "leg" | "quads" | "hamstrings" => Some(Self::Legs),
            "glutes" | "hips" => Some(Self::Glutes),
            "shoulders" | "shoulder" | "delts" => Some(Self::Shoulders),
            "arms" | "biceps" | "triceps" => Some(Self::Arms),
            "core" | "abs" | "trunk" => Some(Self::Core),
            "cardio" | "conditioning" => Some(Self::Cardio),
            "mobility" | "stretching" | "warmup" => Some(Self::Mobility),
            _ => None,
        }
    }
}

/// Canonical equipment tags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Equipment {
    Barbell,
    Dumbbell,
    Cable,
    Machine,
    Bodyweight,
    Band,
    Kettlebell,
    Bench,
    PullUpBar,
    Sled,
    Cardio,
    Other,
}

impl Equipment {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Barbell => "barbell",
            Self::Dumbbell => "dumbbell",
            Self::Cable => "cable",
            Self::Machine => "machine",
            Self::Bodyweight => "bodyweight",
            Self::Band => "band",
            Self::Kettlebell => "kettlebell",
            Self::Bench => "bench",
            Self::PullUpBar => "pull-up-bar",
            Self::Sled => "sled",
            Self::Cardio => "cardio",
            Self::Other => "other",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "barbell" | "bb" => Some(Self::Barbell),
            "dumbbell" | "db" => Some(Self::Dumbbell),
            "cable" | "cables" | "pulley" => Some(Self::Cable),
            "machine" => Some(Self::Machine),
            "bodyweight" | "bw" => Some(Self::Bodyweight),
            "band" | "resistance-band" => Some(Self::Band),
            "kettlebell" | "kb" => Some(Self::Kettlebell),
            "bench" => Some(Self::Bench),
            "pull-up-bar" | "pullup-bar" | "chin-up-bar" => Some(Self::PullUpBar),
            "sled" => Some(Self::Sled),
            "cardio" | "treadmill" | "bike" | "rower" | "elliptical" => Some(Self::Cardio),
            "other" => Some(Self::Other),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Mechanics {
    Compound,
    Isolation,
}

impl Mechanics {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Compound => "compound",
            Self::Isolation => "isolation",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "compound" | "multi-joint" => Some(Self::Compound),
            "isolation" | "single-joint" => Some(Self::Isolation),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Force {
    Push,
    Pull,
    Static,
    Hinge,
    Squat,
    Carry,
}

impl Force {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Push => "push",
            Self::Pull => "pull",
            Self::Static => "static",
            Self::Hinge => "hinge",
            Self::Squat => "squat",
            Self::Carry => "carry",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "push" => Some(Self::Push),
            "pull" => Some(Self::Pull),
            "static" | "isometric" | "hold" => Some(Self::Static),
            "hinge" | "deadlift" => Some(Self::Hinge),
            "squat" => Some(Self::Squat),
            "carry" | "loaded-carry" => Some(Self::Carry),
            _ => None,
        }
    }
}

/// Client-side optimistic cache identity (`architect::Store`): keyed by
/// the stable `id`.
#[cfg(feature = "atom")]
impl architect::StoreEntity for Exercise {
    type Key = uuid::Uuid;
    fn key(&self) -> uuid::Uuid {
        self.id
    }
}
