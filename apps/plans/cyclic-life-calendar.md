# Cyclic Life Calendar

**Status:** shipped — `features/task/cycle/cycle` implements the model and cites this file as its design rationale. Kept as the rationale doc.

A planning system that reshapes the year into uniform 28-day cycles
instead of the calendar's irregular 28/30/31-day months. Built around
Cody's stated preferences but applicable to anyone who wants
predictable-shape planning periods.

Inspired by the user's two-year experience switching off monthly
planning. The benefits captured here are *theirs*, not ours; the design
exists to support that workflow with first-class entities + tooling.

---

## Why not months?

Monthly planning has structural problems:

- **Different lengths.** A goal that takes "one month" lands on
  28 / 29 / 30 / 31 days depending on which month — same effort,
  different time budgets.
- **Different start days.** A month starting on a Wednesday changes
  which weekday the midpoint lands on, which affects reflection /
  review routines.
- **Different weekday counts.** One month has five Sundays, the next
  has four. Output expectations can't be uniform if available time
  isn't.
- **Time perception** is awkward: the 10th of a month is one-third
  through, the 15th is half through, the 20th is two-thirds — the
  gradient compresses unevenly.

For habit-building and routine-anchored work, that variance is the
opposite of what's helpful.

## The cyclic structure

```
Year (52 weeks)
├── Quarter 1   (13 weeks)
│   ├── Cycle 1.1   (4 weeks)
│   ├── Cycle 1.2   (4 weeks)
│   ├── Cycle 1.3   (4 weeks)
│   └── Reset week  (1 week)
├── Quarter 2   (13 weeks)
│   ├── Cycle 2.1   (4 weeks)
│   ├── Cycle 2.2   (4 weeks)
│   ├── Cycle 2.3   (4 weeks)
│   └── Reset week  (1 week)
├── Quarter 3   (13 weeks)
│   └── … (same shape)
└── Quarter 4   (13 weeks)
    └── … (same shape)
```

4 quarters × (3 cycles × 4 weeks + 1 reset week) = 4 × 13 = **52 weeks**
= 364 days. The remaining 1–2 days per year accumulate into a
**bonus week** every ~5 years (Cody's Monday-start: 2026 → 2032 → 2037
…) used as "week zero" for the following year — a longer reset / prep
window.

### Properties

| Property | Cyclic | Monthly |
|---|---|---|
| Period length | always 28 days | 28 / 29 / 30 / 31 |
| Day-of-week each period starts | always the same (Mon for Cody) | varies |
| Midpoint day-of-week | always weekend (Sun) for Mon-start | varies |
| End day-of-week | always weekend | varies |
| Weekday count per period | always 4 of each | varies (4 or 5) |
| Each week as % of period | always exactly 25% | 23 % – 25 % |

The "always" column is what makes habit-building tractable: routines
land on the same weekday every cycle, midpoint reflection lands on a
weekend, and a week is a clean quarter of a period.

### Reset weeks

The 13th week of each quarter is **reset week** — explicitly *not* a
planning cycle. Used for:

- Refreshing spaces (declutter / clean / set up for the new cycle)
- Reviewing the quarter that was — what worked, what didn't
- Reviewing progress against quarterly / yearly goals
- Preparing for the quarter ahead — high-level intent, energy budget,
  one-off setup tasks

It's deliberate slack. Treating it as a working cycle defeats its
purpose.

### Week numbering

- **Week 1** of the year = the first week with **at least 4 days** in
  the new calendar year (Cody's convention — others may pick week
  containing Jan 1).
- For Cody / Monday-start / 2026: Week 1 = **Dec 29, 2025 – Jan 4,
  2026**.
- Cycles run Mon → Sun; "weekend" = Sat + Sun is the natural review
  surface.

### Cyclic leap years

Years where the four quarters plus their accumulated drift have at
least 4 days left over get an extra **week zero** before the next
year's Cycle 1.1. For Monday-start: 2026, 2032, 2037 (computed from
Jan 1 day-of-week + accumulated 364-day shortfalls).

