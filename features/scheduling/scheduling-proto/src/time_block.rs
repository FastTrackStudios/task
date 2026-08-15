//! Personal day-template types.
//!
//! A [`DayTemplate`] is an ordered list of [`TimeBlock`]s covering a
//! 24-hour day. The user's daily routine — the table from the
//! feature brief — round-trips through this shape. The vault stores
//! one template per markdown file (e.g.
//! `scheduling/templates/weekday.md`); per-day overrides live on
//! booking-style entries that point at a template + a date.

use facet::Facet;
use serde::{Deserialize, Serialize};

/// Stable id for a template (uuid v4 string, minted by the consumer
/// so the proto stays serializer-agnostic).
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct DayTemplateId(pub String);

impl From<String> for DayTemplateId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Stable id for a single block within a template.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct TimeBlockId(pub String);

impl From<String> for TimeBlockId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

/// Time of day as minutes-since-midnight. 0..=1440. We avoid
/// `chrono::NaiveTime` on the wire so callers don't have to depend
/// on chrono just to ship a payload.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
pub struct TimeOfDay {
    pub minutes_since_midnight: u16,
}

impl TimeOfDay {
    #[must_use]
    pub const fn new(hours: u8, minutes: u8) -> Self {
        Self {
            minutes_since_midnight: hours as u16 * 60 + minutes as u16,
        }
    }

    #[must_use]
    pub const fn hours(self) -> u8 {
        (self.minutes_since_midnight / 60) as u8
    }

    #[must_use]
    pub const fn minutes(self) -> u8 {
        (self.minutes_since_midnight % 60) as u8
    }
}

/// Coarse categorization of a block. Drives color + grouping in
/// the UI. The set tracks the brief's table — every block in the
/// example schedule maps to exactly one of these.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Copy, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(u8)]
pub enum BlockCategory {
    /// Wake-up routine, morning reset.
    Reset,
    /// Spiritual / journaling / meditation time.
    Spiritual,
    /// Cooking, eating, cleanup. Covers breakfast / lunch / dinner.
    Meal,
    /// Movement, gym, walks.
    Exercise,
    /// Personal hygiene, grooming, getting ready.
    Hygiene,
    /// One of the three allocatable Work / Event / Free-Time blocks.
    /// The brief calls these "Block 1 / 2 / 3" — the UI labels
    /// them by ordinal.
    Allocatable,
    /// Errands, chores, low-energy administrative work.
    Maintenance,
    /// Evening wind-down, no-screens, prep for bed.
    WindDown,
    /// Sleep.
    Sleep,
    /// Catch-all for blocks that don't fit the above. The UI
    /// renders them as a neutral chip.
    #[default]
    Other,
}

/// One row in the day template.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct TimeBlock {
    pub id: TimeBlockId,
    pub start: TimeOfDay,
    pub end: TimeOfDay,
    pub label: String,
    pub category: BlockCategory,
    /// Optional free-text note (e.g. "Block 1 — deep work" or
    /// "Block 3 — Tuesday only").
    pub note: Option<String>,
}

/// `Vec<TimeBlock>` newtype — a JSON column on [`DayTemplate`].
/// Transparent so it round-trips as a plain array in JSON / on the
/// wire; `Deref` lets callers treat it like the underlying `Vec`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct TimeBlocks(pub Vec<TimeBlock>);

impl TimeBlocks {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<TimeBlock>> for TimeBlocks {
    fn from(v: Vec<TimeBlock>) -> Self {
        Self(v)
    }
}

impl FromIterator<TimeBlock> for TimeBlocks {
    fn from_iter<I: IntoIterator<Item = TimeBlock>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for TimeBlocks {
    type Target = Vec<TimeBlock>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for TimeBlocks {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// Personal day template — an ordered list of blocks that should
/// cover the whole 24-hour day. The UI surfaces gaps + overlaps;
/// the proto leaves validation to the consumer so storing
/// partially-filled drafts works.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "scheduling_day_templates", repo)]
pub struct DayTemplate {
    /// Vault-relative path of the markdown file backing this
    /// template (populated by the scanner; not part of the wire
    /// write). PK — mirrors [`crate::EventType`].
    #[architect(primary_key, auto_increment = false)]
    pub path: String,
    /// Stable id (uuid string). The vault file is
    /// `Projects/Scheduling/templates/<id>.md`.
    #[architect(json, filterable)]
    pub id: DayTemplateId,
    #[architect(filterable, sortable, fulltext)]
    pub name: String,
    /// Optional description (e.g. "Weekday routine" /
    /// "Saturday — long-form work").
    #[architect(fulltext)]
    pub description: Option<String>,
    #[architect(json)]
    pub blocks: TimeBlocks,
}
