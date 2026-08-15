# Scheduling — cal.com feature parity tracker

**Status:** parity tracker — ongoing. Living scoreboard, never "done".

Maps the cal.com / cal.diy surface onto our `scheduling` feature so
we can tell at a glance what's shipped, what's planned, and what we
intentionally aren't doing.

Reference checkout (read-only): `~/Development/research/cal-diy`
(actually the full cal.com monorepo — `cal.diy` is the simpler
single-user variant; we model the core types after cal.com's Prisma
schema since the shapes are the same).

## Legend

| Status | Meaning |
| --- | --- |
| 🟢 done | Shipped (in `scheduling-proto` / `scheduling` / `scheduling-ui`). |
| 🟡 partial | Shape exists in proto; impl or UI is stubbed. |
| 🔵 planned | On the roadmap, not yet started. |
| ⚪ deferred | Possible later; not committed to. |
| 🔴 won't do | Out of scope for our product. |

## Core scheduling primitives

| cal.com surface | Our surface | Status | Notes |
| --- | --- | --- | --- |
| `EventType` (title / slug / duration / location) | `scheduling_proto::EventType` | 🟢 done | Wire shape lives in `event_type.rs`. UI editor 🔵. |
| `Schedule` (named availability bundle + timezone) | `scheduling_proto::AvailabilitySchedule` | 🟢 done | UI editor 🔵. |
| `Availability` (days[] + start + end + optional date) | `scheduling_proto::AvailabilityRule` | 🟢 done | Per-date overrides via `date: Option<NaiveDate>` 🔵. |
| `Booking` (event_type + slot + attendee + status) | `scheduling_proto::Booking` + `NewBooking` + `BookingStatus` | 🟢 done | UI booking page + host inbox 🔵. |
| `User` profile / settings | Reuse vault note metadata | ⚪ deferred | Single-user for now; multi-tenant later. |
| `Membership` / Team support | — | ⚪ deferred | Solo user first; team-bookings unlock later. |
| `Host` / round-robin assignment | — | ⚪ deferred | Follows team support. |
| `Credential` (per-app OAuth tokens) | Architect auth bridge | 🔵 planned | We have `architect-auth` already; reuse. |

## Booking flow (the heart of cal.com)

