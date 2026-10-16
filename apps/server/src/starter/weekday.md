---
type: scheduling-day-template
id: weekday
name: Weekday
description: Mon–Fri — three blocks for the work that matters, an hour for the small jobs, and the meals between.
blocks:
  - id: morning-reset
    label: Morning reset
    start: "07:00"
    end: "07:30"
    category: reset
  - id: breakfast
    label: Breakfast
    start: "07:30"
    end: "08:00"
    category: meal
  - id: block-1
    label: Block 1
    start: "08:30"
    end: "11:30"
    category: allocatable
    note: Deep work — the thing that would win the day
  - id: lunch
    label: Lunch
    start: "12:00"
    end: "13:00"
    category: meal
  - id: block-2
    label: Block 2
    start: "13:00"
    end: "16:00"
    category: allocatable
  - id: maintenance
    label: Maintenance
    start: "16:00"
    end: "17:00"
    category: maintenance
    note: Errands, chores and admin — @errands and #chore work leads the list
  - id: dinner
    label: Dinner
    start: "18:00"
    end: "19:00"
    category: meal
  - id: block-3
    label: Block 3
    start: "19:00"
    end: "21:00"
    category: allocatable
    note: Free time, or whatever the day still needs
  - id: wind-down
    label: Wind-down
    start: "21:30"
    end: "22:30"
    category: winddown
---

# Weekday

The shape of a working day, and the default for every weekday that has
no plan of its own. Edit the blocks here to change every weekday; change
one day on the Calendar and only that day moves.

The three **Blocks** are yours to fill — tap one on the Calendar to say
what it is for. **Maintenance** is the hour the small jobs get done.
