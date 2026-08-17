---
title: Inventory
type: index
folder: "[[Operations]]"
---

# Inventory

Owned things: gear, equipment, household possessions. Each item is a
page with frontmatter (`type: item`) recording name, category,
location, condition, lifecycle status, repair tasks.

- `Pantry/` — food items the `[[Mealplan]]` project consumes.

Items reference their `[[Locations]]` by id (uuid) so renames don't
break links. Parsed by the `inventory` (general) and `pantry`
(food-specific) crates.
