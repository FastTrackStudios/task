---
"task-server" = "minor"
"live-proto" = "none"
---

Added `Merge` lineage for app documents: a document every device makes offline under the same id (a rig's library) now merges when the copies meet, keeping both devices' edits, instead of being refused. Existing documents stay `Strict`. Architect moves to v0.9.29 (whose device registry can also record a device's iroh relay; Task's own devices do not send one).
