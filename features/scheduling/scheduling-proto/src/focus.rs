//! What an hour is *for*: the tasks a time block brings forward.
//!
//! A day plan says "16:00–17:00 is the maintenance hour". That only
//! matters if, during that hour, the task list leads with errands and
//! chores instead of whatever happens to be due. This module is the one
//! place that knows which kinds of work each [`BlockCategory`] is for,
//! as context / tag names; the task side
//! (`task_proto::RelevanceContext::focus`) opens those gates and ranks
//! those tasks first, without knowing anything about schedules.
//!
//! Names carry no sigil and are lowercase. A task matches by `@context`
//! or by `#tag`, so `@errands`, `#errand` and `#chore` all land in a
//! maintenance hour — both spellings are listed because people write
//! both.

use crate::day_plan::PlannedBlock;
use crate::resolve::day_span;
use crate::time_block::BlockCategory;

impl BlockCategory {
    // t[impl process.focus.surfaces]
    /// The context / tag names a block of this kind brings forward.
    /// Empty for blocks that steer nothing (sleep, an open slot whose
    /// assignment says what it holds, `Other`).
    #[must_use]
    pub const fn surfaces(self) -> &'static [&'static str] {
        match self {
            Self::Maintenance => &[
                "errands",
                "errand",
                "chores",
                "chore",
                "home",
                "admin",
                "maintenance",
            ],
            Self::Meal => &["mealprep", "kitchen"],
            Self::Exercise => &["exercise", "health"],
            Self::Spiritual => &["spiritual", "prayer", "journal"],
            Self::Reset | Self::Hygiene => &["morning"],
            Self::WindDown => &["evening"],
            Self::Allocatable | Self::Sleep | Self::Other => &[],
        }
    }

    /// How the hour is named on the task list ("Maintenance hour"),
    /// for kinds that steer it.
    #[must_use]
    pub const fn focus_label(self) -> Option<&'static str> {
        match self {
            Self::Maintenance => Some("Maintenance hour"),
            Self::Meal => Some("Meal time"),
            Self::Exercise => Some("Exercise"),
            Self::Spiritual => Some("Quiet time"),
            Self::Reset => Some("Morning reset"),
            Self::Hygiene => Some("Getting ready"),
            Self::WindDown => Some("Wind-down"),
            Self::Allocatable | Self::Sleep | Self::Other => None,
        }
    }

    /// The kind a [`focus_label`](Self::focus_label) names — how a
    /// running "Maintenance hour" timer session is read back as the
    /// mode it started.
    #[must_use]
    pub fn from_focus_label(label: &str) -> Option<Self> {
        const STEERING: [BlockCategory; 7] = [
            BlockCategory::Maintenance,
            BlockCategory::Meal,
            BlockCategory::Exercise,
            BlockCategory::Spiritual,
            BlockCategory::Reset,
            BlockCategory::Hygiene,
            BlockCategory::WindDown,
        ];
        let label = label.trim();
        STEERING.into_iter().find(|c| {
            c.focus_label()
                .is_some_and(|l| l.eq_ignore_ascii_case(label))
        })
    }
}

// t[impl process.focus.steering]
/// The block running at `minute` (minutes since midnight) that steers
/// the task list — one whose category [`surfaces`](BlockCategory::surfaces)
/// something. Where blocks overlap, a fixed one wins, then the one that
/// started last (the nearer, more specific commitment).
#[must_use]
pub fn steering_block_at(blocks: &[PlannedBlock], minute: u16) -> Option<&PlannedBlock> {
    blocks
        .iter()
        .filter(|b| !b.category.surfaces().is_empty())
        .filter(|b| {
            let (s, e) = day_span(b.start, b.end);
            minute >= s && minute < e
        })
        .max_by_key(|b| (b.fixed, b.start.minutes_since_midnight))
}

/// The task a block running at `minute` was planned for — the calendar's
/// say on what to start now ("09:00 Studio block: Comp lead vocals").
/// Returns the block and the assigned task's id; the same overlap rule as
/// [`steering_block_at`].
#[must_use]
pub fn planned_task_at(blocks: &[PlannedBlock], minute: u16) -> Option<(&PlannedBlock, &str)> {
    blocks
        .iter()
        .filter(|b| {
            let (s, e) = day_span(b.start, b.end);
            minute >= s && minute < e
        })
        .filter_map(|b| {
            let a = b.assignment.as_ref().filter(|a| a.kind == "task")?;
            Some((b, a.ref_id.as_deref()?))
        })
        .max_by_key(|(b, _)| (b.fixed, b.start.minutes_since_midnight))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::time_block::{TimeBlockId, TimeOfDay};

    fn block(start: (u8, u8), end: (u8, u8), category: BlockCategory) -> PlannedBlock {
        PlannedBlock {
            id: TimeBlockId(format!("{start:?}")),
            start: TimeOfDay::new(start.0, start.1),
            end: TimeOfDay::new(end.0, end.1),
            label: format!("{category:?}"),
            category,
            note: None,
            assignment: None,
            fixed: false,
        }
    }

    // t[verify process.focus.steering]
    // t[verify process.focus.surfaces]
    #[test]
    fn the_maintenance_hour_steers_only_while_it_runs() {
        let day = vec![
            block((9, 0), (12, 0), BlockCategory::Allocatable),
            block((16, 0), (17, 0), BlockCategory::Maintenance),
        ];
        assert!(
            steering_block_at(&day, 10 * 60).is_none(),
            "work blocks steer nothing"
        );
        let hour = steering_block_at(&day, 16 * 60 + 30).expect("in the hour");
        assert_eq!(hour.category, BlockCategory::Maintenance);
        assert!(hour.category.surfaces().contains(&"errands"));
        assert!(
            steering_block_at(&day, 17 * 60).is_none(),
            "end is exclusive"
        );
    }

    // t[verify process.focus.steering]
    #[test]
    fn overlapping_blocks_prefer_fixed_then_latest_start() {
        let mut meal = block((12, 0), (13, 0), BlockCategory::Meal);
        let chores = block((12, 30), (13, 30), BlockCategory::Maintenance);
        let day = vec![meal.clone(), chores.clone()];
        assert_eq!(steering_block_at(&day, 12 * 60 + 45), Some(&chores));
        meal.fixed = true;
        let day = vec![meal.clone(), chores];
        assert_eq!(steering_block_at(&day, 12 * 60 + 45), Some(&meal));
    }

    #[test]
    // t[verify process.focus.mode]
    fn a_focus_label_reads_back_as_its_kind() {
        assert_eq!(
            BlockCategory::from_focus_label("maintenance hour "),
            Some(BlockCategory::Maintenance)
        );
        assert_eq!(BlockCategory::from_focus_label("Mix track two"), None);
    }
}
