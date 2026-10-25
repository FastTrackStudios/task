---
"task-server" = "minor"
"live-proto" = "none"
---

Added `Merge` lineage for app documents: a document every device makes offline under the same id (a rig's library) now merges when the copies meet, keeping both devices' edits, instead of being refused. Existing documents stay `Strict`. Devices can also record their iroh relay with the account. Architect moves to v0.9.29.
