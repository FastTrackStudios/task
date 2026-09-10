---
type: asset
asset_kind: chart
slug: track-one
title: Track One
key: A
notation: keyflow
sections:
- verse-1
- chorus
- bridge
song: song:track-one
arrangement: original
is_default: true
updated_at: '2026-09-05T10:00:00Z'
source: keyflow
tags: [chart, album]
---
# Track One

```keyflow
[Verse 1]
| A | E | F#m | D |
| A | E | D  | D |

[Chorus]
| D | A | E | F#m |
| D | A | E | E   |

[Bridge]
| F#m | D | A | E |
```

The chart for the album's opener, and — since ADR 0004 — an ordinary
**vault document** on the `Assets/` shelf rather than a pair of files in
a tier nothing could collaborate on.

That is the whole of what changed, and everything it bought is visible
from right here. Open this page in two tabs and type in both: it
converges, because a chart is a vault file and `vault-collab` holds a
Loro document for every vault file. Nothing chart-shaped was built to
make that work. The same goes for the `chart` tag in the frontmatter, for the backlinks
panel, for `[[wikilinks]]` written in the notes below, and for the fact
that searching the vault finds this page. All of it is what a note has
always had, and none of it was built for charts.

The song this arranges is `song:track-one`, named as a reference rather
than as a `[[wikilink]]` — a song and its default arrangement share a
slug by design, so the two documents share a basename and a bare
wikilink between them would be ambiguous. A `song:`-qualified reference
is not.

It is also the album arrangement of `song:track-one`, and the song's
**default** chart — the one to open when somebody asks for "the chart"
and names no arrangement. `chart:track-one-condensed-live` beside it is
the other one, and `chart:track-one#chorus` addresses the section above.

## Notes

_Notes about this chart go here, in the same document as the chart, and
they converge with it. That is the point._
