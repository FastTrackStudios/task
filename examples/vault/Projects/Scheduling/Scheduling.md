---
title: Scheduling
type: project
folder: "[[Projects]]"
---

# Scheduling

Cal.com-shape booking config. Event types, availability schedules, and
day templates live here; bookings (the history) live in
`[[Records/bookings]]`.

- `event-types/` — bookable meeting types (`30min-consult.md`, …).
- `schedules/` — availability windows (`work-hours.md`, …).
- `templates/` — day templates (`weekday.md`, `weekend.md`).

Locations referenced by event-types live in `[[Operations/Locations]]`.

Parsed by the `scheduling` crate (`features/scheduling/scheduling`).
