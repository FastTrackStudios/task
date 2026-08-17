---
title: Shared Project
status: active
project_type: media-production
origin: acme-audio
collaborators:
  - acme-audio
  - vnt-video
clients:
  - Example Client
---

# Shared Project

**Two orgs on one project, and the folder is on the disk of neither the
one that started it.**

- it sits under `Projects/vnt-video/`, so vnt-video's disk holds the files
- `origin: acme-audio` — acme-audio started it, so their disk is where new
  content lands by default
- both are `collaborators`, with the same standing

Three separate facts, and a model with one "owner" field has to throw two
of them away. `origin` is a *default location* and confers nothing else:
acme-audio cannot remove vnt-video, cannot revoke their access, and is not
needed for a transfer between vnt-video and a third party. It can even
leave, and the default moves.

Real archives express this by straining two hand-written fields — an
`organization:` line and a tag — to carry three meanings. That works
while a person is reading it and not at all otherwise.
