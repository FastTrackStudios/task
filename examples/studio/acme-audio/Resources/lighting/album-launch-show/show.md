---
type: resource
resource_kind: lighting
slug: album-launch-show
title: Album Launch Show
scope: show
cues:
- '1'
- '12'
- '13'
- '27'
content_root: ''
content_path: ''
updated_at: '2026-09-05T10:00:00Z'
source: ignition
---
<!-- The cue list itself is `show.json` beside this file; edit it there. Reference it as lighting:album-launch-show, a cue as lighting:album-launch-show#cue:12. -->
# Album Launch Show

Ignition's document for the night the album goes out. The `scope` is
`show` because it covers the evening rather than one song; a per-song
document would be `song`, and one for a set `setlist`.

Only the four labels in `cues` are addressable — cue 12 is
`lighting:album-launch-show#cue:12`. Nothing on the server parses the
cue list, so a cue that is not declared here cannot be pointed at, the
same way a chart section cannot.

## Notes

_Notes about this show go here; the source is `show.json`._
