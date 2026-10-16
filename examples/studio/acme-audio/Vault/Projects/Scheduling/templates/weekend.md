---
type: scheduling-day-template
id: weekend
name: Weekend
description: Sat–Sun — a slow start, a longer maintenance stretch, and the afternoon off.
blocks:
  - id: morning-reset
    label: Morning reset
    start: "08:00"
    end: "08:30"
    category: reset
  - id: breakfast
    label: Breakfast
    start: "08:30"
    end: "09:15"
    category: meal
  - id: maintenance
    label: Maintenance
    start: "10:00"
    end: "12:00"
    category: maintenance
    note: The week's errands, in one run
  - id: lunch
    label: Lunch
    start: "12:30"
    end: "13:30"
    category: meal
  - id: free
    label: Free time
    start: "13:30"
    end: "18:00"
    category: other
  - id: dinner
    label: Dinner
    start: "18:30"
    end: "19:30"
    category: meal
---

# Weekend

The default for Saturdays and Sundays. The maintenance stretch is
longer than the weekday's hour because the errands that need shops to
be open collect here.
