---
type: scheduling-day-template
id: weekend
name: Weekend Daily Plan
description: Saturday/Sunday shape. Sleep-in, longer spiritual time, two allocatable blocks, no maintenance hour.
blocks:
  - id: morning-reset
    label: Morning Reset
    start: "07:30"
    end: "08:00"
    category: reset
  - id: spiritual-time
    label: Extended Spiritual Time
    start: "08:00"
    end: "09:00"
    category: spiritual
  - id: breakfast
    label: Breakfast prep + breakfast
    start: "09:00"
    end: "10:00"
    category: meal
  - id: block-1
    label: "Block 1: Project / Family / Free"
    start: "10:00"
    end: "13:00"
    category: allocatable
  - id: lunch
    label: Lunch
    start: "13:00"
    end: "14:00"
    category: meal
  - id: block-2
    label: "Block 2: Project / Family / Free"
    start: "14:00"
    end: "18:00"
    category: allocatable
    note: Longer block — saturday is the long-form project day.
  - id: dinner
    label: Dinner
    start: "18:00"
    end: "19:30"
    category: meal
  - id: evening
    label: Evening — social / rest
    start: "19:30"
    end: "23:00"
    category: other
  - id: sleep
    label: Sleep
    start: "23:00"
    end: "07:30"
    category: sleep
---

# Weekend Daily Plan

Looser variant of [[weekday]] — sleep in, longer spiritual time, two
allocatable blocks instead of three, no maintenance hour.
