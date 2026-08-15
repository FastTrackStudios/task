---
title: Daily Plan
type: synthesis
tags: [project, scheduling, habit]
sources: ["scheduling/templates/weekday.md", "scheduling/templates/weekend.md"]
status: active
priority: 0
folder: "[[Projects]]"
---

# Daily Plan

The fixed day shape Cody uses to allocate energy. Built on the
[[scheduling]] feature — every block is a `scheduling-day-template`
entry under [[weekday]] / [[weekend]], not a free-form note.

## Why a fixed shape

Three load-bearing constraints, in priority order:

1. **7.5 hours of sleep, every night.** Block 3 ends at 22:00 so wind-down
   can start at 22:00 sharp.
2. **30 min of dedicated spiritual time, every morning.** Anchors the day
   before anything else can claim the slot.
3. **3 full home-cooked meals.** Forces grocery + pantry discipline — see
   [[pantry]] and the [[mealplan]] week.

Everything else fits around those three.

## Block budget

Weekdays produce **9.5 allocatable hours** spread across three blocks:

| Block       | Window          | Default use                         |
| ----------- | --------------- | ----------------------------------- |
| **Block 1** | 09:30 – 12:30   | Deep work — hardest task of the day |
| **Block 2** | 13:30 – 16:30   | Meetings + collaborative work       |
| **Block 3** | 19:00 – 22:00   | Personal projects, study, music     |

Weekends ([[weekend]]) drop to two longer blocks and skip the
maintenance hour.

## Booking against the plan

Only Block 1 + Block 2 are exposed for external booking — see
[[work-hours]]. Block 3 is intentionally reserved.

Live event types:

- [[30min-consult]] — short intro calls
- [[60min-mentor]] — longer working sessions
- [[45min-in-person]] — unpublished, sent on request

Confirmed bookings live under `scheduling/bookings/` (e.g.
[[2026-06-01-alice]], [[2026-06-03-ben]]). Cancellations are kept on
disk for audit, see [[2026-05-29-cancelled]].

## Ported from

Originally a markdown table in The Observatory's `Daily Plan.md`.
Lifting it into the scheduling feature gives it real structure: each
block is queryable, bookings can resolve against the available
sub-windows, and the colocated `.base` view in
[[Journal/Daily/daily-plan-blocks.base]] can render the whole day inline.
