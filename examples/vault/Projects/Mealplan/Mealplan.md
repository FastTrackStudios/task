---
title: Mealplan
type: project
folder: "[[Projects]]"
---

# Mealplan

Project tracking planned, cooked, and skipped meals. Each meal
references one or more `[[Wiki/Cookbook]]` recipes and, once cooked,
the `[[Operations/Inventory/Pantry]]` items it consumed.

- `meals/` — one page per `YYYY-MM-DD-<slot>.md`.
- `mealplan.base` — table views: This week / Planned / Consumed / Skipped.

Parsed by the `mealplan` crate.