The bonus week's role is yours to define — Cody uses it as a longer
reset / prep window for the year ahead. The cyclic calendar itself
doesn't change shape; the bonus week sits between the previous Q4
reset week and the next year's Q1 Cycle 1.1.

---

## Mapping into Task entities

### Today

- `task::TaskInfo` has `recurrence: Option<String>` (RRULE) +
  `recurrence_anchor: Option<String>` (`"scheduled" | "completion"`) +
  `complete_instances: StringList`. Daily / weekly / monthly habits
  fit this surface as-is.
- `project::ProjectInfo` now has `parent_id: Option<Uuid>` —
  hierarchical decomposition works for any planning system, including
  cyclic.
- `goal::Goal` (planned, separate PR) gets a typed `kind` and
  `target_date` so a goal can be tagged "this cycle" / "this quarter"
  / "this year" / "lifetime".

### Cyclic primitives (planned)

Two new entities make the cycle structure first-class:

```rust
struct Cycle {
    id: Uuid,
    year: u16,        // e.g. 2026
    quarter: u8,      // 1..=4
    ordinal: u8,      // 1..=3 (which cycle of the quarter)
    start_date: NaiveDate,
    end_date: NaiveDate,
    week_start: Weekday,  // for now always Monday
    // Reset week is encoded as Quarter.reset_week_*, not its own
    // Cycle — keeps the type honest (resets aren't planning units).
}

struct Quarter {
    id: Uuid,
    year: u16,
    ordinal: u8,            // 1..=4
    start_date: NaiveDate,
    end_date: NaiveDate,
    reset_week_start: NaiveDate,
    reset_week_end: NaiveDate,
    bonus_week_start: Option<NaiveDate>,  // cyclic-leap-year only
    bonus_week_end: Option<NaiveDate>,
}
```

`Cycle` joins to:
- `TaskInfo.scheduled / due` — tasks owned by a cycle.
- `Goal.target_date` — milestones land in a specific cycle.
- A future `Reflection` entity — what the cycle yielded.

### Calendar generation

Pure-function generator. Given:
- `year_start_day: Weekday` (Mon)
- `first_week_anchor: FirstWeekRule` (≥4-days-in-year)
- `start_year: i32`

…produces the full `Vec<Quarter>` (each with three `Cycle`s + reset +
optional bonus). Same fn that produces the in-doc dates above. No
external dep; chrono's already in workspace.

### CLI surface (planned)

```
task cycle current             # which cycle are we in right now
task cycle list --year 2026    # all quarters/cycles/reset weeks
task cycle goals               # goals scoped to the current cycle
task cycle reflect             # capture this cycle's reflection
```

### Wiki integration

- `wiki/Knowledge/cycles/2026-Q1-Cycle-1.md` — durable cycle pages
  (notes, learnings, what worked). Curated tier.
- `wiki/LLM/Journals/<cycle-id>/...` — agent-written daily journals
  scoped to a cycle (loose tier). Tied to the cycle by id reference.

---

## Phases

1. **Doc-only (this plan).** Cyclic-life-calendar concept written
   down; no entities, no code. Vault stays usable with manual
   cycle-tagged tasks.
2. **Generator + entities.** `Cycle` + `Quarter` types + the
   pure-function date generator. `task cycle current` works against
   the generated calendar.
3. **UI overlays.** view-calendar renders cycle / reset-week
   boundaries as backgrounds; week labels switch from ISO week to
   "Q1 C1 W2 (4/4)". Habit views surface streaks against cycle
   structure.
4. **Reflection capture.** Per-cycle reflection prompts on the
   weekend before the next cycle starts; auto-creates a page under
   `wiki/Knowledge/cycles/`.

Phase 1 is *this document*. Phase 2 onward is committed-to work but
not yet scheduled.

## Non-goals

- Forcing this on anyone. Cyclic planning is opinionated; the rest of
  the Task feature set works without it.
- Replacing months in the underlying calendar. iCal / Google
  Calendar / cal.com still see months; cycles are a logical overlay
  in Task's UI + DB.
- Mandating Cody's start day / first-week rule. Both are configurable
  per user.
