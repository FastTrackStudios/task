//! Per-day plan — a concrete, editable instance of a day's schedule.
//!
//! A [`DayPlan`] is what the calendar's day-by-day editor reads and
//! writes. The recurring [`crate::DayTemplate`] gives the default
//! shape, but a date the user rearranges gets its own saved plan with
//! moved/resized blocks and per-block assignments. One markdown file
//! per date under `Records/dayplans/<date>.md`.

use facet::Facet;
use serde::{Deserialize, Serialize};

use crate::time_block::{BlockCategory, DayTemplateId, TimeBlockId, TimeOfDay};

/// What the user has put in a block for the day. Flat (a `kind`
/// discriminator + fields) rather than a data-carrying enum so it
/// round-trips cleanly through Facet / YAML.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct BlockAssignment {
    /// `"label"` (free text), `"task"`, or `"project"`.
    pub kind: String,
    /// Display text — the typed label, or the task / project title.
    pub title: String,
    /// The assigned task / project id (uuid string). `None` for a
    /// plain label.
    #[serde(default)]
    pub ref_id: Option<String>,
}

/// One block in a day plan — a [`crate::TimeBlock`] plus what's
/// assigned to it that day.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::JsonField, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
pub struct PlannedBlock {
    pub id: TimeBlockId,
    pub start: TimeOfDay,
    pub end: TimeOfDay,
    pub label: String,
    pub category: BlockCategory,
    pub note: Option<String>,
    /// `None` = nothing assigned (e.g. an empty allocatable slot).
    pub assignment: Option<BlockAssignment>,
    /// Immovable. Fixed blocks ("be at work at 10", a recurring
    /// church service) win every overlap — [`crate::resolve`]
    /// reflows the flexible blocks around them and never moves,
    /// shrinks, or drops a fixed block.
    #[serde(default)]
    pub fixed: bool,
}

/// `Vec<PlannedBlock>` newtype — a JSON column on [`DayPlan`].
/// Transparent so it round-trips as a plain array; `Deref` lets
/// callers treat it like the underlying `Vec`.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(
    architect::JsonField, Debug, Clone, Default, PartialEq, Eq, Facet, Serialize, Deserialize,
)]
#[repr(transparent)]
#[serde(transparent)]
pub struct PlannedBlocks(pub Vec<PlannedBlock>);

impl PlannedBlocks {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl From<Vec<PlannedBlock>> for PlannedBlocks {
    fn from(v: Vec<PlannedBlock>) -> Self {
        Self(v)
    }
}

impl FromIterator<PlannedBlock> for PlannedBlocks {
    fn from_iter<I: IntoIterator<Item = PlannedBlock>>(iter: I) -> Self {
        Self(iter.into_iter().collect())
    }
}

impl std::ops::Deref for PlannedBlocks {
    type Target = Vec<PlannedBlock>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for PlannedBlocks {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

/// One date's concrete plan. `date` (ISO `YYYY-MM-DD`) is the key.
#[cfg_attr(feature = "fake", derive(::fake::Dummy))]
#[derive(architect::Entity, Debug, Clone, PartialEq, Eq, Facet, Serialize, Deserialize)]
#[architect(table_name = "scheduling_day_plans", repo)]
pub struct DayPlan {
    /// PK — the vault file is `Records/dayplans/<date>.md`.
    #[architect(primary_key, auto_increment = false)]
    pub date: String,
    /// The template this plan was materialized from, for provenance.
    #[architect(json, filterable)]
    pub from_template: Option<DayTemplateId>,
    #[architect(json)]
    pub blocks: PlannedBlocks,
}
