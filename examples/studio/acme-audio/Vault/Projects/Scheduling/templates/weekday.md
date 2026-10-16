---
type: scheduling-day-template
id: weekday
name: Studio weekday
description: Mon–Fri at ACME — two studio blocks, and an hour at four for the errands and chores the studio runs on.
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
  - id: studio-am
    label: Studio block
    start: "09:00"
    end: "12:30"
    category: allocatable
    note: Sessions, comping, mixing
  - id: lunch
    label: Lunch
    start: "12:30"
    end: "13:30"
    category: meal
  - id: studio-pm
    label: Studio block
    start: "13:30"
    end: "16:00"
    category: allocatable
  - id: maintenance
    label: Maintenance
    start: "16:00"
    end: "17:00"
    category: maintenance
    note: The maintenance hour — @errands, @chores and #chore tasks lead the list
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

# Studio weekday

The shape of a working day at ACME, and the default for every weekday
that has no saved plan of its own.

The block worth reading twice is the **maintenance hour** at 16:00. A
studio runs on small jobs nobody books time for — gaff tape, strings,
the recycling, a hired mic stand that has to go back — and a task list
ordered by due date never gets to them. During this hour the task list
leads with work tagged `@errands`, `@chores`, `#chore` and their kin
(`scheduling_proto::BlockCategory::surfaces`), and those gates open
wherever Alice says she is.
