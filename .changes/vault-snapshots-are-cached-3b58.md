---
"files" = "patch"
"goal" = "patch"
"mealplan" = "patch"
"milestone" = "patch"
"pantry" = "patch"
"project" = "patch"
"task" = "patch"
"vault" = "patch"
"vault-entity" = "patch"
"vault-live" = "patch"
"wiki-live" = "patch"
"workstream" = "patch"
---

Vault snapshots are cached per root and pages share their text as Arc<str>, so loads stop cloning every note.
