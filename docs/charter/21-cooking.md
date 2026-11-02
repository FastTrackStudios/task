# The Ideal Cooking System

_Charter #21 (formerly issue #21)._

Meal planning, recipes, and the shopping that follows from them — the domestic counterpart to project planning, and a real test of whether the system handles life as well as work.

## Hard requirements

_To be detailed._

Prompts worth answering: What is a recipe as data — ingredients with quantities and units, or prose? How does scaling work, and does it survive fractions and awkward units? How does a week of meals become one shopping list, deduplicated and grouped by aisle? What does the system know about what is already in the pantry? How do dietary constraints, leftovers, and cooking time factor into a plan? What happens when you cook something not on the plan? How does a recipe arrive — typed, pasted from a website, photographed? What does the phone experience need while you are actually cooking, hands dirty?

## Non-goals

_To be detailed._

## Current state

A `mealplan` feature exists with a cookbook (parsing, model, wiki integration), a mealplan store and service, and a week view landed via `worktree/cooking`. Recipes live in the vault as notes.

Related: [The Ideal Inventory System](10-inventory.md) (pantry as inventory), [The Ideal Vault (PKM)](08-vault.md).
