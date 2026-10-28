---
"files-proto" = "minor"
"files" = "minor"
"files-client" = "minor"
"files-storage" = "patch"
"task-server" = "patch"
"task-cli" = "patch"
---

`RootsService::create_on` makes a new, empty root on one of the org's granted Storage Locations (`<location>/<grant prefix>/<dir>`), named by the location's id or, left out, the org's one location — what a client with no server path uses to put a library on the media volume. `FilesClient::ensure_root_on` and `task files root ensure --location` call it
