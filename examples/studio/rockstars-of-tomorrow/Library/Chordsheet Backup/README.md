# Chordsheet Backup

Rockstars of Tomorrow's chord charts, as they came out of chordsheet.com and
through Keyflow's converter — the Adult Jam library.

- `manifest.json` — the backup's index: title, artist and source file per chart.
- `kf/` — every chart as Keyflow source.

**Copied verbatim from keyflow**, `examples/chordsheet-compat/` at `b720d0a`
(the `kf/` directory and `manifest.json`; the raw `.txt` exports are not
needed here). Keyflow is where these are made; to refresh them, re-run the
converter there and copy the two paths across again. Never edit a chart here
by hand — this folder is somebody else's output.

## What the seed does with it

It sits in the org's files exactly as a backup folder would sit on the jam
leader's disk. At plant time `example_org::plant_chordsheet_library` imports it
into the library: one song per manifest entry (the artist as its writer), one
chart per song carrying the `.kf` source and made the song's default, and a
`songlist` called **Adult Jam** holding every song in the backup's order.
Setlists drawn from that list, and a show holding them, are declared in
`example_org::DECLARED_COLLECTIONS`.
