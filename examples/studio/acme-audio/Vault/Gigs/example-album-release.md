---
type: gig
title: Example Album release show
date: 2026-12-05
project: project:example-album
venue: "[[The Hall]]"
fee: 600
---
# Example Album release show

The album's release night at [[The Hall]]. Everything about it is
notes and bases (`docs/recipes/gigs.md`):

- the set is the `setlist-use` notes in `Gigs/example-album-release/`,
  one per song, each linking back here;
- `Setlist.base`, `Readiness.base` and `Calendar.base` beside them are
  the views;
- the rehearsal and the show are calendar events in `Records/events/`
  that link to this note, so they are this note's backlinks.
