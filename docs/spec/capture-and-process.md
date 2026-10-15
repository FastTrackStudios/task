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

### One capture surface; it becomes a task by being given one

t[process.capture.as]
The capture button, the fleeting-note door and New task open one
full-screen surface. Nobody chooses "note" or "task": a capture is
written down, verbatim, into the inbox for later — **until it is given
something a task has**. Choosing a person, a date, a priority, a status,
a project, a label, a repeat or a subtask (with the chips, or in the
text: `[[Project]]`, `@context`, "tomorrow", `!high`) makes it a task,
created at once and assigned to its maker unless someone else was
chosen (`process.outcome.mine`). The one deliberate choice is
**Capture** or **Project**, a project being the more structured thing;
a line beginning `project:` or `new project` picks it. A plain capture
may start a timer instead. A project made here is a capture processed
on the spot (`process.outcome.project`), so it keeps its words.

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

### Processed work is yours

t[process.outcome.mine]
Every task processing makes — "make a task", "waiting on someone", "do it
now" — is assigned to the person who processed it (a human assignee in
the task's `workflow`), and so is an issue made in the composer unless
another assignee is chosen. The Tasks list is what is assigned to the
signed-in person (`task_proto::is_mine`); unassigned or someone else's
work is documented work, listed under Project work. A capture that became
a task therefore always lands on its maker's list.

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
card stays on it while the clock runs ("Doing: …", the elapsed time):
**Done** closes the entry and sets the task `done`; **Pause** closes the
entry and sets the task back to `todo`, on the list to come back to.
Either way the time spent is recorded on the task.

---

### A project

t[process.outcome.project]
"It's a project" creates a project titled from the capture's first line
and writes the item `processed` with `processed_into = project:<id>`.

---

### A note, in your own words

t[process.outcome.note]
"Write a note" is the rewrite: the person titles the idea, says it in
their own words, and picks the one parent it belongs under (a note of
the vault). It writes `<Title>.md` with `tags: [atomic]`, `created`, and
`folder: "[[Parent]]"` when a parent was picked; the body is what they
wrote, followed by the capture verbatim in a `> [!quote] Original
capture` callout, so the source is never lost. The write is create-only
— a title already taken fails the save and the capture stays in the
inbox. The item is written `processed` with `processed_into` set to the
note's path.

---

### Keep as reference

t[process.outcome.reference]
"Keep as reference" files the capture as it stands — no rewrite — at
`Wiki/Atomic/<slug>-<id6>.md`, carrying the whole capture, and writes the
item `processed` into it. It is for what is worth keeping but not yet
worth thinking through.

---

### Later

t[process.outcome.later]
"Snooze" puts the item off by spaced repetition
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

## The card

### One card, every outcome

t[process.card]
There is one way to process — no modes to choose between. The card leads
with the four outcomes a capture usually wants: **Do it now** (`d`),
**Add to a project** (`p`, the task shape — project, contexts, date),
**Write a note** (`n`, the rewrite) and **Snooze** (`s`). Beneath them,
smaller: **Keep as reference** (`r`), **It's a project**, **Already done**,
**Delete** (`x`), and **Skip** (space) in the deck. Each step past the
first can step back to it.

---

### A capture's marks

t[process.capture.marks]
A capture's row reads its words for marks without changing them: a
`#tag` shows as a badge (its colour fixed by the tag), an `@context`
as a chip, and a person — an org member or contact named by `@handle`
(their first name) or by their capitalised first name — as an avatar.
Tags and contexts lift out of the row's text; an `@handle` reads as the
name. Bare numbers (`#12`) and email addresses are not marks.

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

### The hour suggests; the person starts the mode

t[process.focus.mode]
A steering block does not change the list by itself. While it runs, the
Tasks page offers to start its mode ("It's your maintenance hour ·
Start"), with how much is waiting, and "Not now" quiets the offer for
that block that day. Starting opens a timer session named for the kind
(`BlockCategory::focus_label`, "Maintenance hour"), stopping whatever was
running; **that open session is the mode**, and only while it runs does
the focus below apply. Ending the mode stops the session, so the timer
log records when maintenance actually began and how long it took. The
focus rules below read "the hour" as "the running mode".

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

## Suggested

### One suggestion for now

t[process.suggested]
The top of the Tasks list holds one suggestion — never a list — the
first of these that applies (`task_proto::suggest`):

1. the task the calendar planned for now (a running block with a task
   assigned) — **Start** opens a timer on it;
2. the running steering block's mode (getting ready, a maintenance
   hour, wind-down) — **Start** opens the mode's session
   (`process.focus.mode`);
3. work in progress;
4. the oldest overdue task;
5. captures waiting to be processed;
6. something due today.

What is already running is not suggested again. "Not now" sets a
suggestion aside for the day and the next one shows. While a mode runs
the slot is the mode's band. Every client — web, phone, the watch bridge
— calls the same pick with the same plan, so they suggest the same thing
at the same minute.

The watch reads it from the bridge (`GET /org/{slug}/watch/v1/suggested`),
sending its own local date and minute — the server does not know the
wearer's timezone — and the keys it set aside. The server resolves that
date's plan with its weekday/weekend template exactly as the calendar
does (`scheduling_proto::resolve_day`), counts the inbox's due captures
(`InboxItem::is_due`), and acts on a tap
(`POST …/suggested/act`) only while the suggestion still applies:
Start opens the timer session, Done completes the task, and processing
captures is left to the phone.

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
| `process.capture.marks` | `pages::inbox` unit tests (what is a mark, people by handle or name); `example_org::declared_tests::the_seeded_inbox_shows_people_tags_and_contexts`; `process.rs` serves the planted captures verbatim |
| `process.suggested` | `task-proto::suggest` unit tests (the order, "Not now", what is running); `apps/server/tests/it/watch_suggested_e2e.rs::the_watch_suggests_and_acts_on_the_same_pick` — planned task, a template's maintenance hour opening a "Maintenance hour" session, the fall-through to overdue and captures, over the watch bridge |
| `process.focus.*` (but `.everywhere`) | unit tests in `task-proto::relevance` and `scheduling-proto::focus`; `process.rs` over `TaskService::query`; the seed's maintenance hour in `example_org::declared_tests` |

The seeded world shows the whole loop to Alice: eight captures in
acme-audio's inbox (one suggested from mail, one snoozed two weeks out,
two carrying `@errands` / `#chore`, and two marked up with a person, a
`#tag` and an `@context` — Sam's avatar, Casey by name), weekday and weekend templates with
an "Errands & chores" maintenance block (16:00–17:00 on weekdays), and
three open album tasks tagged `@errands`, `@chores` and `#chore` for that
hour to bring forward. What is not met is in [unmet](unmet.md).
