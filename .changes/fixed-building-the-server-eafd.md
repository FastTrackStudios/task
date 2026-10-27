---
"task-server" = "patch"
---

Fixed building the server with only some plugins (`--no-default-features --features plugin-…`): a subset such as `plugin-fasttrackstudio` alone now compiles, so an embedded device node can leave out mail, agents, git, scheduling and the other plugins.
