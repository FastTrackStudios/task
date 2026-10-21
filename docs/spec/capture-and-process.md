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
gesture. The ✎ capture offers **Process now** while what is typed is
still a plain capture; it opens the same card the deck uses, holding the
capture unstored until the first decision writes it once, and closing it
undecided files it in the inbox, open. Quick-add opens that card
directly.

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
thing it became leaves the capture in the inbox, not lost. What a
decision writes back to the capture is decided once, for every surface,
by `inbox_proto::Decision::apply` (`features/inbox/inbox-proto/src/decision.rs`):
`Into(<what it became>)`, `Done`, `Later(response)`, `Keep`, `Delete`;
the note and reference shapes live beside it.

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
as a chip. Tags and contexts lift out of the row's text. Bare numbers
(`#12`) and email addresses are not marks.

---

### People are named in place

t[process.capture.people]
A person — a contact, or an org member — named by `@handle` or by their
capitalised name is drawn **where they are named**: a small avatar and
their first name, or their full name when the words wrote it out
(`@casey-lane`, "Casey Lane"). Tapping them opens their profile
(`/people/<contact id>`) whether or not they are in the org. Every
person is in exactly one context (`Contact::context`: their first group,
or `people` until sorted), and naming them brings that context onto the
capture's row. Parsing lives in `crates/ui/src/people.rs` (`marks`).

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

## Who made it

