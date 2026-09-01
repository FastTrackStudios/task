---
title: Getting Started
type: guide
---

# Getting Started

Install the CLI, sign in to the studio's server, and pick an org.

```sh
task auth login https://task.acme.test
task org use acme-audio
```

From there, `task wiki list` shows every wiki the org holds — this one
included. It is repo-sourced: what you are reading is `docs/Getting
Started.md` in the `task-docs` repository, mirrored onto the wiki at the
commit the wiki's page header names.

Next: [[Configuration]], then [[Deploying]].
