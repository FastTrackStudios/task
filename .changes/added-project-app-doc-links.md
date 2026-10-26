---
"task-server" = "minor"
"links-proto" = "minor"
"links" = "minor"
"task-cli" = "minor"
---

Added app documents to projects: a project can now name the documents client apps keep in the org's app-docs catalog, whatever app made them. The MCP tools `list_app_docs`, `attach_doc`, `detach_doc` and `project_docs`, and the commands `task project app-docs [--kind K]`, `task project attach-doc <project> <doc-id>`, `task project detach-doc <project> <doc-id>` and `task project docs <project>`, list the catalog and attach, detach and list a project's docs; a doc deleted since it was attached is listed as gone. Links can now point at an app document as `doc:<uuid>`.
