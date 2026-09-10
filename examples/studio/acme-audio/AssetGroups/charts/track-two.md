---
type: asset
asset_kind: chart
slug: track-two
title: Track Two
key: Bb
notation: keyflow
sections:
- verse
- chorus
song: song:track-two
arrangement: original
is_default: true
updated_at: '2026-09-06T10:00:00Z'
source: keyflow
tags: [chart, album]
---
# Track Two

```keyflow
[Verse]
| Bb | F | Gm | Eb |
| Bb | F | Eb | Eb |

[Chorus]
| Eb | Bb | F | Gm |
| Eb | Bb | F | F  |
```

The album's second chart. One chart is a file; two are a *library* —
which is the point of it being here. `Chart Library` in the planted org
is an ordinary `Collection` of kind `Library` holding `chart:track-one`
and `chart:track-two`, and nothing was built to make that possible
(ADR 0003, which ADR 0004 keeps).

What ADR 0004 changed is where the file lives: on the vault's `Assets/`
shelf instead of the resources tier. A library still collects
`chart:<slug>` references and still does not care where the bytes are.

## Notes

_Notes about this chart go here._
