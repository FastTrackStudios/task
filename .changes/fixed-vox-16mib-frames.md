---
"task-server" = "patch"
"task-dial" = "patch"
---

Fixed a large document failing to sync: a vox message over 16 MiB (the first sync of a real song's session, say) was refused by the WebSocket's frame limit and the connection dropped and retried forever. Task's vox links, server and client, now carry messages up to 64 MiB.
