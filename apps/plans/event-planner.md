# Event Planner — events, teams, order, times (the Planning Center shape)

Status: DESIGN (2026-07-22). Captured from product direction; nothing
implemented. Companions: `collaboration-sharing.md` (sharing an event/
setlist to its teams is the distribution channel), `architect-permissions.md`
(who may edit vs confirm), the wikilink-setlist authoring landed in
`crates/task/ui/src/pages/vault.rs::setlist_songs_from_body`.

## The target

Replace Planning Center for a worship/production org. One EVENT note is the
whole plan — three facets (tabs within the note):

```
Event: JHM Sunday Morning — July 19
├─ Order      the service order (Welcome → Games → Worship → Closing),
│             where "Worship" IS the setlist (composable wikilink)
├─ Teams      teams assigned to THIS event, each position staffed,
│             each person Confirmed / Pending / Declined
└─ Times      event time (11:00–12:00), rehearsal (9:30–11:00),
              per-team call times
```

## Authoring model — everything is a note + wikilinks

The wikilink-setlist convention generalizes: **a wikilink on its own line
is a composable block reference**.

- `type: event` note, `Order` section: an ordered list where any line can
  be a plain item (`Welcome`) or a wikilink to a setlist/note
  (`[[Sunday Worship]]`) — the Worship slot embeds the whole setlist
  (rendered as the streaming player / rehearsal entry point).
- Setlist notes stay as landed: `[[Praise - Elevation Worship]]` lines.
- FUTURE arrangement/key/leader selection rides the link syntax:
  `[[Praise - Elevation Worship#<Arrangement>]]` picks the arrangement
  (arrangements = sections/variants inside the song note when they exist —
  none yet), and structured annotations follow
  (`[[Song#Arr|key: G, leader: Bianca]]` — exact embed syntax TBD with the
  editor widget pass).

## Data model

### Teams — architect already has half of this

`architect-auth` ships `AuthTeam` + `AuthTeamMember` (teams within an
organization). Events need a THIRD layer: **an event's team assignment**
— a team template instantiated for one event with per-position people and
per-person status.

```
EventTeamAssignment {
  event_id, team_id           // e.g. "Band", "Vocals", "Tech" (org team)
  positions: [
    { position: "Bass Guitar",        person: contact/user, status },
    { position: "Drums",              person, status },
    { position: "Music Director",     person, status },
    …
  ]
}
status: Confirmed | Pending | Declined      // per person per event
```

- Org teams ("Band", "Tech") define the POSITION VOCABULARY and the
  default roster (who usually plays bass). An event assignment starts as a
  copy of the template and is edited per event.
- People are contacts first (many musicians never get accounts), linkable
  to `AuthUser`/federated members later — the invite flow
  (collaboration-sharing S5) is how "Pending" reaches them and how
  Confirm/Decline comes back (a share link scoped to the event where the
  person taps Confirm — that loop IS the Planning Center "accept").

### Where it lives

Event facts stay IN THE NOTE (portable, diffable, shareable) as structured
frontmatter, mirroring the setlist convention:

```yaml
---
type: event
date: 2026-07-19
times:
  event: 11:00–12:00
  rehearsal: 9:30–11:00
  call:
    Band: 9:00
    Tech: 8:30
teams:
  - team: Band
    positions:
      - { position: Bass Guitar,     person: Cody Wright,      status: confirmed }
      - { position: Drums,           person: Andrew Gerges,    status: pending }
      - { position: Electric Guitar, person: Issac Chaves,     status: confirmed }
      - { position: Keys,            person: Heaven Ghaly,     status: pending }
      - { position: Music Director,  person: Cody Wright,      status: confirmed }
  - team: Vocals
    positions:
      - { position: Worship Leader,  person: Bianca Borquez,   status: confirmed }
      - { position: Vocals,          person: Lily Lequex,      status: pending }
      - { position: Vocals,          person: Morgan Conrad,    status: pending }
      - { position: Vocals,          person: Janissa Velasco,  status: declined }
  - team: Tech
    positions:
      - { position: Audio FOH,               person: Ethan Solarte }
      - { position: Computer Graphics/Lyrics, person: Ainsleigh Pemberton }
      - { position: Lighting Operator,        person: Jeremiah William }
      - { position: Production Lead,          person: Jose Solarte }
---
# JHM Sunday Morning

## Order
Welcome
Games
[[Sunday Worship]]
Closing
```

The Properties tab renders/edits this structurally (the frontmatter is
already hidden from Raw view); sqlite indexes (event queries, "my
assignments", per-person history) are derived, not primary.

## UI

- **Event note = tabs within the note**: Order | Teams | Times (the tabbed
  note body slot the setlist note already exercises with Session/Chart).
- **Order tab**: the order list; the setlist wikilink row expands into the
  streaming player / opens the rehearsal Experience.
- **Teams tab**: Planning-Center-style roster — grouped by team, one row
  per position (position · person chip · status pill), status editable by
  leaders, tap-to-confirm for the person themself (via their share/invite
  link). Fed by `pages/members.rs` person-chips + contacts.
- **Times tab**: event/rehearsal ranges + per-team call times; later feeds
  the schedule/calendar feature (`scheduling-*` crates) and per-person
  ICS.
- **Statuses roll up**: the event row in lists shows e.g. "9 confirmed ·
  3 pending · 1 declined".

## Staging

1. **E1 — event note type + Times/Teams frontmatter parse** (read-only
   render of the three tabs from the YAML above; no RPC).
2. **E2 — Teams editing** (Properties-grade structured editing; org-team
   templates from `AuthTeam` seed the position lists; contacts
   autocomplete).
3. **E3 — confirm loop**: per-person share link (collaboration-sharing
   S5's grants) with a Confirm/Decline landing; status writes back to the
   note over the org lane; notification hooks (email later).
4. **E4 — order embeds**: wikilinked setlist rows render inline (the
   editor-widget pass shared with per-song stream rows); arrangement/key/
   leader link annotations.
5. **E5 — rollups + schedule integration**: event lists, "my events",
   call-time ICS, blackout dates (the full PC parity backlog).
