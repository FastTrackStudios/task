---
title: Configuration
type: reference
---

# Configuration

The server reads its configuration from the environment. The variables
that matter to a studio:

| variable | what it does |
|---|---|
| `TASK_DATA_ROOT` | where every org's data lives |
| `TASK_SERVER_BIND` | address and port to listen on |
| `TASK_WIKI_REPO_SYNC_SECS` | how often repo-sourced wikis fetch (default 600) |

A repo-sourced wiki such as this one is declared once, with a clone
URL, a branch and a path. After that the mirror follows the branch on
its own; `task wiki refresh-source docs` asks for a fetch now rather
than at the next tick.

See also [[Getting Started]] and [[Troubleshooting]].
