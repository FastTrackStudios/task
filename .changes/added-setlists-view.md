---
"ui" = "minor"
"task-server" = "minor"
"task-cli" = "minor"
---

Added a Setlists page (`/setlists`, in the sidebar beside Projects): every collection of kind `setlist` or `songlist` in your organizations, grouped by the project that holds it, with the ones no project holds under "Not in a project". `/setlists/<org>/<id>` shows one setlist's songs in order, each linked to its project when Task knows it, and a project's page lists the setlists it holds. A project holds a setlist by a `project:<dir>` → `collection:<id>` (`Down`) link, made by the MCP tools `attach_setlist` / `detach_setlist` or `task project attach-setlist <project> <collection>` / `task project detach-setlist`. A project's form can now be set with `form` on the MCP `create_project` / `update_project` tools, `task project create --form` and `task project set-form`. A project path handed to `attach_doc` and friends may now be its page path (`example-album/project.md`).
