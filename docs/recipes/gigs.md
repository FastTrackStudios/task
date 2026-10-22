# Recipe: gigs as notes and bases

A gig in Task is not a feature. It's a few kinds of ordinary vault notes,
some calendar events that link to them, and three `.base` files that query
them. Everything here runs on generic primitives:

- frontmatter properties, edited with `update_properties`;
- wikilinks, with the bases link functions (`file.hasLink`, `.exists()`,
  `.hasProperty()`) and link-follow (`note.song.key`);
- the `transpose_key` formula;
- calendar event links;
- `query_base` over MCP.

Nothing in the code knows what a gig is. A church service from Planning
Center uses the same types: the service is a `gig`, its plan items are
`setlist-use`s, and its plan times are events with
`external_ref: pco:plan_time:<id>`.

The worked example is seeded in the example org (`acme-audio`) under
`examples/studio/acme-audio/Vault/`: the *Example Album release show*.
`example_org::declared_tests::the_gig_recipe_seed_runs_as_bases` checks it,
and so does the MCP test `apps/server/tests/it/mcp_gig_recipe_e2e.rs`.

## Note types

The type is the `type:` property. Paths are conventions, not rules: the
bases filter on `type` and on links, not on folders.

### `song`

One song, whatever gigs it is played at.

| property | value |
|---|---|
| `type` | `song` |
| `key` | the written key, e.g. `G`, `F#m`, `Bb minor` |
| `bpm` | number |
| `chart` | the chart asset, `chart:<slug>` |
| `session` | the playback session, a wikilink to a session note |
| `reference` | the reference audio, a resource path |

The song library itself is the `assets/songs` shelf (`asset_kind: song`,
ADR 0004). A `.base` row set, though, is the org **vault** only: `query_base`
reads what `read_note` can read. That means link-follow (`note.song.key`)
needs the song as a vault note. The seed keeps them in `Gigs/Songs/`. Name
each song note uniquely across the vault, because a bare `[[Night Drive]]`
resolves by basename.

### `gig`

One booking: a show, a wedding, or a Sunday service. Its path is
`Gigs/<slug>.md`.

| property | value |
|---|---|
| `type` | `gig` |
| `title` | display name |
| `date` | `YYYY-MM-DD` |
| `project` | the project the gig belongs to, `project:<slug>` |
| `venue` | a wikilink to a venue note |
| `fee` | number |

The gig doesn't list its songs or its events. Both link **to** the gig, so
there's only one copy of each fact, and nothing has to be kept in step.

### `setlist-use`

One song as played at one gig. Its path is
`Gigs/<gig-slug>/<Song> (<gig>).md`. The basename must differ from the
song's, so that `[[Song]]` stays unambiguous.

| property | value |
|---|---|
| `type` | `setlist-use` |
| `gig` | `"[[Gigs/<gig-slug>]]"` |
| `song` | `"[[<Song>]]"` |
| `position` | 1-based number; the setlist sorts on it |
| `key_offset` | `"+4"` / `"-2"` / `3` (semitones), or an absolute key (`E`). Leave it out to play the song's own key. |
| `lead` | who sings or leads it |
| `transition` | how you get into it from the previous song |
| `patch` | the sound, `patch:<slug>` |

Use one property for both kinds of key change. `transpose_key(key, offset)`
moves a key by a number of semitones, keeping its own spelling, and an
offset that is itself a key name replaces the key. So "up four" and "in E"
are both just `key_offset`, and changing one with `update_properties` is a
single property write.

## Events

Rehearsals and shows are calendar events (`Records/events/<id>.md`,
`type: scheduling-cal-event`, see `features/scheduling/spec/scheduling.md`).
They link to the gig by **relation**. Each relation is a frontmatter
property whose value is a list of wikilinks:

```yaml
for:                # the show itself
  - '[[Gigs/example-album-release]]'
rehearsal-for:      # a rehearsal for it
  - '[[Gigs/example-album-release]]'
```

`create_event` / `update_event` take these as
`links: [{relation: "for", target: "Gigs/example-album-release"}]`. Because
a frontmatter wikilink is a graph edge:

- the events are the gig note's backlinks;
- `list_events { linked_to: "Gigs/example-album-release" }` returns them;
- a base finds them with `file.hasLink("Gigs/example-album-release")`.

