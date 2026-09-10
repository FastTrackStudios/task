---
type: asset
asset_kind: song
slug: track-one
title: Track One
writers: [ACME Audio]
key: A
tags: [song, album]
updated_at: '2026-09-09T10:00:00Z'
source: song
---
# Track One

The album's opener, as a **song** — the thing the charts beside it are
arrangements *of*. ADR 0004 decision 1 made this a vault document on the
`Assets/` shelf, which is what lets you tag it, link to it, search it,
and edit it with somebody else looking at the same page.

## Arrangements

Not listed here, deliberately. `chart:track-one` and
`chart:track-one-condensed-live` each carry `song: song:track-one`, and
that reference is the join: adding an arrangement is one write with
nothing to keep in step, and the answer to "which arrangements does this
song have?" is the backlinks panel — or `list_charts("song:track-one")`
over the wire.

The vendored song folder this replaced kept the same fact in four places
at once — an `arrangements:` list, a `defaultArrangement` uuid, a
directory per arrangement, and each arrangement's own note. Four places
is four places to drift.

## The audio is not here, and that is the tier rule

`resources/songs/track-one/` still holds `manifest.json` and the
reference mix, and still will. Those are **Resources**: imported bytes
nobody types into, streamed by the player through
`/org/acme-audio/media/songs/track-one/…`. This page is the part
somebody writes.

## Notes

_Notes about the song — not about any one arrangement — go here._
