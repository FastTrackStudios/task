---
"task-server" = "patch"
---

The demo plant encodes its video deliverables on one ffmpeg thread each, so concurrent plants (the test suite, a reseed) stop oversubscribing the machine.
