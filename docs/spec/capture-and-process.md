# Capture and Process Spec

The core of Task is two acts. **Capture** gets a thought out of a head
with no questions asked. **Process** comes back to each capture once and
decides what it actually is. Everything else — the board, Home, the
maintenance hour — is downstream of those two, and a system people trust
is one where nothing captured is ever lost and everything captured is
eventually decided.

Cross-cutting: the inbox (`features/inbox`), tasks (`features/task`),
projects, the wiki and the day plan (`features/scheduling`) all answer to
it. The app's surfaces are `crates/ui/src/process.rs` (the decision),
`crates/ui/src/pages/inbox.rs` (the deck) and `crates/ui/src/now_block.rs`
(the hour).

The vocabulary:

| Word | What it is |
|---|---|
| **capture** | one `inbox_proto::InboxItem`, a markdown file at `Records/inbox/<id>.md` in the org's vault |
| **due** | a capture waiting to be decided today |
| **outcome** | what processing turned a capture into |
| **steering block** | the time block running now whose kind brings some work forward |
| **focus** | the context / tag names a steering block brings forward |

Answers to charter
[#2 A Single Trusted System](https://github.com/FastTrackStudios/task/issues/2).

---

## Capture

### There is one way in

t[process.capture.single-path]
Every capture — the ✎ button, quick-add, the CLI, the watch, an agent —
lands in the org's inbox as an `InboxItem` before it is anything else.
Quick-add does not create a task directly: it captures, then opens
processing on that one item; the first decision writes it once, and
closing without deciding files it in the inbox, open. Nothing typed is
ever lost, and there is one queue to drain rather than one per surface.

---

### Any capture can be processed on the spot

t[process.capture.process-now]
A capture surface in the app offers to process what was just captured,
there and then, rather than only filing it for later. The deck is for the
backlog; a capture whose answer is obvious is decided in the same
gesture.

---

### A capture keeps its words

t[process.capture.verbatim]
The item's `body` is the capture verbatim, and processing never rewrites
it. A processed item keeps its words and names what it became in
`processed_into` — a task's path, `project:<id>`, or a note's path — so
the walk from a durable thing back to the thought that started it always
exists.

---

## What is due

### Due means open and resurfaced, or suggested

t[process.due]
An item is due for processing on a day when it is `open` and has no
`resurface_on` date or one that has come, **or** when it is `suggested` —
proposed by something else (mail ingestion, an agent) and awaiting a yes
or no. Processed and archived items are never due. The inbox deck, the
tab badge and Home's count use this one rule, so they never disagree
about how much is waiting.

---

### A suggestion is kept or dismissed

t[process.suggestion]
A suggested item is shown as one, with where it came from, and is out of
the trusted review queue until a person keeps it (→ `open`) or dismisses
it (deleted). Any other decision on it — task, later, note — accepts it
in passing.

---

## Outcomes

Each outcome is one decision, and each says exactly what it writes. The
item's own status change is the last write, so a failure creating the
thing it became leaves the capture in the inbox, not lost.

### A task

t[process.outcome.task]
"Make a task" parses the capture's first line the way quick-add parses
(`task_proto::capture`: `@contexts`, `#tags`, `[[Project]]`, dates,
`!priority`, recurrence) into an editable title, project, contexts, due
date and estimate; the rest of the capture becomes the task's details. It
creates the task, then writes the item `processed` with `processed_into`
set to the task's path. A task leaves the inbox already filed, which is
what lets a maintenance hour find it.

---

### Waiting on someone

t[process.outcome.waiting]
"Waiting on someone" is a task shaped the same way, created with status
`waiting`, and the item processed into it as for a task.

---

### Do it now

t[process.outcome.now]
"Do it now" — the two-minute rule — creates the task `in-progress` with
one open time entry starting now, and processes the item into it. The
clock is already running when the card closes.

---

### A project

t[process.outcome.project]
"It's a project" creates a project titled from the capture's first line
and writes the item `processed` with `processed_into = project:<id>`.

---

### A note

t[process.outcome.note]
"Note" writes an atomic wiki note at `Wiki/Atomic/<slug>-<id6>.md`
carrying the whole capture, and writes the item `processed` with
`processed_into` set to the note's path.

---

### Later

t[process.outcome.later]
"Later" (and Clarify's "Someday") puts the item off by spaced repetition
(`inbox_proto::review`): Soon, Maybe and Someday are the Hard, Good and
Easy responses, each showing how far off it lands. The item stays `open`
with a new `resurface_on`, interval, ease and review count, and is not
due until that date.

---

### Done

t[process.outcome.done]
"Done" — already handled — writes the item `processed` with no
`processed_into`: it became nothing, and that is recorded rather than
deleted.

---

### Delete

t[process.outcome.delete]
"Delete" removes the item's file. A capture from quick-add that was never
stored writes nothing at all.

---

## Modes

### Fast puts every outcome on the card

t[process.mode.fast]
In **Fast** mode the card shows the outcomes side by side — make a task,
later, note, done, delete (and skip, in the deck) — each one key away
(`t`, `l`, `n`, `d`, `x`, space). The mode is the person's choice,
remembered per device, and Fast is the default.

---

### Clarify asks the questions in order

t[process.mode.clarify]
In **Clarify** mode the card asks, one at a time:

1. *Is this actionable?*
2. If yes — *What's the next step?* — **Do it now**, **Make it a task**,
   **Waiting on someone**, or **It's a project**.
3. If no — *Keep it?* — **Note**, **Someday** (later, Easy), or
   **Delete**.

Each question can step back to the one before. The outcomes it reaches
are the same outcomes Fast reaches, written the same way.

---

### Undo where undoing is honest

t[process.undo]
Later, Done and Delete offer Undo, which writes the item back exactly as
it was before the decision and steps the deck back to it. Outcomes that
made something new elsewhere — a task, a project, a note — do not offer
Undo, because undoing them would mean deleting work the person can now
see in another place.

---

## Time-block focus

A day plan says what each part of the day is for. That only matters if,
during that part, the task list leads with that work.

### One block steers the hour

t[process.focus.steering]
The steering block at a minute is the block running then (start
inclusive, end exclusive, across midnight) whose kind brings something
forward. Where blocks overlap, a fixed block wins, then the one that
started last. A day with no saved plan steers from its weekday / weekend
template. Work blocks, sleep and `other` steer nothing.

---

### Each kind of block brings its own work forward

t[process.focus.surfaces]
Each block kind names its focus as lowercase context / tag names with no
sigil (`scheduling_proto::BlockCategory::surfaces`). A **maintenance**
block brings forward `errands`, `errand`, `chores`, `chore`, `home`,
`admin` and `maintenance`; a meal block `mealprep` and `kitchen`;
exercise `exercise` and `health`; spiritual `spiritual`, `prayer` and
`journal`; reset and hygiene `morning`; wind-down `evening`. Both
singular and plural are listed because people write both.

---

### Focus opens its own gates

t[process.focus.opens]
A gate context named in the focus is met: during a maintenance hour an
`@errands` task is relevant wherever the person says they are. Outside
the hour the gate is shut again and the task waits for its place or
time.

---

### Focus ranks first, after the work in hand

t[process.focus.ranks]
A task matches the focus when it carries one of its names as an
`@context` **or** a `#tag`. Matching tasks rank right after the task in
progress and ahead of active-project work, due work and everything else,
so the hour's list leads with what the hour is for.

---

### Every "what now" surface reads the same focus

t[process.focus.everywhere]
Every surface that answers "what should I do now" — the web board, Home,
the CLI's `--relevant`, the morning brief — resolves the steering block
the same way and passes the same focus. A maintenance hour that exists in
the browser and not in the terminal is two systems.

---

### What you are on is part of the context

t[process.context.device]
The device a person is using (`phone`, `computer`, …) is filled in by the
client, so an `@phone` or `@computer` task is relevant on that device
without the person saying so.

---

## Where each rule is checked

| rule | checked by |
|---|---|
| `process.capture.single-path`, `.verbatim`, `process.outcome.task` | `tests/integration/tests/it/process.rs` — capture on the Inbox lane, task on the Task lane, back-link read back |
| `process.due` | `process.rs` (the planted inbox over the wire) and `example_org::declared_tests::the_planted_inbox_has_captures_to_process` |
| `process.focus.*` (but `.everywhere`) | unit tests in `task-proto::relevance` and `scheduling-proto::focus`; `process.rs` over `TaskService::query`; the seed's maintenance hour in `example_org::declared_tests` |

The seeded world shows the whole loop to Alice: six captures in
acme-audio's inbox (one suggested from mail, one snoozed two weeks out,
two carrying `@errands` / `#chore`), weekday and weekend templates with
an "Errands & chores" maintenance block (16:00–17:00 on weekdays), and
three open album tasks tagged `@errands`, `@chores` and `#chore` for that
hour to bring forward. What is not met is in [unmet](unmet.md).