Each bot that works for the people of an org (Atlas, Commander, a
team's helper) has a Task identity of its own. So a capture or a task
records who made it, as well as where it came from.

### Every capture and task names its creator

t[attribution.created-by]
An inbox item and a task each carry `created_by`, the principal that
made them: `user:<account uuid>` for a person, or `bot:<account uuid>`
for a bot account (an address under the bot domain). They also carry
`created_by_name`, the account's display name at that moment, so a
surface can say who made it without a directory. `source` keeps its
own meaning: the *surface* the item came in through (`ui`, `cli`,
`watch`, `mcp`, `email`). An agent's capture says `mcp` there, never
`agent`. Records made before attribution existed have no creator, and
they stay valid.

### The server says who, never the caller

t[attribution.server-stamped]
`created_by` is set by the server from the credentials behind the
write, and never from the payload. On the RPC lanes that means the
permission gate's caller. Over MCP it means the bearer's principal: a
bot key's bot, or a session's person. On the watch bridge it means the
device's account. Whatever creator a client sends is dropped first,
so no caller can name someone else, and a write with no principal
behind it records none.

### A creator is written once

t[attribution.immutable]
Once a record has a creator, nothing changes it. That covers an
update, a processing decision, a stale client copy, an upsert that
leaves the field out, and a text version merged in from a folder edit
or another replica (`vault_doc::merge_text` keeps `created_by` /
`createdBy` from the first version that had one).

### Lists filter by creator

t[attribution.filter]
`TaskListFilter::created_by`, `Inbox::list_inbox_by` and the
`created_by` argument of the MCP `list_tasks` / `list_inbox` tools keep
one creator's records. Each takes a full ref, a bare account uuid, or
`none` for unattributed records. All of them share one rule,
`workflows_proto::creator::creator_matches`.

### Surfaces say who

t[attribution.shown]
An inbox row and a task's page say "from you" for the signed-in
account's own records, and "from <name>" for anyone else's
(`creator_label`). A record with no creator says nothing.

### A personal bot says whom it acted for

t[attribution.on-behalf-of]
When a bot works for one person ("Atlas for Cody"), what it creates
also carries `on_behalf_of`: that person's ref, `user:<account uuid>`,
plus `on_behalf_of_name`, their display name at that moment. A
person's own records, and a team bot's, carry none. Like the creator,
it is written once: an update, a stale copy, an upsert that leaves it
out and a replica merge (`vault_doc::WRITE_ONCE_PROPS`) all keep the
first value.

### On behalf of comes from the key, never the caller

t[attribution.on-behalf-of.server-stamped]
`on_behalf_of` is set by the server from the authenticated bot's key:
the **owner** in the key's metadata (`{bot: true, owner, personal}`),
when the key marks the bot personal (`bots::acts_for`). It is never
taken from tool or RPC arguments; whatever a payload says is dropped
with the creator it claims (`attribution::attribute_*`). A bot account
holds exactly one key, issued with the account, so the account's key is
the key that authenticated.

### Personal or team is a mark on the bot

t[attribution.bot-kind]
Every bot has an owner, the person who made it and alone can see or
revoke it. Whether it *acts for* that owner is a separate, explicit
mark, `personal`, kept beside the owner on the key's metadata:
`admin bot create --personal`, the app's "My assistant — acts for me"
switch (`CreateBotRequest::personal`), and `admin bot personal|team
--bot <uuid>` to change it later. A team bot (Commander, a studio
helper) is the default, and so is every key made before the mark, so
nothing is claimed for anyone until the owner says so. A change
applies to records made after it; earlier ones keep what they say.

### Surfaces say whom it was for

t[attribution.on-behalf-of.shown]
An inbox row and a task's page say "from Atlas (for Cody)", or
"from Atlas (for you)" to the person it was for
(`creator::attribution_label`). A team bot's record says only
"from <name>". The assignee is shown on its own, in the task's
assignee row, never folded into this line.

### Lists filter on behalf of me

t[attribution.on-behalf-of.filter]
`TaskListFilter::on_behalf_of` and the `on_behalf_of` argument of the
MCP `list_tasks` / `list_inbox` tools keep the records made for one
person, by the same rule as `created_by` (a ref, a bare uuid or
`none`). Every MCP person filter also takes `me`, the caller's own ref.
The Tasks and Project work boards' Filter sheet offers **Assignee**,
**Created by** and **On behalf of** sections, the signed-in person
listed first as "Me", so "on behalf of me" is one pick.

### Assignment is not creation

t[attribution.assignee.separate]
Who does a task is its assignee (`workflow.assignees`), never its
creator: Atlas may make a task that Cody hands to an employee. The
assignee is a principal ref, a person or a bot account by account id
(`AgentRef::Human`), or a named agent; it changes over time, and
nothing about it touches `created_by` or `on_behalf_of`.

### Assigning is logged with who did it

t[attribution.assignee.logged]
Each assignment change is an activity entry (`Assigned` /
`Unassigned`, from → to) that names the actor who made it, with the
time. On the RPC lanes that is the gate's caller; over MCP the tools
that write tasks (`update_task`, `file_task`, `claim_task`,
`create_task`, `email_to_task`) note the bearer's principal the same
way, so the feed never has to guess the actor from the assignee.

### Lists filter by assignee

t[attribution.assignee.filter]
`TaskListFilter::assignee` and the `assignee` argument of the MCP
`list_tasks` tool keep one assignee's tasks: `user:<uuid>` or a bare
uuid for a person or bot account, `agent:<name>` for a named agent,
`none` for unassigned, and `me` over MCP
(`creator::assignee_matches`). An employee asking for `me` sees
exactly the work assigned to them.

### The seed shows every kind of creator

t[attribution.seed]
The planted world has two bots in acme-audio: Atlas, Alice's personal
assistant, and the Studio Desk, a team bot. Its captures and tasks mix
the kinds of creator: Alice, Sam and Atlas each made some, the Studio
Desk made a chore for nobody in particular, and a few have none,
standing for records from before attribution. What Atlas made says
"for Alice". Priya, a second employee, has the work Atlas made for
Alice and Alice handed on.

---

## Where each rule is checked

| rule | checked by |
|---|---|
| `process.capture.single-path`, `.verbatim`, `process.outcome.task` | `tests/integration/tests/it/process.rs` — capture on the Inbox lane, task on the Task lane, back-link read back and walked (`decision::origin_of`) |
| `process.capture.process-now` | `task-ui` `composer::tests::process_now_is_offered_while_it_is_still_a_capture` (when the door shows); `apps/tests/playwright/capture-process-now.spec.js` drives the ✎ door, the card it opens with every outcome, and closing undecided into the inbox |
| `process.outcome.{waiting,now,note,reference,project,later,done,delete}`, `process.suggestion`, `process.undo` | `process.rs` — each outcome as the card makes it (the thing on its own lane, then the write `Decision::apply` makes), each walked back from what it made; snooze, done and delete undone to exactly the stored item; a note's create-only clash leaving the capture open |
| the same decisions with no host | `apps/server/tests/it/capture_peers_e2e.rs` — two device nodes, no host: processed, snoozed and deleted on one, seen so on the other; a decision on the other comes back |
| `process.due` | `process.rs` (the planted inbox over the wire) and `example_org::declared_tests::the_planted_inbox_has_captures_to_process` |
| `process.capture.marks`, `.people` | `people` unit tests (what is a mark; people by handle or name, first or full); `example_org::declared_tests::every_person_a_capture_names_has_a_profile`; `example_org::declared_tests::the_seeded_inbox_shows_people_tags_and_contexts`; `process.rs` serves the planted captures verbatim |
| `process.suggested` | `task-proto::suggest` unit tests (the order, "Not now", what is running); `apps/server/tests/it/watch_suggested_e2e.rs::the_watch_suggests_and_acts_on_the_same_pick` — planned task, a template's maintenance hour opening a "Maintenance hour" session, the fall-through to overdue and captures, over the watch bridge |
| `process.focus.*` (but `.everywhere`) | unit tests in `task-proto::relevance` and `scheduling-proto::focus`; `process.rs` over `TaskService::query`; the seed's maintenance hour in `example_org::declared_tests` |
| `attribution.created-by`, `.server-stamped`, `.filter` | `apps/server/tests/it/attribution_e2e.rs`: a bot key and a person's session, each over MCP and the person also over the RPC lane, every call claiming someone else in its arguments; each record names its real creator, and the four filters each keep one creator's records. Also `attribution::tests`, and the filter rule in `workflows-proto::creator` |
| `attribution.immutable` | `vault_inbox::tests::created_by_is_write_once`, `task::backend::tests::created_by_is_write_once_and_filters`, `vault_doc::doc::tests::a_creator_survives_every_later_text` (replica merges); `attribution_e2e.rs` edits over the wire |
| `attribution.shown` | `workflows-proto::creator::tests::the_label_says_you_or_the_name` (the label both the inbox row and the task page render) |
| `attribution.on-behalf-of`, `.on-behalf-of.server-stamped` | `attribution_e2e.rs::records_say_who_made_them_and_nobody_can_claim_otherwise` (Atlas's key records Atlas for Cody whatever the arguments claim; a payload's `on_behalf_of` over the RPC lane is dropped); `attribution::tests::whom_a_bot_acted_for_comes_from_the_creator_alone`; write-once in `vault_inbox::tests::created_by_is_write_once`, `task::backend::tests::on_behalf_of_is_write_once_and_both_filters_work`, `vault_doc::doc::tests::a_creator_survives_every_later_text` |
| `attribution.bot-kind` | `bots::tests::a_key_from_before_the_marker_is_a_team_bot`; the team bot in `attribution_e2e.rs` records no `on_behalf_of` whatever it claims; `org-proto/tests/bots_wire_compat.rs` (apps that never send `personal` still make bots) |
| `attribution.on-behalf-of.shown` | `workflows-proto::creator::tests::a_personal_bot_says_whom_it_acted_for` |
| `attribution.on-behalf-of.filter` | `attribution_e2e.rs` (`on_behalf_of: me` over MCP `list_tasks` / `list_inbox`, and `TaskListFilter::on_behalf_of` over RPC); `ui::pages::tasks::tests::the_filter_sheet_offers_assignee_creator_and_on_behalf_of_me` |
| `attribution.assignee.separate`, `.logged`, `.filter` | `attribution_e2e.rs::an_employee_gets_the_work_an_assistant_made_and_the_handoff_is_logged` (Atlas makes it, Cody assigns it to Priya over MCP, the creator stays Atlas, the feed's `Assigned` entry names Cody, Priya's `assignee: me` and RPC `assignee` filter find it); `workflows-proto::creator::tests::the_assignee_rule`; `task::backend::tests::on_behalf_of_is_write_once_and_both_filters_work` |
| `attribution.seed` | `example_org::declared_tests::the_seed_has_mixed_creators_from_its_own_org` (a personal and a team bot, a second employee holding assistant-made work); `demo_plant.rs::demo_plants_an_inbox_to_process_and_a_maintenance_hour` (a planted capture from Atlas for Alice, one from Alice, one unattributed, tasks from both, Atlas's task for Alice assigned to Priya, the Studio Desk's for nobody) |

The seeded world shows the whole loop to Alice: twelve captures in
acme-audio's inbox (one suggested from mail, one snoozed two weeks out,
two carrying `@errands` / `#chore`, and two marked up with a person, a
`#tag` and an `@context` — Sam by handle, Casey by name — and one naming
Jordan, a mastering engineer who is a contact but not a member; and three
already decided — one processed into the "Return the hired mic stands"
task, one rewritten as the committed note "The B room hum is the fridge
on the studio circuit", quoting it verbatim, and one already done —
`example_org::declared_tests::the_seed_has_a_capture_in_every_state_and_each_walks_back`),
weekday and weekend templates with
a "Maintenance" block (16:00–17:00 on weekdays), and
three open album tasks tagged `@errands`, `@chores` and `#chore` for that
hour to bring forward. What is not met is in [unmet](unmet.md).
