---
type: scheduling-day-template
id: weekend
name: Weekend
description: Sat–Sun — a slow start, the week's errands in one run, and the rest of the day free.
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
  - id: block-1
    label: Free time
    start: "13:30"
    end: "18:00"
    category: allocatable
  - id: dinner
    label: Dinner
    start: "18:30"
    end: "19:30"
    category: meal
  - id: wind-down
    label: Wind-down
    start: "21:30"
    end: "22:30"
    category: winddown
---

# Weekend

The default for Saturdays and Sundays: the maintenance stretch is longer
because the errands that need shops open collect here, and the
afternoon is free.
