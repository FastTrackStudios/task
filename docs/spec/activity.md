# Activity Spec

One history of everything that happens in an org — every task change,
comment, capture, decision, timer and mode — that a person can scroll back
through, filter, and open from. The notifications bell is the *queue*
("this needs you", read/unread); activity is the *history* (append-only,
never read-flipped). Both are written by the same server-side notifier
from the same event hubs, and they live side by side in the org's
`notify.sqlite`.

The pieces:

| piece | where |
|---|---|
| the wire contract — `Activity`, `ActivityKind`, `ActivityFilter`, `ActivityCursor`, `#[architect::rpc] trait ActivityFeed { list, #[subscribe] events }` | `features/notify/notify-proto/src/activity.rs` |
| the store — table `notify_activity`, `ActivityStore::{record, insert_if_absent}` | `features/notify/notify/src/activity.rs` |
| the rules (pure), the author seams, the comment hook | `apps/server/src/activity.rs` |
| the pumps that drive the rules | `apps/server/src/notifier.rs` |
| the page, the timeline, the words | `crates/ui/src/activity.rs` |
| the seed | `apps/server/src/example_org.rs` (`SEED_ACTIVITY`, `SEED_COMMENTS`, `plant_activity`) |

Why the notify feature and not a crate of its own: the queue and the
history are produced by the same rules from the same hubs, stored in the
same per-org database, and read by the same surfaces (the bell links to
`/activity`). A second crate would be a second migrator, a second pool and
a second snapshot entry for one table.

---

## The feed

### Newest first, and it scrolls back

t[activity.feed.order]
`ActivityFeed::list` returns entries newest first — `at` descending, `id`
descending as the tie-break — at most `limit` of them (50 by default, 500
at most). `before` is a cursor: only entries strictly older than it in
that same order. Pages fetched by passing the last entry of one page as
the next page's `before` tile the whole feed with no gap and no repeat.

---

### It narrows by what you ask

t[activity.feed.filter]
Every set field of `ActivityFilter` narrows (AND): `entity_type` +
`entity_id` is one thing's history; `project_id` is a project's — the
project's own entries *and* its tasks', because every task entry carries
its task's project; `kinds` keeps only those verbs; `actor_id` keeps one
person's ("only mine"). `ActivityFilter::matches` is the one predicate the
server's query and the client's live fold agree through.

---

### It is live

t[activity.feed.live]
Every recorded entry is published on `ActivityFeed::events` as
`ActivityEvent::Recorded` the moment it is written. Subscribers fetch a
page after subscribing and fold — so `/activity`, a task's timeline and a
project's update as things happen, on any device.

---

### Clients read it; the server writes it

t[activity.feed.server-written]
There is no client write. Entries are recorded by the notifier, by the
comment hook, and by the seed; the permit tables grant `list` and
`events` only.

---

## What is recorded

### A task change says what changed

t[activity.task.diff]
For each task the notifier keeps what it last looked like (seeded from
`TaskService::list` on every (re)subscribe, because the hubs replay
nothing). A full-state `TaskEvent::Upserted` is compared with it, and each
difference is one entry, with `field`, `from` and `to`:

| change | kind |
|---|---|
| first seen after the seed | `created` |
| title | `renamed` |
| status into a terminal state / out of one / otherwise | `completed` / `reopened` / `status_changed` |
| primary assignee gained or changed / lost | `assigned` / `unassigned` |
| priority | `priority_changed` |
| due | `due_changed` |
| description | `edited` — once per editing burst (ten minutes), not per save |

`TaskEvent::Deleted` is `deleted`, titled from the last state seen. Every
entry carries the task's title, its project and `/tasks/<id>`.

---

### A comment is recorded where its thread is anchored

t[activity.comment]
Posting a message through `ThreadsService::post_message` records a
`commented` entry about the thread's anchor (a task or a project), with
the comment as `body`, the author's label as the byline, and the anchor's
title, project and route. Threads have no change stream; the server's
`ThreadsService` mount is the hook.

---

### Capturing and processing are recorded

t[activity.inbox]
An inbox item first seen open (or suggested) is `captured`. An item that
was not processed and now is, is `processed`, and names what it became:
`processed_into` is resolved to a task (title, `/tasks/<id>`), a project
(`project:<id>` → title, `/projects/<id>`) or a note (its path).

---

### Timers and modes

t[activity.timer.mode]
A timer session arriving open that was not running is `timer_started`; one
that was running arriving closed is `timer_stopped`, with its duration as
`body`. A session whose description is a steering kind's label
("Maintenance hour" — `BlockCategory::focus_label`) is a *mode*:
`mode_started` / `mode_ended` instead. Sessions logged after the fact, and
edits to a running session, record nothing.

