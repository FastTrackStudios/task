---
"live-proto" = "none"
"scripture" = "none"
"task-cli" = "none"
"task-server" = "minor"
"task-vault" = "minor"
"wiki-proto" = "none"
---

Changed: the vault, links and tags are architect's (FastTrackStudios/architect#333, #344). Task's `vault`, `vault-*`, `links`, `links-proto`, `tag` and `tag-proto` crates are gone; their workspace dependencies point at architect's git, and architect itself moves to the same commit. `project` is a built-in kind there and `project.md` the default nested-root marker. Task's own layer is the new `task-vault` crate: its `task` and `area` kinds (`task_vault::schema_registry` in place of `PropertySchemaRegistry::with_builtins`) and the `.base` formula `transpose_key`, registered by `task_vault::install` (`AppState::open_at`, the CLI's `main`). The `sync_vault_links` example moves to `scripture`; `links-ui` stays.