| cal.com behavior | Our behavior | Status |
| --- | --- | --- |
| Public `/book/<user>/<event-slug>` page | Cal.com-style booking page | 🔵 planned (commit 2) |
| Listing open slots = `Schedule rules ∩ ¬existing bookings` | `slots::list_open_slots` + `VaultScheduler::list_open_slots` | 🟢 done — pure algorithm in `slots.rs` (5 tests), wired into `VaultScheduler` end-to-end (intersect test passes). |
| Slot timezone conversion (bookee's TZ vs host's) | TZ string on `AvailabilitySchedule`; conversion at UI render time | 🔵 planned |
| Buffer time before / after | `EventType.buffer_min` | 🟢 done (proto field exists, slot impl 🔵) |
| Minimum notice / max future booking window | EventType fields | 🔵 planned (add `min_notice_min`, `max_future_days`) |
| Daily / weekly booking limit | EventType fields | 🔵 planned |
| Custom booking questions (`form-builder`) | — | ⚪ deferred — `NewBooking.note` is a free-form catch-all for v1 |
| Confirmation: instant vs require-host-approval | `Booking.status` Pending → Confirmed transition | 🟢 done (status enum); auto-vs-manual flag on EventType 🔵 |
| No-show marking | `BookingStatus::NoShow` + `update_booking_status` | 🟢 done |
| Reschedule + cancel flow | New booking referencing prior + cancel mutation | 🔵 planned |
| Booking references (calendar IDs from external providers) | — | 🔵 planned (CalDAV → see Sync) |
| Recurring bookings | — | ⚪ deferred (RRULE on EventType after view-calendar's RRULE lands across the app) |

## Calendar sync

| cal.com integration | Our integration | Status |
| --- | --- | --- |
| Google Calendar (read busy + write bookings) | — | ⚪ deferred |
| Apple iCloud (CalDAV) | First-party CalDAV backend | 🔵 planned (high priority — user's primary sync target) |
| Microsoft Outlook / Office 365 | — | ⚪ deferred |
| Generic CalDAV | Same backend covers Apple + any RFC 4791 server | 🔵 planned |
| External calendar selection per event type | `EventType.calendar_id: Option<String>` | 🔵 planned (proto extension) |
| Webhooks on booking events | Architect event bus | 🔵 planned |
| iCal export (`.ics`) per booking | Static .ics generator in `scheduling::ical` | 🔵 planned |

## Conferencing / location

| cal.com integration | Our surface | Status |
| --- | --- | --- |
| In-person address | `EventTypeLocation::InPerson { address }` | 🟢 done |
| Phone | `EventTypeLocation::Phone` | 🟢 done |
| Generic URL (custom Zoom / Meet / etc.) | `EventTypeLocation::Link { url }` | 🟢 done |
| Cal Video (built-in) | — | 🔴 won't do (use any external link instead) |
| Zoom / Google Meet first-class | — | ⚪ deferred (the user pastes a URL on the event type for v1) |

## Notifications

| cal.com | Our plan | Status |
| --- | --- | --- |
| Email confirmation / reminders | SMTP via Vox or local stub | 🔵 planned |
| SMS reminders | — | ⚪ deferred |
| Slack / Discord | — | ⚪ deferred |
| ICS attachment on confirmation email | Reuse iCal export | 🔵 planned (paired with email) |

## Personal scheduling (our extension — *not* in cal.com)

These are the half of our feature cal.com doesn't cover at all —
the brief's daily-routine table.

| Surface | Status | Notes |
| --- | --- | --- |
| `DayTemplate` shape (ordered `TimeBlock`s + categories) | 🟢 done | Proto + markdown round-trip + scanner stub. |
| Markdown frontmatter round-trip | 🟢 done | Parser + writer in `scheduling::{parse,write}`. Tests pass. |
| Read-only `DayTemplateView` UI | 🟢 done | Renders the brief's example table with per-category color chips + summary chip row. |
| Day-template editor (drag time-block edges, inline rename, category swap) | 🔵 planned | Reuse the kanban/calendar inline-edit pattern. |
| Per-day overrides ("today Block 1 is sales call") | 🔵 planned | New entity: `DayInstance { date, template_id, allocations }`. |
| Allocation-into-blocks UI | 🔵 planned | Drop a task / event / project onto a Block; track utilization. |
| Aggregate stats (3 blocks / 7.5 h sleep / 1 h gym checks) | 🟡 partial | `Summary` chip row in the view; richer dashboard 🔵. |
| Template variants (Weekday vs Saturday vs travel) | 🟢 done | Multiple `DayTemplate` rows; UI picker 🔵. |

## CalDAV sync architecture (planned)

The proto is intentionally backend-agnostic. The CalDAV bridge slots
in as a `SchedulingService` impl:

```
┌────────────────────┐         ┌──────────────────────────────────┐
│ scheduling-ui      │────────▶│ trait SchedulingService          │
│ (Dioxus)           │         │ (#[architect::rpc] in proto)     │
└────────────────────┘         └─────────┬────────────────────────┘
                                         │
                  ┌──────────────────────┼────────────────────────┐
                  ▼                      ▼                        ▼
       InMemoryScheduler         VaultScheduler              CaldavScheduler
       (tests + demo)            (markdown round-trip)       (mirror to remote)
                                         │                        │
                                         ▼                        ▼
                                  vault::Vault            tower::caldav (TBD)
```

The CalDAV impl wraps the vault scheduler — every write fans out to
both the markdown vault *and* the remote server, with a sync token
held in the `.task/` sidecar (see below) so reconnects are
idempotent.

## Trait surface — capability sub-traits, no umbrella

We don't have one fat `SchedulingService`. The proto exposes five
`#[architect::rpc]` capability sub-traits (mirrors `wiki-proto`),
each with its own auto-emitted async client / dispatcher /
descriptor:

- `DayTemplates` — personal day-template CRUD.
- `EventTypes` — cal.com-style event-type CRUD.
- `Schedules` — availability-schedule CRUD.
- `Slots` — open-slot listing (derived; read-only).
- `Bookings` — booking CRUD + status transitions.

Function signatures bind to exactly the capabilities they touch,
so the requirements are self-documenting:

```rust
fn render_booking_page<S: EventTypes + Slots + Bookings>(s: &S, /* … */) { /* … */ }
fn save_template       <S: DayTemplates>                 (s: &S, /* … */) { /* … */ }
```

Backends mix + match. A read-only federation peer could impl just
`EventTypes + Slots`; the local `VaultScheduler` impls all five.
`InMemoryScheduler` (in the `scheduling` crate) is the test +
demo-route backend and impls everything.

## Storage split — content vs. app state

Two stores, one set of capability traits. The backend that owns
both is what `scheduling-ui` mounts.

**`<vault>/scheduling/` — markdown content (portable, git-friendly,
project-associated):**

- `templates/<slug>.md` — `DayTemplate`s.
- `event-types/<slug>.md` — `EventType`s.
- `schedules/<slug>.md` — `AvailabilitySchedule`s.
- `bookings/<uuid>.md` — `Booking`s. One file per booking so the
  meeting and its notes / attached project / agenda live together.

**`<vault>/.task/scheduling/` — app state (hidden, gitignored, can
swap backends freely):**

- `sync/<calendar-id>.json` — CalDAV sync tokens, ETags, last-seen
  revision per remote calendar.
- `cache/busy-times.json` — external-calendar busy-time cache
  (TTL'd, refreshed on demand).
- `audit/booking-events.jsonl` — append-only log of booking creates
  / status changes / webhook receipts.
- `credentials.json` — last-resort OAuth fallback when the OS
  keyring isn't available. Documented as machine-only, never
  synced.

> **Superseded (2026-07-27).** `store-proto` was deleted. It only
> ever shipped `MemStore`, the promised `store-json` / `store-sqlite`
> siblings were never written, and the server mounted the in-memory
> pair in production — so the one thing written through it (the
> booking audit trail) was lost on every restart. Nothing in the tree
> ever called `KvStore::put` or `LogStore::read`. The audit trail now
> lives where the rest of the slice's state lives: the vault, at
> `Records/audit/booking-events.jsonl` (`scheduling::audit`). The
> design below is kept for context; reach for a per-slice sqlite
> store (the tree's sea-orm idiom) if a future feature needs indexed
> app-state, not a resurrected generic proto.

App-state lives behind a **general** persistence proto so any
feature can use it, not just scheduling. `crates/store-proto/`
exposes two capability sub-traits — same pattern as the scheduling
ones:

- `KvStore` — `get` / `put` / `delete` / `list_keys` over
  opaque-byte values, scoped by `(namespace, key)`.
- `LogStore` — `append` / `read` / `truncate` for append-only
  channels (audit logs, webhook receipts).

Backends pick whichever they support. `MemStore` (in `store-proto`)
impls both for tests. Future siblings:

- `store-json` — JSON-on-disk under `<vault>/.task/<feature>/`.
- `store-sqlite` — single SQLite file when we want indexed queries.

The scheduling backend internally:

```rust
pub struct VaultScheduler {
    vault: vault::Vault,                 // → <vault>/scheduling/*.md
    kv:    Box<dyn KvStore + Send + Sync>,   // sync tokens, busy cache
    log:   Box<dyn LogStore + Send + Sync>,  // audit, webhook receipts
}

impl DayTemplates for VaultScheduler { /* reads `vault` */ }
impl EventTypes   for VaultScheduler { /* reads `vault` */ }
impl Schedules    for VaultScheduler { /* reads `vault` */ }
impl Bookings     for VaultScheduler {
    fn create_booking(&self, b: &NewBooking) -> ... {
        // 1. write markdown to vault
        // 2. log.append("audit", "booking-created:<id>")
        // 3. kv.delete("scheduling.cache", "busy:<calendar>") — invalidate
    }
}
impl Slots for VaultScheduler {
    fn list_open_slots(&self, q: &SlotQuery) -> ... {
        // Read schedule from vault, busy cache from kv, intersect.
    }
}
```

`VaultScheduler::new()` accepts boxed trait objects so callers
swap the backend without touching the proto or the UI:

```rust
let scheduler = VaultScheduler::new(
    vault,
    Box::new(JsonStore::open(".task/scheduling")?),
    Box::new(JsonStore::open(".task/scheduling")?),  // or SqliteStore::open(...)
);
```

This mirrors how Obsidian splits content (`*.md`) from app state
(`.obsidian/`). We keep portability where it matters (every
template / event-type / schedule / booking is plain markdown the
user can grep + diff + ship to git) and free ourselves to pick
the right store for high-churn machine data.

## Out of scope (locked)

These are cal.com features we explicitly aren't building:

- 🔴 **Multi-tenant SaaS hosting** — we ship a desktop / self-host product.
- 🔴 **Stripe / payment** — pay-to-book is not in our product surface.
- 🔴 **Cal Video / first-party video** — use any external URL.
- 🔴 **Marketing landing pages / blog / docs hosting** — not the product.
- 🔴 **Embed JS SDK / iframe SDK** — public booking page is enough; teams hosting it elsewhere is a follow-up.
- 🔴 **App-store ecosystem** — cal.com's "Apps" plugin layer doesn't translate; our extensibility is the vault + architect bus.

## Roadmap order

Suggested order for follow-up commits (top = next):

1. ~~**Vault-backed `SchedulingService`**~~ — 🟢 done. `VaultScheduler`
   round-trips every entity through `<vault>/scheduling/*.md`,
   keeps sidecar state (the booking audit trail) durable in the
   vault as JSONL, and
   includes a real slot-generation algorithm (rules ∩ ¬bookings).
   21/21 tests cover parse/write + slot edges + end-to-end on-disk
   roundtrips.
2. **Event-type editor UI** — form for title / duration / location /
   schedule pick. Drives `upsert_event_type`.
3. **Schedule editor UI** — weekly grid of availability rules. Click
   to add a window; drag edges to resize. Per-date overrides next.
4. **Public booking page UI** — slot list + booking form. The first
   client-facing surface; the existing `view-calendar` time-grid is
   the natural starting point for the slot picker.
5. **CalDAV backend** — Apple iCloud first. New crate
   `scheduling-caldav` implementing the same capability sub-traits,
   mounted via `architect::serve` so the UI talks to it through vox
   the same way it talks to the in-memory backend.
6. **Day-template editor** — drag block edges + inline rename +
   category swap. Reuse view-table inline-edit patterns.
7. **Allocation flow** — drag a `task` / `view-calendar` event onto
   a Block; track per-block utilization. Surfaces the "Block 1 is
   only 60 % allocated" view the user mentioned.
8. **Notifications** — email confirmation + .ics attachment.
9. **Recurring bookings** — pair with view-calendar's RRULE support.
10. **Team bookings** — multi-user shapes (Membership / Host /
    round-robin).
