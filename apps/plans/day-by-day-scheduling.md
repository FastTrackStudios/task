# Day-by-day scheduling — editable per-day plan with assignable blocks

**Status:** partially shipped — needs triage (2026-07-27). `DayPlan` / `DayTemplate` parse + write + `vault_scheduler` exist in `features/task/scheduling/`; the drag/resize editing UX this doc asks for was not verified.

**Goal:** the recurring daily-plan template (`weekday` / `weekend`
`DayTemplate`s, rendered today as read-only ghost outlines on the
calendar) becomes a **live, per-day plan** the user can rearrange and
fill: drag/resize blocks for a specific date, and assign tasks /
events / labels into blocks (especially the three allocatable
"Block 1/2/3" slots).

Agreed scope (from the user): **both** editable blocks (drag/resize
per-day) **and** droppable (drop tasks/events into blocks); and **all
three** assign modes are viable — drag a task onto a block, click a
block to pick/type, and plain text labels.

## Model

A `DayPlan` is a concrete, per-date instance derived from the matching
template. Untouched dates show the template (materialized on the fly);
a date the user edits gets its own saved `DayPlan`.

```rust
// scheduling-proto
pub struct PlannedBlock {
    pub id: TimeBlockId,
    pub start: TimeOfDay,
    pub end: TimeOfDay,
    pub label: String,
    pub category: BlockCategory,
    pub note: Option<String>,
    pub assignment: Option<BlockAssignment>, // what's in the block today
}

pub enum BlockAssignment {
    Label(String),                       // free text typed in
    Task { id: Uuid, title: String },    // a task note
    Project { id: Uuid, title: String }, // a project
}

pub struct DayPlan {
    pub date: String,                    // YYYY-MM-DD
    pub from_template: Option<DayTemplateId>,
    pub blocks: Vec<PlannedBlock>,
}
```

Persisted at `<vault>/Records/dayplans/<date>.md` (YAML frontmatter,
same round-trip shape as `DayTemplate`). `DayPlans` rpc service:
`get_day_plan(date) -> Option<DayPlan>` (None ⇒ UI materializes from
template), `upsert_day_plan(plan)`, `delete_day_plan(date)` (reset to
template). Mounted on the org vox router next to `DayTemplates`.

## Slices (ship in order)

1. **Foundation (this PR):** `DayPlan` / `PlannedBlock` /
   `BlockAssignment` proto types + parse/write round-trip in the
   `scheduling` crate + `DayPlans` service on `VaultScheduler` + mount
   on the server. Verifiable by a round-trip test + CLI/curl. No UI
   change yet.
2. **Rearrange:** make the calendar's plan-block layer editable —
   drag-to-move / drag-edge-to-resize per day, emitting an edit event.
   `/schedule` loads the `DayPlan` per visible date (saved or
   materialized) and upserts on edit. Delivers "rearrange a day".
3. **Assign:** drop a task/event onto a block (drag from a task list),
   click a block to pick a task/project or type a label, and inline
   label editing. Block renders its assignment. Delivers "assign things
   to blocks". Ties into the timer (track time against the assigned
   task).
4. **Polish:** "reset day to template", copy a day, per-day notes,
   week overview of allocatable usage.

## Notes
- Calendar events are still in-memory (no persistence) — orthogonal;
  the `DayPlan` persistence here is separate and lands first.
- The timer already exists; slice 3 can let a block's assigned task
  start a timer.
