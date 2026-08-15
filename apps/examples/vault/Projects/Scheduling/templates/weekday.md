---
type: scheduling-day-template
id: weekday
name: Weekday Daily Plan
description: Cody's default Mon–Fri day. Three full allocatable blocks with structured wrappers (reset / spiritual / gym / meals / wind-down / 7.5h sleep). Ported from `Daily Plan` in The Observatory.
blocks:
  - id: morning-reset
    label: Morning Reset
    start: "06:00"
    end: "06:30"
    category: reset
  - id: spiritual-time
    label: Spiritual Time with God
    start: "06:30"
    end: "07:00"
    category: spiritual
  - id: breakfast-prep
    label: Breakfast prep
    start: "07:00"
    end: "07:30"
    category: meal
  - id: breakfast
    label: Breakfast + quick cleanup
    start: "07:30"
    end: "08:00"
    category: meal
  - id: gym
    label: "Gym: walk/run there, workout, walk/run back"
    start: "08:00"
    end: "09:00"
    category: exercise
  - id: shower
    label: Shower + get ready
    start: "09:00"
    end: "09:30"
    category: hygiene
  - id: block-1
    label: "Block 1: Work / Event / Free Time"
    start: "09:30"
    end: "12:30"
    category: allocatable
    note: Deep work — protect for the hardest task of the day.
  - id: lunch
    label: Lunch prep + lunch + quick cleanup
    start: "12:30"
    end: "13:30"
    category: meal
  - id: block-2
    label: "Block 2: Work / Event / Free Time"
    start: "13:30"
    end: "16:30"
    category: allocatable
    note: Meetings / collaborative work / errands.
  - id: maintenance
    label: Maintenance Hour
    start: "16:30"
    end: "17:30"
    category: maintenance
    note: Inbox, errands, laundry, life admin.
  - id: dinner
    label: Dinner prep + dinner + quick cleanup
    start: "17:30"
    end: "19:00"
    category: meal
  - id: block-3
    label: "Block 3: Work / Event / Free Time"
    start: "19:00"
    end: "22:00"
    category: allocatable
    note: Personal projects, study, music, social.
  - id: wind-down
    label: Wind down
    start: "22:00"
    end: "22:30"
    category: winddown
  - id: sleep
    label: Sleep — 7.5 hours
    start: "22:30"
    end: "06:00"
    category: sleep
---

# Weekday Daily Plan

The default Mon–Fri day. Three full allocatable blocks (9.5h total) with
structured wrappers for spiritual time, gym, three home-cooked meals, a
maintenance hour, and 7.5 hours of sleep.

## Targets
- 3 full allocatable time blocks
- 30 min dedicated spiritual time
- 1 hour of gym
- 30 min to get ready
- 3 full meals, home-cooked
- 7.5 hours of sleep

See [[weekend]] for the looser variant.
