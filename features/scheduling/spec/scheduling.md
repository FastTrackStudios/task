# Scheduling Spec — calendar events

A calendar event (`CalEvent`, `features/scheduling/scheduling-proto/src/cal_event.rs`)
is one markdown file under the org vault's `Records/events/`. The
frontmatter is the event; the body is whatever the user (or a seeded
setlist experience) wrote under it. The `VaultScheduler`
(`features/scheduling/scheduling/src/vault_scheduler.rs`) reads and
writes those files; the MCP tools `list_events` / `create_event` /
`update_event` / `reschedule_event` / `cancel_event`
(`apps/server/src/mcp.rs`) and the calendar page drive it.

This spec covers what an event carries besides its time — links to
notes, an upstream id, a tombstone — because those are what let a
gig, a church service or a Planning Center plan be *notes and bases*
with the calendar as one more view of them, rather than a bespoke
feature each.

| piece | where |
|---|---|
| the type, `EventLink`, `ExternalRef` | `features/scheduling/scheduling-proto/src/cal_event.rs` |
| frontmatter → event | `features/scheduling/scheduling/src/parse/cal_event.rs` |
| event → frontmatter, rewrite-in-place | `features/scheduling/scheduling/src/write/cal_event.rs` |
| the tools | `apps/server/src/mcp.rs` (`list_events`, `create_event`, `update_event`) |
| end to end | `apps/server/tests/it/mcp_calendar_e2e.rs` |

---

## Links

### An event links to notes by relation

t[scheduling.event.links]
`create_event` and `update_event` accept `links: [{relation, target}]`.
`relation` is a lowercase slug (`for`, `rehearsal-for`, `venue`) that is
not one of the event's own keys (`CalEvent::RESERVED_KEYS`); `target` is
a note, bare (`Gigs/caa-2026`) or as a wikilink (`[[Gigs/caa-2026]]`),
and is stored as a wikilink. On disk each relation is **one frontmatter
property whose value is a list of wikilinks**:

```yaml
type: scheduling-cal-event
id: 6f1c…
title: CAA 2026 — show
start: 2026-11-06T19:00:00-06:00
end: 2026-11-06T22:00:00-06:00
for:
  - '[[Gigs/caa-2026]]'
```

`list_events` returns every event's `links`, and reading the file back
gives the same set (grouped by relation, in first-appearance order). An
invalid relation or an empty target is refused with an error naming the
rule.

### A note finds its events — the decision

t[scheduling.event.links.note-side]
The events for a note are found **from the note's side through the
vault's own link graph, not through a calendar index**. Because the
relation is a frontmatter property of wikilinks, the link index counts
the event file as a backlink of the note (frontmatter wikilinks are
edges — `vault-obsidian/src/links.rs`), so:

- the note's backlinks (`VaultGraph::backlinks`, the inspector's Links
  tab) list `Records/events/<file>.md`;
- a `.base` over events filters on the property —
  `type == "scheduling-cal-event" && note.for.contains("[[Gigs/caa-2026]]")`
  (list properties match whole elements, so the target is the wikilink)
  — or, with the bases link functions, `file.hasLink("Gigs/caa-2026")`;
- from the note's own row a base counts them with
  `file.backlinks("scheduling-cal-event")`;
- `list_events { linked_to: "Gigs/caa-2026" }` answers the same question
  over MCP.

Both a backlink and a queryable field, from one stored fact. No second
copy of the link lives on the note, so nothing can drift.

---

## External refs

### An integration names what it mirrored

t[scheduling.event.external-ref]
An event may carry `external_ref: {system, id}` — the id an integration
knows it by (`{system: "pco", id: "plan_time:123"}`). It is written as
one string, `system:id` (`external_ref: pco:plan_time:123`); the system
is everything before the first `:`, so upstream ids may contain colons.
`list_events { external_ref: "pco:plan_time:123" }` returns only that
event, **including when it is tombstoned** — a re-sync must find what it
made last time whatever state it left it in.

### Creating twice makes one event

t[scheduling.event.external-ref.idempotent]
`create_event` with an `external_ref` that an event already carries
writes nothing and returns that event with `existing: true`. A re-sync
therefore calls `create_event` for every upstream item and then
`update_event` / `reschedule_event` for what changed, and never
duplicates.

---

## Updating without moving

### `update_event` changes what, not when

t[scheduling.event.update]
`update_event { id, title?, description?, location?, links?, tombstone? }`
changes only the fields passed: `''` clears `description` / `location`,
`links` replaces the whole set (`[]` removes every link). `start`,
`end`, `all_day` and `recurrence` are untouched — moving is
`reschedule_event`. Calling it with nothing to change is an error. The
write goes through the same `CalendarEvents::upsert_event` as every
other event write, so the same `scheduling/events/**` write permit and
audit apply.

### A rewrite keeps what isn't the event's

t[scheduling.event.rewrite-keeps]
Writing an event that already has a file rewrites **that file**, under
its existing name (a seeded `Records/events/Sunday Service.md` stays
that file; it does not grow a `Sunday-Service.md` twin). The rewrite
keeps the markdown body byte for byte, the file's existing `type:`, and
every frontmatter property that is neither an event key nor a link
relation (`experience`, `tags`, a property a `.base` or an integration
set). The calendar page carries location, links, external ref and
tombstone through its own edits for the same reason.

---

## Tombstones

### Removed upstream is marked, not deleted

t[scheduling.event.tombstone]
When an integration finds that something it mirrored is gone upstream,
it does **not** delete the event: it calls
`update_event { id, tombstone: true }`, which stamps
`tombstone: <RFC-3339 UTC now>` into the frontmatter. A tombstoned
event keeps its file, body, links and external ref; `list_events`, the
calendar page, the day plan and the brief leave it out;
`list_events { include_tombstoned: true }` and an `external_ref` lookup
still return it. `tombstone: false` clears the stamp (the item came
back upstream). Deleting stays `cancel_event`, a human's decision.

### The same convention for mirrored notes

t[scheduling.tombstone.notes]
A note an integration mirrors (a PCO plan as `Gigs/<slug>.md`, a song,
a role assignment) follows the same convention through its properties:

```yaml
external_ref: pco:plan:456
tombstone: 2026-10-21T14:03:00Z      # set when gone upstream; removed when it returns
tombstone_reason: deleted in PCO     # optional, free text
```

The integration sets and clears these with the property tools
(`update_properties`), never by deleting the note, so backlinks, base
rows and history survive. Views that should hide removed items filter
`!tombstone` (a base: `filters: and: ['!note.tombstone']`). An
integration identifies its own notes by `external_ref` exactly as it
does events; one `system:id` string, one owner.
