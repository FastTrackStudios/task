---
type: scheduling-schedule
id: work-hours
name: Default work hours
timezone: America/Chicago
rules:
  - days: [mon, tue, wed, thu, fri]
    start: "09:30"
    end: "12:30"
  - days: [mon, tue, wed, thu, fri]
    start: "13:30"
    end: "16:30"
  - days: [sat]
    start: "10:00"
    end: "13:00"
---

# Default work hours

Availability shared by all `event-types/*` that don't override their own
schedule. Matches Block 1 + Block 2 on weekdays (see [[weekday]]) plus
Saturday morning. Sundays are off.

Block 3 is intentionally NOT bookable — that's personal-project time.