A mirrored event that was deleted upstream is tombstoned (`tombstone:`),
not removed. The Calendar base filters those out.

## The three bases

Each gig gets its own three bases, beside its setlist-uses in
`Gigs/<slug>/`. Each one scopes to the gig with
`file.hasLink("Gigs/<slug>")`.

### `Setlist.base`

```yaml
filters:
  and:
    - 'type == "setlist-use"'
    - file.hasLink("Gigs/example-album-release")
formulas:
  song_key: "note.song.key"
  effective_key: "transpose_key(note.song.key, key_offset)"
views:
  - type: table
    name: Setlist
    order: [position, song, formula.song_key, key_offset, formula.effective_key, lead, transition, patch]
    sort:
      - property: position
        direction: ASC
```

`note.song.key` follows the `song` link and reads the song note's `key`.
`transpose_key` applies the use's offset or absolute key. With no
`key_offset`, the effective key is the song's own.

### `Readiness.base`

This base asks whether everything the band needs for each song is in
place:

```yaml
formulas:
  chart: 'note.song.hasProperty("chart")'
  session: "note.song.session.exists()"
  reference: 'note.song.hasProperty("reference")'
  patch_set: 'file.hasProperty("patch")'
  ready: "formula.chart && formula.session && formula.reference && formula.patch_set"
```

It has two views. **Readiness** shows every use with the four checks.
**Not ready** (`!formula.ready`) lists only the songs that are missing
something.

The session is a vault note, so its check is `.exists()`: the link must
resolve to a note that is really there. A link to a session nobody has
written yet is "not ready". The chart, the reference audio and the patch
live outside the vault, on the asset and resource shelves, so a base can
only see whether the song or the use names them (`hasProperty`).

### `Calendar.base`

```yaml
filters:
  and:
    - 'type == "scheduling-cal-event"'
    - file.hasLink("Gigs/example-album-release")
    - '!file.hasProperty("tombstone")'
formulas:
  role: 'if(file.hasProperty("rehearsal-for"), "rehearsal", "show")'
views:
  - type: calendar
    name: Calendar
    dateProperty: note.start
    order: [title, start, end, location, formula.role]
    sort:
      - property: start
        direction: ASC
```

## Driving it over MCP

- `query_base { path: "Gigs/example-album-release/Setlist.base" }` returns
  the uses in position order. Each row's `formulas.effective_key` is the
  key to play in.
- `query_base { path: ".../Readiness.base", view: "Not ready" }` is the
  to-do list for the gig.
- `update_properties { path: "Gigs/example-album-release/Night Drive (release show).md", set: { key_offset: "+2" } }`
  moves a song. The next `query_base` reflects it. Pass
  `remove: ["key_offset"]` to go back to the written key.
- `create_event` with `links: [{relation: "rehearsal-for", target: "Gigs/<slug>"}]`
  books a rehearsal, and the Calendar base picks it up.
- To add a song to the set, write a new `setlist-use` note (`write_note`).
  To reorder the set, change `position` on the uses.

## The seeded example

The seed lives under `examples/studio/acme-audio/Vault/`.

| file | what it shows |
|---|---|
| `Gigs/example-album-release.md` | the gig, linking `project:example-album` and `[[The Hall]]` |
| `Gigs/example-album-release/Night Drive (release show).md` | position 1, `key_offset: "+4"`: G becomes B |
| `Gigs/example-album-release/Paper Moon (release show).md` | position 2, absolute `key_offset: E`: D becomes E |
| `Gigs/example-album-release/Last Call (release show).md` | position 3, no offset: A stays A. Its song's session note is missing, so it's not ready. |
| `Gigs/Songs/{Night Drive,Paper Moon,Last Call}.md` | the songs, with key, chart, session and reference |
| `Records/sessions/{night-drive,paper-moon}-playback.md` | the playback sessions that exist |
| `Records/events/example-album-release-{rehearsal,show}.md` | `rehearsal-for` and `for` events linked to the gig |
| `Gigs/example-album-release/{Setlist,Readiness,Calendar}.base` | the three views |

The file names deliberately don't sort in set order (Last Call <
Night Drive < Paper Moon), so the Setlist view's order can only come from
`position`.
