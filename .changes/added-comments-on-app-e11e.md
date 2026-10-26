---
"task-cli" = "minor"
"task-server" = "minor"
"task-ui" = "minor"
"threads-db" = "minor"
"threads-proto" = "minor"
---

Added comments on app documents and on a place inside any entity: a thread anchored on an app document (entity type doc) is titled by the document's face in the activity feed, and a thread may carry an anchor path (entries/<id>) with list_threads_at listing a place and everything under it.

Migration: the threads database gains a nullable anchor_path column (migration m20261026_000001_thread_anchor_path, runs on boot); the Thread and CreateThreadRequest wire types gain anchor_path, so clients and server update together.
