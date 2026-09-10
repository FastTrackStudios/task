---
type: project
title: Track Two
status: active
capabilities:
  - music-production
form: song
---

# Track Two

**A part that earned a project.** Track Two is the third track of
*Example Album* and it is also a project of its own, which is the case
`project.part.promotion` exists for: it grew its own Reaper session, its
own renders and its own deliverable, and at that point calling it a
line in the album's roster stopped being enough.

The album still lists it. `project.part.listing` is explicit that a
parent's part list is "a roster of its pieces, not a list of the ones
that are not projects", so *Example Album* has three tracks before and
after this page existed, in the same order. Nothing that referenced
this song has to know which side of the line it now sits on — the
subproject carries the part's own id, so every link, deliverable and
time entry already attached to it goes on resolving.

## Why it is a directory inside its parent

`<org>/projects/example-album/track-two/` — inside, because that is
where the work already was, and promotion is not supposed to move
bytes. The nesting is where the files live; the parentage is the
`parentId` in this page's frontmatter, written by the seeder. Nothing
reads one off the other, which is `project.nesting.explicit`.

Demoting this back to a plain part means deleting this file. The
sessions, the renders and the deliverable stay exactly where they are.