---

### Projects

t[activity.project]
A project first seen after the seed is `project_created`; a change of
title or status is `project_updated` with `field` saying which. Project
entries carry their own id as `project_id`, so a project's feed starts
with the project.

---

### Who did it

t[activity.actor]
An entry names the signed-in person whose call made the change, whenever
the change came through the org router with a session: the permission
gate's caller, carried across the blocking-thread hop for the sync
services (`TaskService`, `Inbox`) by `ActorDispatcher` and noted per
entity in the `ActorLedger`, and read directly by the async ones
(`TimerService`, `ThreadsService`). The byline is the auth store's display
name. Where no caller is known the entry says "Someone" — never an empty
name. What falls short of this is in [unmet](unmet.md#activity).

---

## In the app

### `/activity`

t[activity.ui.page]
`/activity` is a core page with a nav tab of its own and a "View all
activity" door at the foot of the notifications bell. It lists the
selected org's feed newest first, grouped under day headings ("Today",
"Yesterday", "Mon, Oct 12"); each row is the actor's initial, a sentence,
and a relative time, and tapping it opens the entry's `href`. "Load
earlier" fetches the page before the oldest row shown, until the feed's
beginning. The header's main action marks every notification read; its ⋯
opens Filter (a sheet on a phone: search, kind groups — Tasks, Comments,
Inbox, Time & modes, Projects — and projects) and "Only mine"; on a desk
the same filters sit as chips under the header.

---

### The words

t[activity.ui.sentence]
A row reads as a sentence: "Alice moved *Comp lead vocals* to In
Progress", "Sam commented on *Comp lead vocals*", "Alice ended
*Maintenance hour* after 47 min". On the thing's own page the title is
dropped and Linear's phrasing used: "changed status from Open to In
Progress", "self-assigned the issue".

---

### A thing's own timeline

t[activity.ui.entity]
A task's page ends with its timeline, oldest first: its recorded changes
and its comments inline, then a composer — "Leave a comment…", Enter to
send, Shift-Enter for a new line — that posts into the task's thread,
opening one on the first comment. A project's page carries the same
timeline over the project's feed (its own entries and its tasks').

---

## The seed

t[activity.seed]
A fresh plant of acme-audio opens on history: a few entries about Alice's
board (she took and started *Comp lead vocals — Track One*, wrapped the
drums, raised the label review's priority) and a comment thread on the
vocal comp — Sam's note and Alice's reply. Every seeded line names a
declared task of its org and a cast member of that org, under a fixed id,
so a replant writes nothing twice.

---

## Where each rule is checked

| rule | checked by |
|---|---|
| `activity.feed.order`, `.filter` | `notify::activity::tests::filters_and_the_before_cursor_tile_the_feed` (store); `apps/server/tests/it/activity_e2e.rs::the_feed_records_what_happened_end_to_end` (entity, project and kind filters, pages of two tiling the feed — over the router) |
| `activity.feed.live` | `activity_e2e.rs` — the status change arrives on the `ActivityFeed` stream |
| `activity.feed.server-written` | the permit tables (`permits.rs`: `rd "list"`, `rd "events"` only), held to the router by `permits_cover_router` |
| `activity.task.diff` | `task_server::activity::tests` (each change, completion/reopen, rename, edit coalescing); `activity_e2e.rs` (create + status change with from/to over `TaskService`) |
| `activity.comment` | `activity_e2e.rs` — a thread opened and a message posted over `ThreadsService`, recorded with byline, body, title and project |
| `activity.inbox` | `activity::tests::captures_and_their_processing`; `activity_e2e.rs` — capture, then processed into a task, resolved to its title and route |
| `activity.timer.mode` | `activity::tests::a_maintenance_hour_is_a_mode_and_a_plain_timer_is_a_timer`; `activity_e2e.rs` — a "Maintenance hour" timer started and stopped over `TimerService` |
| `activity.project` | `activity::tests::a_project_change_names_the_field` |
| `activity.actor` | `activity::tests::the_ledger_names_the_router_caller_and_forgets`; `activity_e2e.rs::the_feed_names_the_signed_in_writer` — a task completed over the WebSocket with a session bearer is bylined with that account |
| `activity.ui.page` | `ui::nav::tests::the_activity_tab_is_offered_to_every_org` |
| `activity.ui.sentence` | `ui::activity::tests` |
| `activity.ui.entity` | not checked by a test — see [unmet](unmet.md#activity) |
| `activity.seed` | `example_org::declared_tests::the_seeded_history_is_about_the_seed`; `apps/server/tests/it/demo_plant.rs::demo_plants_activity_history_and_a_comment_thread` (the real `admin demo`, twice) |
